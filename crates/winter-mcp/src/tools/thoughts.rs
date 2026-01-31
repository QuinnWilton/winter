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
    vec![ToolDefinition {
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
    }]
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
        Ok(response) => CallToolResult::success(
            json!({
                "rkey": rkey,
                "uri": response.uri,
                "cid": response.cid,
                "kind": kind_str,
                "content": content
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to record thought: {}", e)),
    }
}
