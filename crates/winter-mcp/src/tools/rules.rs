//! Rule tools for MCP.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Rule, Tid, WriteOp, WriteResult};

use super::{MAX_BATCH_SIZE, ToolMeta, ToolState, parse_string_array};

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
            name: "create_rules".to_string(),
            description: "Create multiple datalog rules in a single atomic transaction. All rules are created together or none are.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rules": {
                        "type": "array",
                        "items": {
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
                                    "description": "The rule head (derived predicate)"
                                },
                                "body": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "The rule body (conditions)"
                                },
                                "constraints": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Optional constraints"
                                },
                                "priority": {
                                    "type": "integer",
                                    "description": "Rule priority (default 0)"
                                }
                            },
                            "required": ["name", "description", "head", "body"]
                        },
                        "description": "Array of rules to create"
                    }
                },
                "required": ["rules"]
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
                    },
                    "name": {
                        "type": "string",
                        "description": "Filter by rule name (case-insensitive substring)"
                    },
                    "head": {
                        "type": "string",
                        "description": "Filter by head predicate name (case-insensitive substring)"
                    },
                    "body": {
                        "type": "string",
                        "description": "Filter by predicates in body (case-insensitive substring, matches any body item)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of rules to return"
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

/// Get all rule tools with their permission metadata.
/// All rule tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
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
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_rule(rkey.clone(), rule.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "name": name,
                    "head": head,
                    "body": rule.body
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to create rule: {}", e)),
    }
}

pub async fn create_rules(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let rules_array = match arguments.get("rules").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return CallToolResult::error("Missing required parameter: rules"),
    };

    if rules_array.is_empty() {
        return CallToolResult::error("rules array cannot be empty");
    }

    if rules_array.len() > MAX_BATCH_SIZE {
        return CallToolResult::error(format!(
            "Batch size {} exceeds maximum of {}",
            rules_array.len(),
            MAX_BATCH_SIZE
        ));
    }

    // Validate and parse all rules first
    let mut validated: Vec<(String, Rule)> = Vec::with_capacity(rules_array.len());
    let now = Utc::now();

    for (i, rule_val) in rules_array.iter().enumerate() {
        let obj = match rule_val.as_object() {
            Some(o) => o,
            None => return CallToolResult::error(format!("rules[{}]: expected object", i)),
        };

        let name = match obj.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return CallToolResult::error(format!("rules[{}]: missing name", i)),
        };

        let description = match obj.get("description").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => return CallToolResult::error(format!("rules[{}]: missing description", i)),
        };

        let head = match obj.get("head").and_then(|v| v.as_str()) {
            Some(h) => h,
            None => return CallToolResult::error(format!("rules[{}]: missing head", i)),
        };

        let body: Vec<String> = match obj.get("body").and_then(|v| v.as_array()) {
            Some(b) => match parse_string_array(b, &format!("rules[{}].body", i)) {
                Ok(arr) => arr,
                Err(e) => return e,
            },
            None => return CallToolResult::error(format!("rules[{}]: missing body", i)),
        };

        let constraints: Vec<String> = match obj.get("constraints").and_then(|v| v.as_array()) {
            Some(a) => match parse_string_array(a, &format!("rules[{}].constraints", i)) {
                Ok(arr) => arr,
                Err(e) => return e,
            },
            None => Vec::new(),
        };

        let priority = obj.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        let rule = Rule {
            name: name.to_string(),
            description: description.to_string(),
            head: head.to_string(),
            body,
            constraints,
            enabled: true,
            priority,
            created_at: now,
        };

        let rkey = Tid::now().to_string();
        validated.push((rkey, rule));
    }

    // Build WriteOp list
    let writes: Vec<WriteOp> = validated
        .iter()
        .map(|(rkey, rule)| WriteOp::Create {
            collection: RULE_COLLECTION.to_string(),
            rkey: rkey.clone(),
            value: serde_json::to_value(rule).unwrap(),
        })
        .collect();

    // Execute batch write
    match state.atproto.apply_writes(writes).await {
        Ok(response) => {
            // Update cache for each created record
            for ((rkey, rule), result) in validated.iter().zip(response.results.iter()) {
                if let WriteResult::Create { cid, .. } = result
                    && let Some(cache) = &state.cache
                {
                    cache.upsert_rule(rkey.clone(), rule.clone(), cid.clone());
                }
            }

            let results: Vec<Value> = validated
                .iter()
                .zip(response.results.iter())
                .map(|((rkey, rule), result)| {
                    if let WriteResult::Create { uri, cid } = result {
                        json!({
                            "rkey": rkey,
                            "uri": uri,
                            "cid": cid,
                            "name": rule.name,
                            "head": rule.head
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

pub async fn list_rules(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let enabled_only = arguments
        .get("enabled_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let name_filter = arguments.get("name").and_then(|v| v.as_str());
    let head_filter = arguments.get("head").and_then(|v| v.as_str());
    let body_filter = arguments.get("body").and_then(|v| v.as_str());
    let limit = arguments.get("limit").and_then(|v| v.as_u64());

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
        .filter(|item| {
            // Filter by enabled status
            if enabled_only && !item.value.enabled {
                return false;
            }
            // Filter by name (case-insensitive substring)
            if let Some(name) = name_filter
                && !item
                    .value
                    .name
                    .to_lowercase()
                    .contains(&name.to_lowercase())
            {
                return false;
            }
            // Filter by head predicate (case-insensitive substring)
            if let Some(head) = head_filter
                && !item
                    .value
                    .head
                    .to_lowercase()
                    .contains(&head.to_lowercase())
            {
                return false;
            }
            // Filter by body (case-insensitive substring, matches any body item)
            if let Some(body) = body_filter {
                let body_lower = body.to_lowercase();
                if !item
                    .value
                    .body
                    .iter()
                    .any(|b| b.to_lowercase().contains(&body_lower))
                {
                    return false;
                }
            }
            true
        })
        .take(limit.unwrap_or(usize::MAX as u64) as usize)
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
        Ok(response) => {
            // Update cache with the modified rule
            if let Some(cache) = &state.cache {
                cache.upsert_rule(rkey.to_string(), rule.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "enabled": enabled,
                    "name": rule.name
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to toggle rule: {}", e)),
    }
}
