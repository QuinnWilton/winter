//! HTTP transport for the MCP server.
//!
//! This module provides an axum-based HTTP server that exposes the MCP protocol
//! over HTTP, allowing Claude Code to connect via HTTP instead of spawning
//! a stdio process for each request.
//!
//! The server handles requests concurrently - multiple tool invocations can
//! run in parallel without blocking each other.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::{
    protocol::{JsonRpcRequest, JsonRpcResponse},
    server::McpServer,
    tools::InterruptionState,
};

/// Application state for the HTTP server.
///
/// The `McpServer` is thread-safe and can handle concurrent requests,
/// so no mutex is needed here.
pub struct HttpState {
    server: McpServer,
    /// Shared interruption state for background sessions.
    interruption: Arc<InterruptionState>,
}

impl HttpState {
    /// Create a new HTTP state wrapping an MCP server.
    pub fn new(server: McpServer) -> Self {
        let interruption = Arc::new(InterruptionState::new());
        Self {
            server,
            interruption,
        }
    }

    /// Create a new HTTP state with a shared interruption state.
    pub fn with_interruption(server: McpServer, interruption: Arc<InterruptionState>) -> Self {
        Self {
            server,
            interruption,
        }
    }

    /// Get the interruption state.
    pub fn interruption(&self) -> &Arc<InterruptionState> {
        &self.interruption
    }
}

/// Create an axum router for the MCP HTTP server.
pub fn create_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/mcp", post(handle_mcp))
        .route("/health", get(handle_health))
        .route("/interrupt", post(handle_interrupt))
        .route("/interrupt", axum::routing::delete(handle_clear_interrupt))
        .route("/builtin-tool-call", post(handle_builtin_tool_call))
        .with_state(state)
}

/// Handle MCP JSON-RPC requests.
///
/// This endpoint accepts JSON-RPC requests and returns JSON-RPC responses.
/// It implements the MCP streamable HTTP transport. Requests are handled
/// concurrently - multiple tool invocations can run in parallel.
async fn handle_mcp(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    debug!(method = %request.method, "received MCP request");

    // Extract trigger from header (set by daemon via WINTER_TRIGGER env var)
    let trigger = headers
        .get("X-Winter-Trigger")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(String::from);

    let response = state
        .server
        .handle_request_with_trigger(&request, trigger)
        .await;

    match response {
        Some(resp) => {
            debug!(id = ?resp.id, "sending MCP response");
            (StatusCode::OK, Json(resp))
        }
        None => {
            // Notification - no response needed, but HTTP requires something
            (
                StatusCode::NO_CONTENT,
                Json(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: None,
                }),
            )
        }
    }
}

/// Health check endpoint for Docker.
async fn handle_health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Request body for setting interruption.
#[derive(Debug, Deserialize)]
pub struct InterruptRequest {
    /// Reason for interruption (e.g., "queue_pressure").
    pub reason: String,
}

/// Response for interruption endpoints.
#[derive(Debug, Serialize)]
pub struct InterruptResponse {
    pub success: bool,
    pub interrupted: bool,
    pub reason: Option<String>,
}

/// Request body for recording built-in tool calls.
///
/// This is used by the agent to record Claude's built-in tool calls (WebSearch, Read, etc.)
/// as Thought records, so they appear in the thoughtstream alongside MCP tool calls.
#[derive(Debug, Deserialize)]
pub struct BuiltinToolCallRequest {
    /// Claude's tool use ID (e.g., "toolu_01ABC")
    pub id: String,
    /// Name of the tool (e.g., "WebSearch", "Read")
    pub name: String,
    /// Input arguments passed to the tool
    pub input: serde_json::Value,
    /// What triggered this tool call (notification URI, job name, etc.)
    pub trigger: Option<String>,
}

/// Response for built-in tool call recording.
#[derive(Debug, Serialize)]
pub struct BuiltinToolCallResponse {
    pub success: bool,
}

/// Set the interruption state (called by daemon when notifications arrive).
async fn handle_interrupt(
    State(state): State<Arc<HttpState>>,
    Json(request): Json<InterruptRequest>,
) -> impl IntoResponse {
    debug!(reason = %request.reason, "setting interruption state");
    state.interruption.set_interrupt(&request.reason).await;

    let (interrupted, reason) = state.interruption.check().await;
    (
        StatusCode::OK,
        Json(InterruptResponse {
            success: true,
            interrupted,
            reason,
        }),
    )
}

/// Clear the interruption state (called by daemon after session ends).
async fn handle_clear_interrupt(State(state): State<Arc<HttpState>>) -> impl IntoResponse {
    debug!("clearing interruption state");
    state.interruption.clear().await;

    (
        StatusCode::OK,
        Json(InterruptResponse {
            success: true,
            interrupted: false,
            reason: None,
        }),
    )
}

/// Record a built-in Claude tool call as a Thought.
///
/// This endpoint is called by the agent after each Claude invocation to log
/// built-in tool usage (WebSearch, Read, WebFetch, etc.) as Thought records.
/// This allows built-in tools to appear in the thoughtstream alongside MCP tools.
async fn handle_builtin_tool_call(
    State(state): State<Arc<HttpState>>,
    Json(request): Json<BuiltinToolCallRequest>,
) -> impl IntoResponse {
    debug!(tool = %request.name, id = %request.id, "recording built-in tool call");

    // Record via the tool registry's thought channel
    state
        .server
        .tools()
        .record_builtin_tool_call(&request.name, &request.id, &request.input, request.trigger)
        .await;

    (
        StatusCode::OK,
        Json(BuiltinToolCallResponse { success: true }),
    )
}

/// Run the MCP HTTP server on the specified port.
pub async fn run_server(server: McpServer, port: u16) -> Result<(), std::io::Error> {
    let interruption = Arc::new(InterruptionState::new());

    // Set the interruption state on the tool registry so check_interruption works
    server
        .tools()
        .set_interruption(Arc::clone(&interruption))
        .await;

    let state = Arc::new(HttpState::with_interruption(server, interruption));
    let router = create_router(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;

    info!("MCP HTTP server listening on http://0.0.0.0:{}", port);

    axum::serve(listener, router).await?;

    Ok(())
}

/// Health check response type.
#[derive(serde::Serialize)]
pub struct HealthResponse {
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolRegistry;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    fn create_test_state() -> Arc<HttpState> {
        // Create a minimal tool registry for testing
        // In tests, we don't have a real ATProto client
        Arc::new(HttpState::new(McpServer::new(ToolRegistry::empty())))
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let state = create_test_state();
        let router = create_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_mcp_initialize() {
        let state = create_test_state();
        let router = create_router(state);

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "test",
                    "version": "1.0.0"
                }
            }
        });

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert!(json["result"]["protocolVersion"].is_string());
        assert_eq!(json["result"]["serverInfo"]["name"], "winter");
    }

    #[tokio::test]
    async fn test_mcp_tools_list() {
        let state = create_test_state();
        let router = create_router(state.clone());

        // First initialize
        let init_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "test",
                    "version": "1.0.0"
                }
            }
        });

        let _init_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&init_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Create a new router with the same state (since oneshot consumes the router)
        let router = create_router(state);

        // Then list tools
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        });

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 2);
        assert!(json["result"]["tools"].is_array());
    }

    #[tokio::test]
    async fn test_mcp_notification_no_response_body() {
        let state = create_test_state();
        let router = create_router(state);

        // Notifications have no id
        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Notifications return 204 No Content
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_mcp_unknown_method() {
        let state = create_test_state();
        let router = create_router(state);

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "unknown/method"
        });

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert!(json["error"].is_object());
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Unknown method")
        );
    }
}
