//! Rule tools for MCP.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Rule, Tid};

use super::ToolState;

/// Collection name for rules.
const RULE_COLLECTION: &str = "diy.razorgirl.winter.rule";

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "create_rule".to_string(),
            description: "Create a new datalog rule. Rules derive new facts from existing ones."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name for the rule (for reference)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Human-readable description of what this rule derives"
                    },
                    "head": {
                        "type": "string",
                        "description": "The rule head (derived predicate), e.g., 'mutual_follow(X, Y)'"
                    },
                    "body": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The rule body (conditions), e.g., ['follows(X, Y)', 'follows(Y, X)']"
                    },
                    "constraints": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional constraints, e.g., ['X != Y']"
                    },
                    "priority": {
                        "type": "integer",
                        "description": "Rule priority (lower = evaluated earlier, default 0)"
                    }
                },
                "required": ["name", "description", "head", "body"]
            }),
        },
        ToolDefinition {
            name: "list_rules".to_string(),
            description: "List all stored datalog rules.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "enabled_only": {
                        "type": "boolean",
                        "description": "Only show enabled rules (default true)"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "toggle_rule".to_string(),
            description: "Enable or disable a rule.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the rule"
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "Whether the rule should be enabled"
                    }
                },
                "required": ["rkey", "enabled"]
            }),
        },
    ]
}

pub async fn create_rule(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    let description = match arguments.get("description").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: description"),
    };

    let head = match arguments.get("head").and_then(|v| v.as_str()) {
        Some(h) => h,
        None => return CallToolResult::error("Missing required parameter: head"),
    };

    let body: Vec<String> = match arguments.get("body").and_then(|v| v.as_array()) {
        Some(b) => match parse_string_array(b, "body") {
            Ok(arr) => arr,
            Err(e) => return e,
        },
        None => return CallToolResult::error("Missing required parameter: body"),
    };

    let constraints: Vec<String> = match arguments.get("constraints").and_then(|v| v.as_array()) {
        Some(a) => match parse_string_array(a, "constraints") {
            Ok(arr) => arr,
            Err(e) => return e,
        },
        None => Vec::new(),
    };

    let priority = arguments
        .get("priority")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    let rule = Rule {
        name: name.to_string(),
        description: description.to_string(),
        head: head.to_string(),
        body,
        constraints,
        enabled: true,
        priority,
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(RULE_COLLECTION, Some(&rkey), &rule)
        .await
    {
        Ok(response) => CallToolResult::success(
            json!({
                "rkey": rkey,
                "uri": response.uri,
                "cid": response.cid,
                "name": name,
                "head": head,
                "body": rule.body
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to create rule: {}", e)),
    }
}

pub async fn list_rules(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let enabled_only = arguments
        .get("enabled_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Try cache first, fall back to HTTP
    let rules = if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            tracing::debug!("using cache for list_rules");
            cache
                .list_rules()
                .into_iter()
                .map(|(rkey, cached)| winter_atproto::ListRecordItem {
                    uri: format!("at://did/{}:{}", RULE_COLLECTION, rkey),
                    cid: cached.cid,
                    value: cached.value,
                })
                .collect()
        } else {
            match state
                .atproto
                .list_all_records::<Rule>(RULE_COLLECTION)
                .await
            {
                Ok(records) => records,
                Err(e) => return CallToolResult::error(format!("Failed to list rules: {}", e)),
            }
        }
    } else {
        match state
            .atproto
            .list_all_records::<Rule>(RULE_COLLECTION)
            .await
        {
            Ok(records) => records,
            Err(e) => return CallToolResult::error(format!("Failed to list rules: {}", e)),
        }
    };

    let formatted: Vec<Value> = rules
        .into_iter()
        .filter(|item| !enabled_only || item.value.enabled)
        .map(|item| {
            let rkey = item.uri.split('/').next_back().unwrap_or("");
            let rule_str = format!(
                "{} :- {}{}.",
                item.value.head,
                item.value.body.join(", "),
                if item.value.constraints.is_empty() {
                    String::new()
                } else {
                    format!(", {}", item.value.constraints.join(", "))
                }
            );
            json!({
                "rkey": rkey,
                "name": item.value.name,
                "description": item.value.description,
                "rule": rule_str,
                "enabled": item.value.enabled,
                "priority": item.value.priority
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "count": formatted.len(),
            "rules": formatted
        })
        .to_string(),
    )
}

/// Parse a JSON array into a Vec<String>, returning an error if any element is not a string.
fn parse_string_array(arr: &[Value], field_name: &str) -> Result<Vec<String>, CallToolResult> {
    let mut result = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        match v.as_str() {
            Some(s) => result.push(s.to_string()),
            None => {
                let type_name = match v {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    Value::Array(_) => "array",
                    Value::Object(_) => "object",
                    Value::String(_) => unreachable!(),
                };
                return Err(CallToolResult::error(format!(
                    "Invalid {}[{}]: expected string, got {}",
                    field_name, i, type_name
                )));
            }
        }
    }
    Ok(result)
}

pub async fn toggle_rule(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    let enabled = arguments
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Get the existing rule
    let mut rule = match state
        .atproto
        .get_record::<Rule>(RULE_COLLECTION, rkey)
        .await
    {
        Ok(record) => record.value,
        Err(e) => return CallToolResult::error(format!("Failed to get rule: {}", e)),
    };

    // Update the enabled status
    rule.enabled = enabled;

    match state.atproto.put_record(RULE_COLLECTION, rkey, &rule).await {
        Ok(response) => CallToolResult::success(
            json!({
                "rkey": rkey,
                "uri": response.uri,
                "enabled": enabled,
                "name": rule.name
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to toggle rule: {}", e)),
    }
}
