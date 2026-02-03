//! Raw PDS access tools for direct record manipulation.
//!
//! These tools provide low-level access to ATProto records, enabling debugging
//! and extensibility without going through typed record structures.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};

use super::ToolState;

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "pds_list_records".to_string(),
            description: "List records in any ATProto collection with pagination. Returns raw record data without type validation.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "collection": {
                        "type": "string",
                        "description": "The collection NSID (e.g., 'diy.razorgirl.winter.fact', 'app.bsky.feed.post')"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of records to return (1-100, default 50)",
                        "minimum": 1,
                        "maximum": 100
                    },
                    "cursor": {
                        "type": "string",
                        "description": "Pagination cursor from a previous response"
                    }
                },
                "required": ["collection"]
            }),
        },
        ToolDefinition {
            name: "pds_get_record".to_string(),
            description: "Get a single record by collection and rkey. Returns raw record data.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "collection": {
                        "type": "string",
                        "description": "The collection NSID (e.g., 'diy.razorgirl.winter.fact')"
                    },
                    "rkey": {
                        "type": "string",
                        "description": "The record key"
                    }
                },
                "required": ["collection", "rkey"]
            }),
        },
        ToolDefinition {
            name: "pds_put_record".to_string(),
            description: "Create or update a record with raw JSON. WARNING: Bypasses validation. The $type field is set automatically from the collection.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "collection": {
                        "type": "string",
                        "description": "The collection NSID (e.g., 'diy.razorgirl.winter.fact')"
                    },
                    "rkey": {
                        "type": "string",
                        "description": "The record key"
                    },
                    "record": {
                        "type": "object",
                        "description": "The record data (JSON object). The $type field will be added automatically."
                    }
                },
                "required": ["collection", "rkey", "record"]
            }),
        },
        ToolDefinition {
            name: "pds_delete_record".to_string(),
            description: "Delete a record by collection and rkey. WARNING: Cannot be undone. Prefer domain-specific tools for Winter records.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "collection": {
                        "type": "string",
                        "description": "The collection NSID (e.g., 'diy.razorgirl.winter.fact')"
                    },
                    "rkey": {
                        "type": "string",
                        "description": "The record key"
                    }
                },
                "required": ["collection", "rkey"]
            }),
        },
    ]
}

/// List records in a collection.
pub async fn pds_list_records(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let collection = match arguments.get("collection").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: collection"),
    };

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|l| l.clamp(1, 100) as u32)
        .unwrap_or(50);

    let cursor = arguments.get("cursor").and_then(|v| v.as_str());

    match state
        .atproto
        .list_records::<Value>(collection, Some(limit), cursor)
        .await
    {
        Ok(response) => {
            let records: Vec<Value> = response
                .records
                .into_iter()
                .map(|item| {
                    json!({
                        "uri": item.uri,
                        "cid": item.cid,
                        "rkey": extract_rkey(&item.uri),
                        "value": item.value
                    })
                })
                .collect();

            let count = records.len();

            let mut result = json!({
                "collection": collection,
                "count": count,
                "records": records
            });

            if let Some(next_cursor) = response.cursor {
                result["cursor"] = json!(next_cursor);
            }

            CallToolResult::success(result.to_string())
        }
        Err(e) => CallToolResult::error(format!("Failed to list records: {}", e)),
    }
}

/// Get a single record.
pub async fn pds_get_record(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let collection = match arguments.get("collection").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: collection"),
    };

    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state.atproto.get_record::<Value>(collection, rkey).await {
        Ok(response) => CallToolResult::success(
            json!({
                "uri": response.uri,
                "cid": response.cid,
                "collection": collection,
                "rkey": rkey,
                "value": response.value
            })
            .to_string(),
        ),
        Err(winter_atproto::AtprotoError::NotFound { .. }) => {
            CallToolResult::error(format!("Record not found: {}/{}", collection, rkey))
        }
        Err(e) => CallToolResult::error(format!("Failed to get record: {}", e)),
    }
}

/// Create or update a record.
pub async fn pds_put_record(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let collection = match arguments.get("collection").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: collection"),
    };

    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    let record = match arguments.get("record") {
        Some(Value::Object(obj)) => obj.clone(),
        Some(_) => return CallToolResult::error("Parameter 'record' must be an object"),
        None => return CallToolResult::error("Missing required parameter: record"),
    };

    // Convert to Value for put_record (it will add $type automatically)
    let record_value = Value::Object(record);

    match state
        .atproto
        .put_record(collection, rkey, &record_value)
        .await
    {
        Ok(response) => CallToolResult::success(
            json!({
                "uri": response.uri,
                "cid": response.cid,
                "collection": collection,
                "rkey": rkey
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to put record: {}", e)),
    }
}

/// Delete a record.
pub async fn pds_delete_record(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let collection = match arguments.get("collection").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: collection"),
    };

    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state.atproto.delete_record(collection, rkey).await {
        Ok(()) => CallToolResult::success(
            json!({
                "deleted": true,
                "collection": collection,
                "rkey": rkey
            })
            .to_string(),
        ),
        Err(winter_atproto::AtprotoError::NotFound { .. }) => {
            CallToolResult::error(format!("Record not found: {}/{}", collection, rkey))
        }
        Err(e) => CallToolResult::error(format!("Failed to delete record: {}", e)),
    }
}

/// Extract the rkey from an AT URI.
fn extract_rkey(uri: &str) -> String {
    uri.rsplit('/').next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rkey() {
        assert_eq!(
            extract_rkey("at://did:plc:abc/diy.razorgirl.winter.fact/3abc123"),
            "3abc123"
        );
        assert_eq!(
            extract_rkey("at://did:plc:abc/app.bsky.feed.post/xyz"),
            "xyz"
        );
        assert_eq!(extract_rkey(""), "");
    }

    #[test]
    fn test_definitions_count() {
        let defs = definitions();
        assert_eq!(defs.len(), 4);
    }

    #[test]
    fn test_definition_names() {
        let defs = definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"pds_list_records"));
        assert!(names.contains(&"pds_get_record"));
        assert!(names.contains(&"pds_put_record"));
        assert!(names.contains(&"pds_delete_record"));
    }
}
