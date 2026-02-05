//! Identity tools for MCP.
//!
//! The identity record is now slim (just operator_did and timestamps).
//! Values, interests, and self-description are stored as directives.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::Identity;

use super::{ToolMeta, ToolState};

/// Collection name for identity.
const IDENTITY_COLLECTION: &str = "diy.razorgirl.winter.identity";

/// Record key for the singleton identity record.
const IDENTITY_RKEY: &str = "self";

pub fn definitions() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "get_identity".to_string(),
        description: "Get your identity configuration (operator_did, timestamps). For values, interests, and self-description, use list_directives instead.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }]
}

/// Get all identity tools with their permission metadata.
/// All identity tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

pub async fn get_identity(
    state: &ToolState,
    _arguments: &HashMap<String, Value>,
) -> CallToolResult {
    match state
        .atproto
        .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_RKEY)
        .await
    {
        Ok(record) => CallToolResult::success(
            json!({
                "operator_did": record.value.operator_did,
                "created_at": record.value.created_at.to_rfc3339(),
                "last_updated": record.value.last_updated.to_rfc3339(),
                "note": "Values, interests, and self-description are now stored as directives. Use list_directives to view them."
            })
            .to_string(),
        ),
        Err(winter_atproto::AtprotoError::NotFound { .. }) => {
            CallToolResult::error("Identity not found. Run 'winter bootstrap' to initialize.")
        }
        Err(e) => CallToolResult::error(format!("Failed to get identity: {}", e)),
    }
}
