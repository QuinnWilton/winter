//! Thought tools for MCP.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Thought, ThoughtKind, Tid};

use super::ToolState;

/// Collection name for thoughts.
const THOUGHT_COLLECTION: &str = "diy.razorgirl.winter.thought";

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "record_thought".to_string(),
            description: "Record a thought in your stream of consciousness. Thoughts are visible in your web UI for transparency.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["insight", "question", "plan", "reflection", "error"],
                        "description": "The kind of thought: insight (something noticed/understood), question (uncertainty), plan (intention), reflection (introspection), error (problem)"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content of the thought"
                    },
                    "trigger": {
                        "type": "string",
                        "description": "What triggered this thought (optional)"
                    }
                },
                "required": ["kind", "content"]
            }),
        },
        ToolDefinition {
            name: "list_thoughts".to_string(),
            description: "List recent thoughts with optional filtering by kind.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["insight", "question", "plan", "reflection", "error", "response", "tool_call"],
                        "description": "Filter by thought kind"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum thoughts to return (default 20)"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "get_thought".to_string(),
            description: "Get a thought by its record key.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the thought"
                    }
                },
                "required": ["rkey"]
            }),
        },
    ]
}

pub async fn record_thought(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let kind_str = match arguments.get("kind").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return CallToolResult::error("Missing required parameter: kind"),
    };

    let content = match arguments.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: content"),
    };

    let trigger = arguments
        .get("trigger")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Parse the kind
    let kind = match kind_str {
        "insight" => ThoughtKind::Insight,
        "question" => ThoughtKind::Question,
        "plan" => ThoughtKind::Plan,
        "reflection" => ThoughtKind::Reflection,
        "error" => ThoughtKind::Error,
        _ => {
            return CallToolResult::error(format!(
                "Invalid kind '{}'. Must be one of: insight, question, plan, reflection, error",
                kind_str
            ));
        }
    };

    let thought = Thought {
        kind,
        content: content.to_string(),
        trigger,
        duration_ms: None,
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(THOUGHT_COLLECTION, Some(&rkey), &thought)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_thought(rkey.clone(), thought.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "kind": kind_str,
                    "content": content
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to record thought: {}", e)),
    }
}

fn thought_kind_to_str(kind: &ThoughtKind) -> &'static str {
    match kind {
        ThoughtKind::Insight => "insight",
        ThoughtKind::Question => "question",
        ThoughtKind::Plan => "plan",
        ThoughtKind::Reflection => "reflection",
        ThoughtKind::Error => "error",
        ThoughtKind::Response => "response",
        ThoughtKind::ToolCall => "tool_call",
    }
}

pub async fn list_thoughts(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let kind_filter = arguments.get("kind").and_then(|v| v.as_str());
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;

    // Try cache first, fall back to HTTP
    let thoughts = if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            tracing::debug!("using cache for list_thoughts");
            cache
                .list_thoughts()
                .into_iter()
                .map(|(rkey, cached)| winter_atproto::ListRecordItem {
                    uri: format!("at://did/{}:{}", THOUGHT_COLLECTION, rkey),
                    cid: cached.cid,
                    value: cached.value,
                })
                .collect()
        } else {
            match state
                .atproto
                .list_all_records::<Thought>(THOUGHT_COLLECTION)
                .await
            {
                Ok(records) => records,
                Err(e) => return CallToolResult::error(format!("Failed to list thoughts: {}", e)),
            }
        }
    } else {
        match state
            .atproto
            .list_all_records::<Thought>(THOUGHT_COLLECTION)
            .await
        {
            Ok(records) => records,
            Err(e) => return CallToolResult::error(format!("Failed to list thoughts: {}", e)),
        }
    };

    let formatted: Vec<serde_json::Value> = thoughts
        .into_iter()
        .filter(|item| {
            if let Some(filter) = kind_filter {
                thought_kind_to_str(&item.value.kind) == filter
            } else {
                true
            }
        })
        .take(limit)
        .map(|item| {
            let rkey = item.uri.split('/').next_back().unwrap_or("");
            // Truncate content for listing
            let preview = if item.value.content.len() > 200 {
                format!("{}...", &item.value.content[..200])
            } else {
                item.value.content.clone()
            };
            json!({
                "rkey": rkey,
                "kind": thought_kind_to_str(&item.value.kind),
                "content": preview,
                "trigger": item.value.trigger,
                "created_at": item.value.created_at.to_rfc3339()
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "count": formatted.len(),
            "thoughts": formatted
        })
        .to_string(),
    )
}

pub async fn get_thought(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state
        .atproto
        .get_record::<Thought>(THOUGHT_COLLECTION, rkey)
        .await
    {
        Ok(record) => {
            let thought = record.value;
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "kind": thought_kind_to_str(&thought.kind),
                    "content": thought.content,
                    "trigger": thought.trigger,
                    "duration_ms": thought.duration_ms,
                    "created_at": thought.created_at.to_rfc3339()
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to get thought: {}", e)),
    }
}
