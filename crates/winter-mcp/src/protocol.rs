//! MCP protocol types for JSON-RPC communication.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request from Claude Code.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// JSON-RPC response to Claude Code.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// MCP initialize request params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

/// Client capabilities sent during initialization.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub roots: Option<Value>,
    #[serde(default)]
    pub sampling: Option<Value>,
}

/// Client info sent during initialization.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// MCP initialize response result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

/// Server capabilities advertised during initialization.
#[derive(Debug, Clone, Serialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
}

/// Tools capability.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCapability {
    pub list_changed: bool,
}

/// Server info returned during initialization.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Tool definition for tools/list response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Result of tools/list.
#[derive(Debug, Clone, Serialize)]
pub struct ListToolsResult {
    pub tools: Vec<ToolDefinition>,
}

/// Parameters for tools/call.
#[derive(Debug, Clone, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(default)]
    pub arguments: HashMap<String, Value>,
}

/// Result of tools/call.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// Content returned from a tool call.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
}

impl ToolContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

impl CallToolResult {
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(text)],
            is_error: Some(false),
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(text)],
            is_error: Some(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // JsonRpcRequest tests

    #[test]
    fn json_rpc_request_deserializes_minimal() {
        let json = r#"{"jsonrpc": "2.0", "method": "test"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "test");
        assert!(req.id.is_none());
        assert!(req.params.is_none());
    }

    #[test]
    fn json_rpc_request_deserializes_with_id_number() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "test"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(json!(1)));
    }

    #[test]
    fn json_rpc_request_deserializes_with_id_string() {
        let json = r#"{"jsonrpc": "2.0", "id": "abc-123", "method": "test"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(json!("abc-123")));
    }

    #[test]
    fn json_rpc_request_deserializes_with_params() {
        let json = r#"{"jsonrpc": "2.0", "method": "test", "params": {"key": "value"}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.params, Some(json!({"key": "value"})));
    }

    // JsonRpcResponse tests

    #[test]
    fn json_rpc_response_success_serializes() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"status": "ok"}));
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"]["status"], "ok");
        assert!(parsed.get("error").is_none());
    }

    #[test]
    fn json_rpc_response_error_serializes() {
        let resp = JsonRpcResponse::error(Some(json!(1)), -32600, "Invalid Request");
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["error"]["code"], -32600);
        assert_eq!(parsed["error"]["message"], "Invalid Request");
        assert!(parsed.get("result").is_none());
    }

    #[test]
    fn json_rpc_response_null_id() {
        let resp = JsonRpcResponse::success(None, json!("result"));
        let json = serde_json::to_string(&resp).unwrap();
        // With None id, id field should be omitted
        assert!(!json.contains("\"id\""));
    }

    // InitializeParams tests

    #[test]
    fn initialize_params_deserializes() {
        let json = r#"{
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "Claude Code",
                "version": "1.0.0"
            }
        }"#;
        let params: InitializeParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.protocol_version, "2024-11-05");
        assert_eq!(params.client_info.name, "Claude Code");
        assert_eq!(params.client_info.version, "1.0.0");
    }

    #[test]
    fn initialize_params_with_capabilities() {
        let json = r#"{
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "roots": {"listChanged": true},
                "sampling": {}
            },
            "clientInfo": {
                "name": "test",
                "version": "1.0"
            }
        }"#;
        let params: InitializeParams = serde_json::from_str(json).unwrap();
        assert!(params.capabilities.roots.is_some());
        assert!(params.capabilities.sampling.is_some());
    }

    // InitializeResult tests

    #[test]
    fn initialize_result_serializes() {
        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ServerCapabilities {
                logging: None,
                prompts: None,
                resources: None,
                tools: Some(ToolsCapability { list_changed: true }),
            },
            server_info: ServerInfo {
                name: "winter".to_string(),
                version: "0.1.0".to_string(),
            },
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["protocolVersion"], "2024-11-05");
        assert_eq!(parsed["serverInfo"]["name"], "winter");
        assert_eq!(parsed["capabilities"]["tools"]["listChanged"], true);
        // Empty optional fields should be omitted
        assert!(parsed["capabilities"].get("logging").is_none());
    }

    // ToolDefinition tests

    #[test]
    fn tool_definition_serializes() {
        let def = ToolDefinition {
            name: "create_fact".to_string(),
            description: "Create a new fact".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "predicate": {"type": "string"}
                },
                "required": ["predicate"]
            }),
        };

        let json = serde_json::to_string(&def).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["name"], "create_fact");
        assert_eq!(parsed["description"], "Create a new fact");
        assert_eq!(parsed["inputSchema"]["type"], "object");
    }

    // CallToolParams tests

    #[test]
    fn call_tool_params_deserializes() {
        let json = r#"{
            "name": "create_fact",
            "arguments": {
                "predicate": "follows",
                "args": ["did:plc:a", "did:plc:b"]
            }
        }"#;
        let params: CallToolParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.name, "create_fact");
        assert_eq!(params.arguments.get("predicate"), Some(&json!("follows")));
    }

    #[test]
    fn call_tool_params_no_arguments() {
        let json = r#"{"name": "list_facts"}"#;
        let params: CallToolParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.name, "list_facts");
        assert!(params.arguments.is_empty());
    }

    // CallToolResult tests

    #[test]
    fn call_tool_result_success_serializes() {
        let result = CallToolResult::success("Operation completed");
        let json = serde_json::to_string(&result).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["content"][0]["type"], "text");
        assert_eq!(parsed["content"][0]["text"], "Operation completed");
        assert_eq!(parsed["isError"], false);
    }

    #[test]
    fn call_tool_result_error_serializes() {
        let result = CallToolResult::error("Something went wrong");
        let json = serde_json::to_string(&result).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["content"][0]["text"], "Something went wrong");
        assert_eq!(parsed["isError"], true);
    }

    // ToolContent tests

    #[test]
    fn tool_content_text_serializes() {
        let content = ToolContent::text("Hello, world!");
        let json = serde_json::to_string(&content).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["type"], "text");
        assert_eq!(parsed["text"], "Hello, world!");
    }

    // ListToolsResult tests

    #[test]
    fn list_tools_result_serializes() {
        let result = ListToolsResult {
            tools: vec![
                ToolDefinition {
                    name: "tool1".to_string(),
                    description: "First tool".to_string(),
                    input_schema: json!({}),
                },
                ToolDefinition {
                    name: "tool2".to_string(),
                    description: "Second tool".to_string(),
                    input_schema: json!({}),
                },
            ],
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["tools"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["tools"][0]["name"], "tool1");
        assert_eq!(parsed["tools"][1]["name"], "tool2");
    }

    #[test]
    fn list_tools_result_empty() {
        let result = ListToolsResult { tools: vec![] };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert!(parsed["tools"].as_array().unwrap().is_empty());
    }
}
