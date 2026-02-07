//! Fact declaration tools for MCP.
//!
//! Fact declarations define predicate schemas before facts of that type exist.
//! This enables ad-hoc queries with proper type info and serves as documentation.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{AtUri, FactDeclaration, Tid, WriteOp, WriteResult};

use super::{ToolMeta, ToolState, parse_args};

/// Collection name for fact declarations.
const DECLARATION_COLLECTION: &str = "diy.razorgirl.winter.factDeclaration";

/// Safely truncate a string to a maximum number of characters.
/// This handles UTF-8 correctly by counting characters, not bytes.
pub(crate) fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "create_fact_declaration".to_string(),
            description: r#"Declare a fact predicate schema before facts of that type exist.

Use this to:
- Enable queries for predicates that don't have facts yet
- Document what predicates mean and their argument structure
- Plan future behavior with undeclared predicates

Example:
```
create_fact_declaration(
  predicate: "thread_completed",
  args: [
    {name: "thread_uri", description: "AT URI of the thread"},
    {name: "outcome", description: "How the thread ended"}
  ],
  description: "Records when a conversation thread has concluded",
  tags: ["conversation", "tracking"]
)
```"#.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "predicate": {
                        "type": "string",
                        "description": "The predicate name (max 64 chars)"
                    },
                    "args": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {
                                    "type": "string",
                                    "description": "Argument name (max 64 chars)"
                                },
                                "type": {
                                    "type": "string",
                                    "description": "Argument type (default: symbol)"
                                },
                                "description": {
                                    "type": "string",
                                    "description": "What this argument represents (max 256 chars)"
                                }
                            },
                            "required": ["name"]
                        },
                        "description": "Argument definitions (max 10)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Human-readable description of what this predicate represents (max 1024 chars)"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tags for categorization (max 20)"
                    }
                },
                "required": ["predicate", "args", "description"]
            }),
        },
        ToolDefinition {
            name: "create_fact_declarations".to_string(),
            description: "Create multiple fact declarations in a single atomic transaction. All declarations are created together or none are.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "declarations": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "predicate": {
                                    "type": "string",
                                    "description": "The predicate name (max 64 chars)"
                                },
                                "args": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "name": { "type": "string" },
                                            "type": { "type": "string" },
                                            "description": { "type": "string" }
                                        },
                                        "required": ["name"]
                                    },
                                    "description": "Argument definitions (max 10)"
                                },
                                "description": {
                                    "type": "string",
                                    "description": "Human-readable description (max 1024 chars)"
                                },
                                "tags": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Tags for categorization (max 20)"
                                }
                            },
                            "required": ["predicate", "args", "description"]
                        },
                        "description": "Array of declarations to create"
                    }
                },
                "required": ["declarations"]
            }),
        },
        ToolDefinition {
            name: "update_fact_declaration".to_string(),
            description: "Update an existing fact declaration. Only provided fields will be changed.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the declaration to update"
                    },
                    "args": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "type": { "type": "string" },
                                "description": { "type": "string" }
                            },
                            "required": ["name"]
                        },
                        "description": "New argument definitions (replaces existing)"
                    },
                    "description": {
                        "type": "string",
                        "description": "New description"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New tags (replaces existing)"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "delete_fact_declaration".to_string(),
            description: "Delete a fact declaration by its record key.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the declaration to delete"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "list_fact_declarations".to_string(),
            description: "List all fact declarations, optionally filtered by tag.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tag": {
                        "type": "string",
                        "description": "Filter by tag (optional)"
                    },
                    "predicate": {
                        "type": "string",
                        "description": "Filter by predicate name (case-insensitive substring)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of declarations to return"
                    }
                }
            }),
        },
    ]
}

/// Get all fact declaration tools with their permission metadata.
/// All fact declaration tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

pub async fn create_fact_declaration(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let predicate = match arguments.get("predicate").and_then(|v| v.as_str()) {
        Some(p) => {
            if p.len() > 64 {
                return CallToolResult::error(format!(
                    "Predicate name too long: {} chars (max 64)",
                    p.len()
                ));
            }
            p.to_string()
        }
        None => return CallToolResult::error("Missing required parameter: predicate"),
    };

    let args = match arguments.get("args").and_then(|v| v.as_array()) {
        Some(arr) => {
            if arr.len() > 10 {
                return CallToolResult::error(format!(
                    "Too many arguments: {} (max 10)",
                    arr.len()
                ));
            }
            match parse_args(arr) {
                Ok(args) => args,
                Err(e) => return e,
            }
        }
        None => return CallToolResult::error("Missing required parameter: args"),
    };

    let description = match arguments.get("description").and_then(|v| v.as_str()) {
        Some(d) => truncate_chars(d, 1024),
        None => return CallToolResult::error("Missing required parameter: description"),
    };

    let tags: Vec<String> = arguments
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .take(20)
                .map(|s| truncate_chars(&s, 64))
                .collect()
        })
        .unwrap_or_default();

    let declaration = FactDeclaration {
        predicate: predicate.clone(),
        args,
        description,
        tags,
        created_at: Utc::now(),
        last_updated: None,
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(DECLARATION_COLLECTION, Some(&rkey), &declaration)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_declaration(rkey.clone(), declaration.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "predicate": predicate,
                    "arity": declaration.args.len()
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to create fact declaration: {}", e)),
    }
}

pub async fn create_fact_declarations(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let decls_array = match arguments.get("declarations").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return CallToolResult::error("Missing required parameter: declarations"),
    };

    if decls_array.is_empty() {
        return CallToolResult::error("declarations array cannot be empty");
    }

    // Validate and parse all declarations first
    let mut validated: Vec<(String, FactDeclaration)> = Vec::with_capacity(decls_array.len());
    let now = Utc::now();

    for (i, decl_val) in decls_array.iter().enumerate() {
        let obj = match decl_val.as_object() {
            Some(o) => o,
            None => return CallToolResult::error(format!("declarations[{}]: expected object", i)),
        };

        let predicate = match obj.get("predicate").and_then(|v| v.as_str()) {
            Some(p) => {
                if p.len() > 64 {
                    return CallToolResult::error(format!(
                        "declarations[{}]: predicate name too long: {} chars (max 64)",
                        i,
                        p.len()
                    ));
                }
                p.to_string()
            }
            None => {
                return CallToolResult::error(format!("declarations[{}]: missing predicate", i));
            }
        };

        let args = match obj.get("args").and_then(|v| v.as_array()) {
            Some(arr) => {
                if arr.len() > 10 {
                    return CallToolResult::error(format!(
                        "declarations[{}]: too many arguments: {} (max 10)",
                        i,
                        arr.len()
                    ));
                }
                match parse_args(arr) {
                    Ok(args) => args,
                    Err(e) => {
                        // Extract error message from CallToolResult
                        if let Some(crate::protocol::ToolContent::Text { text }) = e.content.first()
                        {
                            return CallToolResult::error(format!("declarations[{}].{}", i, text));
                        }
                        return CallToolResult::error(format!("declarations[{}]: invalid args", i));
                    }
                }
            }
            None => return CallToolResult::error(format!("declarations[{}]: missing args", i)),
        };

        let description = match obj.get("description").and_then(|v| v.as_str()) {
            Some(d) => truncate_chars(d, 1024),
            None => {
                return CallToolResult::error(format!("declarations[{}]: missing description", i));
            }
        };

        let tags: Vec<String> = obj
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .take(20)
                    .map(|s| truncate_chars(&s, 64))
                    .collect()
            })
            .unwrap_or_default();

        let declaration = FactDeclaration {
            predicate,
            args,
            description,
            tags,
            created_at: now,
            last_updated: None,
        };

        let rkey = Tid::now().to_string();
        validated.push((rkey, declaration));
    }

    // Build WriteOp list
    let writes: Vec<WriteOp> = validated
        .iter()
        .map(|(rkey, decl)| WriteOp::Create {
            collection: DECLARATION_COLLECTION.to_string(),
            rkey: rkey.clone(),
            value: serde_json::to_value(decl).unwrap(),
        })
        .collect();

    // Execute batch write
    match state.atproto.apply_writes(writes).await {
        Ok(response) => {
            // Update cache for each created record
            for ((rkey, decl), result) in validated.iter().zip(response.results.iter()) {
                if let WriteResult::Create { cid, .. } = result
                    && let Some(cache) = &state.cache
                {
                    cache.upsert_declaration(rkey.clone(), decl.clone(), cid.clone());
                }
            }

            let results: Vec<Value> = validated
                .iter()
                .zip(response.results.iter())
                .map(|((rkey, decl), result)| {
                    if let WriteResult::Create { uri, cid } = result {
                        json!({
                            "rkey": rkey,
                            "uri": uri,
                            "cid": cid,
                            "predicate": decl.predicate,
                            "arity": decl.args.len()
                        })
                    } else {
                        json!({ "rkey": rkey, "error": "unexpected result type" })
                    }
                })
                .collect();

            CallToolResult::success(
                json!({
                    "created": validated.len(),
                    "results": results
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Batch write failed: {}", e)),
    }
}

pub async fn update_fact_declaration(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    // Get the existing declaration
    let mut declaration = match state
        .atproto
        .get_record::<FactDeclaration>(DECLARATION_COLLECTION, rkey)
        .await
    {
        Ok(record) => record.value,
        Err(winter_atproto::AtprotoError::NotFound { .. }) => {
            return CallToolResult::error(format!("Fact declaration not found: {}", rkey));
        }
        Err(e) => return CallToolResult::error(format!("Failed to get fact declaration: {}", e)),
    };

    let mut changes = Vec::new();

    // Update args if provided
    if let Some(arr) = arguments.get("args").and_then(|v| v.as_array()) {
        if arr.len() > 10 {
            return CallToolResult::error(format!("Too many arguments: {} (max 10)", arr.len()));
        }
        match parse_args(arr) {
            Ok(args) => {
                declaration.args = args;
                changes.push("args");
            }
            Err(e) => return e,
        }
    }

    // Update description if provided
    if let Some(desc) = arguments.get("description").and_then(|v| v.as_str()) {
        declaration.description = truncate_chars(desc, 1024);
        changes.push("description");
    }

    // Update tags if provided
    if let Some(tags) = arguments.get("tags").and_then(|v| v.as_array()) {
        declaration.tags = tags
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .take(20)
            .map(|s| truncate_chars(&s, 64))
            .collect();
        changes.push("tags");
    }

    if changes.is_empty() {
        return CallToolResult::error("No changes specified");
    }

    // Update the last_updated timestamp
    declaration.last_updated = Some(Utc::now());

    // Save the updated declaration
    match state
        .atproto
        .put_record(DECLARATION_COLLECTION, rkey, &declaration)
        .await
    {
        Ok(response) => {
            // Update cache with the modified declaration
            if let Some(cache) = &state.cache {
                cache.upsert_declaration(
                    rkey.to_string(),
                    declaration.clone(),
                    response.cid.clone(),
                );
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "changes": changes,
                    "predicate": declaration.predicate
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to update fact declaration: {}", e)),
    }
}

pub async fn delete_fact_declaration(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state
        .atproto
        .delete_record(DECLARATION_COLLECTION, rkey)
        .await
    {
        Ok(()) => {
            // Remove from cache
            if let Some(cache) = &state.cache {
                cache.delete_declaration(rkey);
            }
            CallToolResult::success(
                json!({
                    "deleted": true,
                    "rkey": rkey
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to delete fact declaration: {}", e)),
    }
}

pub async fn list_fact_declarations(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let tag_filter = arguments.get("tag").and_then(|v| v.as_str());
    let predicate_filter = arguments.get("predicate").and_then(|v| v.as_str());
    let limit = arguments.get("limit").and_then(|v| v.as_u64());

    // List all declarations
    let declarations = match state
        .atproto
        .list_all_records::<FactDeclaration>(DECLARATION_COLLECTION)
        .await
    {
        Ok(records) => records,
        Err(e) => return CallToolResult::error(format!("Failed to list fact declarations: {}", e)),
    };

    // Filter and format
    let filtered: Vec<Value> = declarations
        .into_iter()
        .filter(|r| {
            // Filter by tag if specified
            if let Some(tag) = tag_filter
                && !r.value.tags.contains(&tag.to_string())
            {
                return false;
            }
            // Filter by predicate name (case-insensitive substring)
            if let Some(pred) = predicate_filter
                && !r
                    .value
                    .predicate
                    .to_lowercase()
                    .contains(&pred.to_lowercase())
            {
                return false;
            }
            true
        })
        .take(limit.unwrap_or(usize::MAX as u64) as usize)
        .map(|r| {
            // Extract rkey from URI (at://did/collection/rkey)
            let rkey = AtUri::extract_rkey(&r.uri).to_string();
            json!({
                "rkey": rkey,
                "predicate": r.value.predicate,
                "args": r.value.args.iter().map(|a| {
                    json!({
                        "name": a.name,
                        "type": a.r#type,
                        "description": a.description
                    })
                }).collect::<Vec<_>>(),
                "description": r.value.description,
                "tags": r.value.tags,
                "created_at": r.value.created_at.to_rfc3339()
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "declarations": filtered,
            "count": filtered.len()
        })
        .to_string(),
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_valid() {
        let arr = vec![
            json!({"name": "arg1", "type": "symbol", "description": "First arg"}),
            json!({"name": "arg2"}),
        ];
        let result = parse_args(&arr);
        assert!(result.is_ok());
        let args = result.unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "arg1");
        assert_eq!(args[0].r#type, "symbol");
        assert_eq!(args[0].description, Some("First arg".to_string()));
        assert_eq!(args[1].name, "arg2");
        assert_eq!(args[1].r#type, "symbol"); // default
        assert_eq!(args[1].description, None);
    }

    #[test]
    fn test_parse_args_missing_name() {
        let arr = vec![json!({"type": "symbol"})];
        let result = parse_args(&arr);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_args_not_object() {
        let arr = vec![json!("not an object")];
        let result = parse_args(&arr);
        assert!(result.is_err());
    }
}
