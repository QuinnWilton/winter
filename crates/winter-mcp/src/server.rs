//! MCP server implementation with stdin/stdout JSON-RPC handling.

use std::io::{self, BufRead, Write};

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

/// MCP server that handles JSON-RPC over stdin/stdout.
pub struct McpServer {
    tools: ToolRegistry,
    initialized: bool,
}

impl McpServer {
    pub fn new(tools: ToolRegistry) -> Self {
        Self {
            tools,
            initialized: false,
        }
    }

    /// Run the server, reading from stdin and writing to stdout.
    pub async fn run(&mut self) -> Result<(), McpError> {
        info!("MCP server starting");

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            debug!(request = %line, "received request");

            let response = self.handle_line(&line).await;

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

    async fn handle_line(&mut self, line: &str) -> Option<JsonRpcResponse> {
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

        // Handle notifications (no id) - don't send response
        if request.id.is_none() {
            self.handle_notification(&request).await;
            return None;
        }

        let result = self.handle_request(&request).await;
        Some(match result {
            Ok(value) => JsonRpcResponse::success(request.id, value),
            Err(e) => JsonRpcResponse::error(request.id, -32603, e),
        })
    }

    async fn handle_notification(&mut self, request: &JsonRpcRequest) {
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

    async fn handle_request(&mut self, request: &JsonRpcRequest) -> Result<Value, String> {
        match request.method.as_str() {
            "initialize" => self.handle_initialize(request).await,
            "tools/list" => self.handle_list_tools().await,
            "tools/call" => self.handle_call_tool(request).await,
            _ => Err(format!("Unknown method: {}", request.method)),
        }
    }

    async fn handle_initialize(&mut self, request: &JsonRpcRequest) -> Result<Value, String> {
        let _params: InitializeParams = request
            .params
            .as_ref()
            .map(|p| serde_json::from_value(p.clone()))
            .transpose()
            .map_err(|e| format!("Invalid initialize params: {}", e))?
            .ok_or("Missing initialize params")?;

        self.initialized = true;

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

    async fn handle_call_tool(&self, request: &JsonRpcRequest) -> Result<Value, String> {
        let params: CallToolParams = request
            .params
            .as_ref()
            .map(|p| serde_json::from_value(p.clone()))
            .transpose()
            .map_err(|e| format!("Invalid call params: {}", e))?
            .ok_or("Missing call params")?;

        debug!(tool = %params.name, "executing tool");

        let result = self.tools.execute(&params.name, &params.arguments).await;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }
}
