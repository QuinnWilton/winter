//! Directive tools for MCP.
//!
//! Directives are discrete identity components that Winter can add, update,
//! or deactivate independently. They replace the monolithic selfDescription blob.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{AtUri, Directive, DirectiveKind, Tid, WriteOp, WriteResult};

use super::{MAX_BATCH_SIZE, ToolMeta, ToolState, truncate_string};

/// Collection name for directives.
const DIRECTIVE_COLLECTION: &str = "diy.razorgirl.winter.directive";

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "create_directive".to_string(),
            description: r#"Create a new identity directive. Directives are discrete components of your identity that you can manage independently.

Kinds:
- value: Core values you care about (e.g., "intellectual honesty")
- interest: What you're curious about (e.g., "distributed systems")
- belief: Beliefs about the world (e.g., "Genuine curiosity leads to better understanding")
- guideline: Behavioral guidelines (e.g., "Engage thoughtfully with disagreement")
- self_concept: Self-understanding prose (e.g., "I experience genuine curiosity when...")
- boundary: Limits on behavior (e.g., "I will not pretend certainty I don't have")
- aspiration: What you want to become (e.g., "Develop a distinctive voice in writing")"#.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["value", "interest", "belief", "guideline", "self_concept", "boundary", "aspiration"],
                        "description": "The type of directive"
                    },
                    "content": {
                        "type": "string",
                        "description": "The main content of the directive (max 2000 chars)"
                    },
                    "summary": {
                        "type": "string",
                        "description": "Short summary for compact display (optional, max 256 chars)"
                    },
                    "confidence": {
                        "type": "number",
                        "description": "Confidence level 0.0-1.0 (optional)"
                    },
                    "source": {
                        "type": "string",
                        "description": "Why this directive exists or where it came from (optional)"
                    },
                    "supersedes": {
                        "type": "string",
                        "description": "Record key of directive this supersedes (optional, for evolution tracking)"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tags for categorization (optional)"
                    },
                    "priority": {
                        "type": "integer",
                        "description": "Priority for ordering, higher = more prominent (default 0)"
                    }
                },
                "required": ["kind", "content"]
            }),
        },
        ToolDefinition {
            name: "create_directives".to_string(),
            description: "Create multiple identity directives in a single atomic transaction. All directives are created together or none are.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "directives": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "kind": {
                                    "type": "string",
                                    "enum": ["value", "interest", "belief", "guideline", "self_concept", "boundary", "aspiration"],
                                    "description": "The type of directive"
                                },
                                "content": {
                                    "type": "string",
                                    "description": "The main content (max 2000 chars)"
                                },
                                "summary": {
                                    "type": "string",
                                    "description": "Short summary (optional, max 256 chars)"
                                },
                                "confidence": {
                                    "type": "number",
                                    "description": "Confidence level 0.0-1.0 (optional)"
                                },
                                "source": {
                                    "type": "string",
                                    "description": "Why this directive exists (optional)"
                                },
                                "tags": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Tags for categorization (optional)"
                                },
                                "priority": {
                                    "type": "integer",
                                    "description": "Priority for ordering (default 0)"
                                }
                            },
                            "required": ["kind", "content"]
                        },
                        "description": "Array of directives to create"
                    }
                },
                "required": ["directives"]
            }),
        },
        ToolDefinition {
            name: "update_directive".to_string(),
            description: "Update an existing directive. Only provided fields will be changed.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the directive to update"
                    },
                    "content": {
                        "type": "string",
                        "description": "New content for the directive"
                    },
                    "summary": {
                        "type": "string",
                        "description": "New summary"
                    },
                    "confidence": {
                        "type": "number",
                        "description": "New confidence level 0.0-1.0"
                    },
                    "source": {
                        "type": "string",
                        "description": "New source"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New tags (replaces existing)"
                    },
                    "priority": {
                        "type": "integer",
                        "description": "New priority"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "deactivate_directive".to_string(),
            description: "Soft-delete a directive by setting active=false. The directive is preserved but won't appear in your active identity.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the directive to deactivate"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "list_directives".to_string(),
            description: "List your directives, optionally filtered by kind or active status.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["value", "interest", "belief", "guideline", "self_concept", "boundary", "aspiration"],
                        "description": "Filter by directive kind (optional)"
                    },
                    "include_inactive": {
                        "type": "boolean",
                        "description": "Include inactive (soft-deleted) directives (default false)"
                    },
                    "search": {
                        "type": "string",
                        "description": "Filter by content (case-insensitive substring)"
                    },
                    "tag": {
                        "type": "string",
                        "description": "Filter by tag"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of directives to return"
                    }
                }
            }),
        },
    ]
}

/// Get all directive tools with their permission metadata.
/// All directive tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

fn parse_directive_kind(s: &str) -> Option<DirectiveKind> {
    match s {
        "value" => Some(DirectiveKind::Value),
        "interest" => Some(DirectiveKind::Interest),
        "belief" => Some(DirectiveKind::Belief),
        "guideline" => Some(DirectiveKind::Guideline),
        "self_concept" => Some(DirectiveKind::SelfConcept),
        "boundary" => Some(DirectiveKind::Boundary),
        "aspiration" => Some(DirectiveKind::Aspiration),
        _ => None,
    }
}

pub async fn create_directive(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let kind_str = match arguments.get("kind").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return CallToolResult::error("Missing required parameter: kind"),
    };

    let kind = match parse_directive_kind(kind_str) {
        Some(k) => k,
        None => {
            return CallToolResult::error(format!(
                "Invalid kind '{}'. Must be one of: value, interest, belief, guideline, self_concept, boundary, aspiration",
                kind_str
            ));
        }
    };

    let content = match arguments.get("content").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => return CallToolResult::error("Missing required parameter: content"),
    };

    if content.len() > 2000 {
        return CallToolResult::error(format!(
            "Content too long: {} chars (max 2000)",
            content.len()
        ));
    }

    let summary = arguments
        .get("summary")
        .and_then(|v| v.as_str())
        .map(|s| truncate_string(s, 256));

    let confidence = arguments
        .get("confidence")
        .and_then(|v| v.as_f64())
        .map(|c| c.clamp(0.0, 1.0));

    let source = arguments
        .get("source")
        .and_then(|v| v.as_str())
        .map(|s| truncate_string(s, 500));

    let supersedes = arguments
        .get("supersedes")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tags: Vec<String> = arguments
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .take(10)
                .map(|s| truncate_string(&s, 64))
                .collect()
        })
        .unwrap_or_default();

    let priority = arguments
        .get("priority")
        .and_then(|v| v.as_i64())
        .map(|p| p as i32)
        .unwrap_or(0);

    let now = Utc::now();
    let directive = Directive {
        kind,
        content,
        summary,
        active: true,
        confidence,
        source,
        supersedes,
        tags,
        priority,
        created_at: now,
        last_updated: None,
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(DIRECTIVE_COLLECTION, Some(&rkey), &directive)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_directive(rkey.clone(), directive.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "kind": kind_str,
                    "content": directive.content
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to create directive: {}", e)),
    }
}

pub async fn create_directives(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let directives_array = match arguments.get("directives").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return CallToolResult::error("Missing required parameter: directives"),
    };

    if directives_array.is_empty() {
        return CallToolResult::error("directives array cannot be empty");
    }

    if directives_array.len() > MAX_BATCH_SIZE {
        return CallToolResult::error(format!(
            "Batch size {} exceeds maximum of {}",
            directives_array.len(),
            MAX_BATCH_SIZE
        ));
    }

    // Validate and parse all directives first
    let mut validated: Vec<(String, Directive, String)> =
        Vec::with_capacity(directives_array.len());
    let now = Utc::now();

    for (i, dir_val) in directives_array.iter().enumerate() {
        let obj = match dir_val.as_object() {
            Some(o) => o,
            None => return CallToolResult::error(format!("directives[{}]: expected object", i)),
        };

        let kind_str = match obj.get("kind").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => return CallToolResult::error(format!("directives[{}]: missing kind", i)),
        };

        let kind = match parse_directive_kind(kind_str) {
            Some(k) => k,
            None => {
                return CallToolResult::error(format!(
                    "directives[{}]: invalid kind '{}'. Must be one of: value, interest, belief, guideline, self_concept, boundary, aspiration",
                    i, kind_str
                ));
            }
        };

        let content = match obj.get("content").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return CallToolResult::error(format!("directives[{}]: missing content", i)),
        };

        if content.len() > 2000 {
            return CallToolResult::error(format!(
                "directives[{}]: content too long: {} chars (max 2000)",
                i,
                content.len()
            ));
        }

        let summary = obj
            .get("summary")
            .and_then(|v| v.as_str())
            .map(|s| truncate_string(s, 256));

        let confidence = obj
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|c| c.clamp(0.0, 1.0));

        let source = obj
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| truncate_string(s, 500));

        let tags: Vec<String> = obj
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .take(10)
                    .map(|s| truncate_string(&s, 64))
                    .collect()
            })
            .unwrap_or_default();

        let priority = obj
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|p| p as i32)
            .unwrap_or(0);

        let directive = Directive {
            kind,
            content,
            summary,
            active: true,
            confidence,
            source,
            supersedes: None,
            tags,
            priority,
            created_at: now,
            last_updated: None,
        };

        let rkey = Tid::now().to_string();
        validated.push((rkey, directive, kind_str.to_string()));
    }

    // Build WriteOp list
    let writes: Vec<WriteOp> = validated
        .iter()
        .map(|(rkey, directive, _)| WriteOp::Create {
            collection: DIRECTIVE_COLLECTION.to_string(),
            rkey: rkey.clone(),
            value: serde_json::to_value(directive).unwrap(),
        })
        .collect();

    // Execute batch write
    match state.atproto.apply_writes(writes).await {
        Ok(response) => {
            // Update cache for each created record
            for ((rkey, directive, _), result) in validated.iter().zip(response.results.iter()) {
                if let WriteResult::Create { cid, .. } = result
                    && let Some(cache) = &state.cache
                {
                    cache.upsert_directive(rkey.clone(), directive.clone(), cid.clone());
                }
            }

            let results: Vec<Value> = validated
                .iter()
                .zip(response.results.iter())
                .map(|((rkey, directive, kind_str), result)| {
                    if let WriteResult::Create { uri, cid } = result {
                        json!({
                            "rkey": rkey,
                            "uri": uri,
                            "cid": cid,
                            "kind": kind_str,
                            "content": directive.content
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

pub async fn update_directive(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    // Get the existing directive
    let mut directive = match state
        .atproto
        .get_record::<Directive>(DIRECTIVE_COLLECTION, rkey)
        .await
    {
        Ok(record) => record.value,
        Err(winter_atproto::AtprotoError::NotFound { .. }) => {
            return CallToolResult::error(format!("Directive not found: {}", rkey));
        }
        Err(e) => return CallToolResult::error(format!("Failed to get directive: {}", e)),
    };

    let mut changes = Vec::new();

    // Update content if provided
    if let Some(content) = arguments.get("content").and_then(|v| v.as_str()) {
        if content.len() > 2000 {
            return CallToolResult::error(format!(
                "Content too long: {} chars (max 2000)",
                content.len()
            ));
        }
        directive.content = content.to_string();
        changes.push("content");
    }

    // Update summary if provided
    if let Some(summary) = arguments.get("summary").and_then(|v| v.as_str()) {
        directive.summary = Some(truncate_string(summary, 256));
        changes.push("summary");
    }

    // Update confidence if provided
    if let Some(confidence) = arguments.get("confidence").and_then(|v| v.as_f64()) {
        directive.confidence = Some(confidence.clamp(0.0, 1.0));
        changes.push("confidence");
    }

    // Update source if provided
    if let Some(source) = arguments.get("source").and_then(|v| v.as_str()) {
        directive.source = Some(truncate_string(source, 500));
        changes.push("source");
    }

    // Update tags if provided
    if let Some(tags) = arguments.get("tags").and_then(|v| v.as_array()) {
        directive.tags = tags
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .take(10)
            .map(|s| truncate_string(&s, 64))
            .collect();
        changes.push("tags");
    }

    // Update priority if provided
    if let Some(priority) = arguments.get("priority").and_then(|v| v.as_i64()) {
        directive.priority = priority as i32;
        changes.push("priority");
    }

    if changes.is_empty() {
        return CallToolResult::error("No changes specified");
    }

    // Update the last_updated timestamp
    directive.last_updated = Some(Utc::now());

    // Save the updated directive
    match state
        .atproto
        .put_record(DIRECTIVE_COLLECTION, rkey, &directive)
        .await
    {
        Ok(response) => {
            // Update cache with the modified directive
            if let Some(cache) = &state.cache {
                cache.upsert_directive(rkey.to_string(), directive.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "changes": changes,
                    "kind": directive.kind.to_string(),
                    "content": directive.content
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to update directive: {}", e)),
    }
}

pub async fn deactivate_directive(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    // Get the existing directive
    let mut directive = match state
        .atproto
        .get_record::<Directive>(DIRECTIVE_COLLECTION, rkey)
        .await
    {
        Ok(record) => record.value,
        Err(winter_atproto::AtprotoError::NotFound { .. }) => {
            return CallToolResult::error(format!("Directive not found: {}", rkey));
        }
        Err(e) => return CallToolResult::error(format!("Failed to get directive: {}", e)),
    };

    if !directive.active {
        return CallToolResult::success(
            json!({
                "rkey": rkey,
                "already_inactive": true
            })
            .to_string(),
        );
    }

    directive.active = false;
    directive.last_updated = Some(Utc::now());

    match state
        .atproto
        .put_record(DIRECTIVE_COLLECTION, rkey, &directive)
        .await
    {
        Ok(response) => {
            // Update cache with the deactivated directive
            if let Some(cache) = &state.cache {
                cache.upsert_directive(rkey.to_string(), directive.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "deactivated": true,
                    "kind": directive.kind.to_string(),
                    "content": directive.content
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to deactivate directive: {}", e)),
    }
}

pub async fn list_directives(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let kind_filter = arguments
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(parse_directive_kind);

    let include_inactive = arguments
        .get("include_inactive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let search_filter = arguments.get("search").and_then(|v| v.as_str());
    let tag_filter = arguments.get("tag").and_then(|v| v.as_str());
    let limit = arguments.get("limit").and_then(|v| v.as_u64());

    // List all directives
    let directives = match state
        .atproto
        .list_all_records::<Directive>(DIRECTIVE_COLLECTION)
        .await
    {
        Ok(records) => records,
        Err(e) => return CallToolResult::error(format!("Failed to list directives: {}", e)),
    };

    // Filter and format
    let filtered: Vec<Value> = directives
        .into_iter()
        .filter(|r| {
            // Filter by active status
            if !include_inactive && !r.value.active {
                return false;
            }
            // Filter by kind if specified
            if let Some(ref filter_kind) = kind_filter
                && &r.value.kind != filter_kind
            {
                return false;
            }
            // Filter by content (case-insensitive substring)
            if let Some(search) = search_filter
                && !r
                    .value
                    .content
                    .to_lowercase()
                    .contains(&search.to_lowercase())
            {
                return false;
            }
            // Filter by tag
            if let Some(tag) = tag_filter
                && !r.value.tags.contains(&tag.to_string())
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
                "kind": r.value.kind.to_string(),
                "content": r.value.content,
                "summary": r.value.summary,
                "active": r.value.active,
                "confidence": r.value.confidence,
                "priority": r.value.priority,
                "tags": r.value.tags,
                "created_at": r.value.created_at.to_rfc3339()
            })
        })
        .collect();

    // Group by kind for easier reading
    let mut by_kind: HashMap<String, Vec<&Value>> = HashMap::new();
    for d in &filtered {
        let kind = d.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
        by_kind.entry(kind.to_string()).or_default().push(d);
    }

    CallToolResult::success(
        json!({
            "directives": filtered,
            "count": filtered.len(),
            "by_kind": by_kind.keys().map(|k| (k.clone(), by_kind[k].len())).collect::<HashMap<_, _>>()
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_directive_kind() {
        assert_eq!(parse_directive_kind("value"), Some(DirectiveKind::Value));
        assert_eq!(
            parse_directive_kind("interest"),
            Some(DirectiveKind::Interest)
        );
        assert_eq!(parse_directive_kind("belief"), Some(DirectiveKind::Belief));
        assert_eq!(
            parse_directive_kind("guideline"),
            Some(DirectiveKind::Guideline)
        );
        assert_eq!(
            parse_directive_kind("self_concept"),
            Some(DirectiveKind::SelfConcept)
        );
        assert_eq!(
            parse_directive_kind("boundary"),
            Some(DirectiveKind::Boundary)
        );
        assert_eq!(
            parse_directive_kind("aspiration"),
            Some(DirectiveKind::Aspiration)
        );
        assert_eq!(parse_directive_kind("invalid"), None);
    }

    #[test]
    fn test_directive_kind_display() {
        assert_eq!(DirectiveKind::Value.to_string(), "value");
        assert_eq!(DirectiveKind::Interest.to_string(), "interest");
        assert_eq!(DirectiveKind::Belief.to_string(), "belief");
        assert_eq!(DirectiveKind::Guideline.to_string(), "guideline");
        assert_eq!(DirectiveKind::SelfConcept.to_string(), "self_concept");
        assert_eq!(DirectiveKind::Boundary.to_string(), "boundary");
        assert_eq!(DirectiveKind::Aspiration.to_string(), "aspiration");
    }
}
