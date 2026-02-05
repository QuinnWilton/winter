//! MCP server implementation with stdin/stdout JSON-RPC handling.

use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use thiserror::Error;
use tracing::{debug, error, info};

use crate::{
    protocol::{
        CallToolParams, InitializeParams, InitializeResult, JsonRpcRequest, JsonRpcResponse,
        ListToolsResult, ServerCapabilities, ServerInfo, ToolsCapability,
    },
    tools::ToolRegistry,
};

/// Errors that can occur in the MCP server.
#[derive(Debug, Error)]
pub enum McpError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// MCP server that handles JSON-RPC over stdin/stdout or HTTP.
///
/// This server is thread-safe and can handle concurrent requests when used
/// with the HTTP transport. The `initialized` flag uses atomic operations
/// to avoid requiring mutable access for request handling.
pub struct McpServer {
    tools: ToolRegistry,
    initialized: AtomicBool,
}

impl McpServer {
    pub fn new(tools: ToolRegistry) -> Self {
        Self {
            tools,
            initialized: AtomicBool::new(false),
        }
    }

    /// Get a reference to the tool registry.
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    /// Run the server using stdio transport, reading from stdin and writing to stdout.
    pub async fn run(&self) -> Result<(), McpError> {
        self.run_stdio().await
    }

    /// Run the server using stdio transport, reading from stdin and writing to stdout.
    pub async fn run_stdio(&self) -> Result<(), McpError> {
        info!("MCP server starting (stdio transport)");

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            debug!(request = %line, "received request");

            let response = self.handle_request_str(&line).await;

            if let Some(response) = response {
                let response_json = serde_json::to_string(&response)?;
                debug!(response = %response_json, "sending response");
                writeln!(stdout, "{}", response_json)?;
                stdout.flush()?;
            }
        }

        info!("MCP server shutting down");
        Ok(())
    }

    /// Handle a JSON-RPC request string, returning an optional response.
    ///
    /// This is the transport-agnostic entry point for processing MCP requests.
    /// Returns `None` for notifications (which don't require responses).
    pub async fn handle_request_str(&self, line: &str) -> Option<JsonRpcResponse> {
        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(req) => req,
            Err(e) => {
                error!(error = %e, "failed to parse request");
                return Some(JsonRpcResponse::error(
                    None,
                    -32700,
                    format!("Parse error: {}", e),
                ));
            }
        };

        self.handle_request(&request).await
    }

    /// Handle a parsed JSON-RPC request, returning an optional response.
    ///
    /// This is the transport-agnostic entry point for processing MCP requests.
    /// Returns `None` for notifications (which don't require responses).
    /// This method is thread-safe and can be called concurrently.
    pub async fn handle_request(&self, request: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        self.handle_request_with_trigger(request, None).await
    }

    /// Handle a parsed JSON-RPC request with an optional trigger context.
    ///
    /// The trigger is passed through to tool execution for thought recording,
    /// allowing tool calls to be associated with their originating session.
    /// This method is thread-safe and can be called concurrently.
    pub async fn handle_request_with_trigger(
        &self,
        request: &JsonRpcRequest,
        trigger: Option<String>,
    ) -> Option<JsonRpcResponse> {
        // Handle notifications (no id) - don't send response
        if request.id.is_none() {
            self.handle_notification(request).await;
            return None;
        }

        let result = self
            .handle_request_inner_with_trigger(request, trigger)
            .await;
        Some(match result {
            Ok(value) => JsonRpcResponse::success(request.id.clone(), value),
            Err(e) => JsonRpcResponse::error(request.id.clone(), -32603, e),
        })
    }

    async fn handle_notification(&self, request: &JsonRpcRequest) {
        match request.method.as_str() {
            "notifications/initialized" => {
                debug!("client sent initialized notification");
            }
            "notifications/cancelled" => {
                debug!("client cancelled request");
            }
            _ => {
                debug!(method = %request.method, "unknown notification");
            }
        }
    }

    async fn handle_request_inner_with_trigger(
        &self,
        request: &JsonRpcRequest,
        trigger: Option<String>,
    ) -> Result<Value, String> {
        match request.method.as_str() {
            "initialize" => self.handle_initialize(request).await,
            "tools/list" => self.handle_list_tools().await,
            "tools/call" => self.handle_call_tool_with_trigger(request, trigger).await,
            _ => Err(format!("Unknown method: {}", request.method)),
        }
    }

    async fn handle_initialize(&self, request: &JsonRpcRequest) -> Result<Value, String> {
        let _params: InitializeParams = request
            .params
            .as_ref()
            .map(|p| serde_json::from_value(p.clone()))
            .transpose()
            .map_err(|e| format!("Invalid initialize params: {}", e))?
            .ok_or("Missing initialize params")?;

        self.initialized.store(true, Ordering::SeqCst);

        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ServerCapabilities {
                logging: None,
                prompts: None,
                resources: None,
                tools: Some(ToolsCapability {
                    list_changed: false,
                }),
            },
            server_info: ServerInfo {
                name: "winter".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    async fn handle_list_tools(&self) -> Result<Value, String> {
        let result = ListToolsResult {
            tools: self.tools.definitions(),
        };
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    async fn handle_call_tool_with_trigger(
        &self,
        request: &JsonRpcRequest,
        trigger: Option<String>,
    ) -> Result<Value, String> {
        let params: CallToolParams = request
            .params
            .as_ref()
            .map(|p| serde_json::from_value(p.clone()))
            .transpose()
            .map_err(|e| format!("Invalid call params: {}", e))?
            .ok_or("Missing call params")?;

        debug!(tool = %params.name, "executing tool");

        let result = self
            .tools
            .execute_with_trigger(&params.name, &params.arguments, trigger)
            .await;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }
}
