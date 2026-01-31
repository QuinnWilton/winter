//! Identity tools for MCP.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::Identity;

use super::ToolState;

/// Collection name for identity.
const IDENTITY_COLLECTION: &str = "diy.razorgirl.winter.identity";

/// Record key for the singleton identity record.
const IDENTITY_RKEY: &str = "self";

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "get_identity".to_string(),
            description: "Get your current identity (values, interests, self_description).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "update_identity".to_string(),
            description: "Update your identity. All fields are optional - only provided fields will be changed.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "self_description": {
                        "type": "string",
                        "description": "Replace your entire self_description"
                    },
                    "add_values": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Values to add"
                    },
                    "remove_values": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Values to remove"
                    },
                    "add_interests": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Interests to add"
                    },
                    "remove_interests": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Interests to remove"
                    }
                }
            }),
        },
    ]
}

pub async fn get_identity(
    state: &ToolState,
    _arguments: &HashMap<String, Value>,
) -> CallToolResult {
    // Try cache first, fall back to HTTP
    if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            if let Some(cached) = cache.get_identity().await {
                tracing::debug!("using cache for get_identity");
                return CallToolResult::success(
                    json!({
                        "values": cached.value.values,
                        "interests": cached.value.interests,
                        "self_description": cached.value.self_description,
                        "created_at": cached.value.created_at.to_rfc3339(),
                        "last_updated": cached.value.last_updated.to_rfc3339()
                    })
                    .to_string(),
                );
            }
        }
    }

    match state
        .atproto
        .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_RKEY)
        .await
    {
        Ok(record) => CallToolResult::success(
            json!({
                "values": record.value.values,
                "interests": record.value.interests,
                "self_description": record.value.self_description,
                "created_at": record.value.created_at.to_rfc3339(),
                "last_updated": record.value.last_updated.to_rfc3339()
            })
            .to_string(),
        ),
        Err(winter_atproto::AtprotoError::NotFound { .. }) => {
            CallToolResult::error("Identity not found. Run 'winter bootstrap' to initialize.")
        }
        Err(e) => CallToolResult::error(format!("Failed to get identity: {}", e)),
    }
}

pub async fn update_identity(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    // First get the existing identity
    let mut identity = match state
        .atproto
        .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_RKEY)
        .await
    {
        Ok(record) => record.value,
        Err(winter_atproto::AtprotoError::NotFound { .. }) => {
            return CallToolResult::error(
                "Identity not found. Run 'winter bootstrap' to initialize.",
            );
        }
        Err(e) => return CallToolResult::error(format!("Failed to get identity: {}", e)),
    };

    let mut changes = Vec::new();

    // Update self_description if provided
    if let Some(desc) = arguments.get("self_description").and_then(|v| v.as_str()) {
        identity.self_description = desc.to_string();
        changes.push("self_description");
    }

    // Add values
    if let Some(add_values) = arguments.get("add_values").and_then(|v| v.as_array()) {
        for v in add_values {
            if let Some(value) = v.as_str()
                && !identity.values.contains(&value.to_string())
            {
                identity.values.push(value.to_string());
            }
        }
        if !add_values.is_empty() {
            changes.push("add_values");
        }
    }

    // Remove values
    if let Some(remove_values) = arguments.get("remove_values").and_then(|v| v.as_array()) {
        for v in remove_values {
            if let Some(value) = v.as_str() {
                identity.values.retain(|x| x != value);
            }
        }
        if !remove_values.is_empty() {
            changes.push("remove_values");
        }
    }

    // Add interests
    if let Some(add_interests) = arguments.get("add_interests").and_then(|v| v.as_array()) {
        for v in add_interests {
            if let Some(interest) = v.as_str()
                && !identity.interests.contains(&interest.to_string())
            {
                identity.interests.push(interest.to_string());
            }
        }
        if !add_interests.is_empty() {
            changes.push("add_interests");
        }
    }

    // Remove interests
    if let Some(remove_interests) = arguments.get("remove_interests").and_then(|v| v.as_array()) {
        for v in remove_interests {
            if let Some(interest) = v.as_str() {
                identity.interests.retain(|x| x != interest);
            }
        }
        if !remove_interests.is_empty() {
            changes.push("remove_interests");
        }
    }

    if changes.is_empty() {
        return CallToolResult::error("No changes specified");
    }

    // Update the last_updated timestamp
    identity.last_updated = Utc::now();

    // Save the updated identity
    match state
        .atproto
        .put_record(IDENTITY_COLLECTION, IDENTITY_RKEY, &identity)
        .await
    {
        Ok(response) => CallToolResult::success(
            json!({
                "uri": response.uri,
                "cid": response.cid,
                "changes": changes,
                "values": identity.values,
                "interests": identity.interests,
                "self_description": identity.self_description
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to update identity: {}", e)),
    }
}
