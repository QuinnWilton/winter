//! Trigger tools for MCP.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Tid, Trigger, TriggerAction};

use std::collections::HashSet;

use super::{ToolMeta, ToolState, parse_args};

/// Collection name for triggers.
const TRIGGER_COLLECTION: &str = "diy.razorgirl.winter.trigger";

/// Build a query and extra_rules from a trigger condition body.
///
/// Trigger conditions are rule bodies (e.g. `follows_me(X, _), !has_impression(X)`)
/// which can't be passed directly as queries. This wraps them into a rule:
///   `_trigger_result(X) :- follows_me(X, _), !has_impression(X).`
/// and returns `("_trigger_result(X)", Some("<rules>"))`.
fn build_trigger_query(condition: &str, condition_rules: Option<&str>) -> (String, Option<String>) {
    let vars = extract_variables(condition);

    let query = if vars.is_empty() {
        "_trigger_result()".to_string()
    } else {
        format!("_trigger_result({})", vars.join(", "))
    };

    let condition_trimmed = condition.trim().trim_end_matches('.');
    let wrapper_rule = if vars.is_empty() {
        format!("_trigger_result() :- {}.", condition_trimmed)
    } else {
        format!(
            "_trigger_result({}) :- {}.",
            vars.join(", "),
            condition_trimmed
        )
    };

    let rules = match condition_rules {
        Some(existing) => format!("{}\n{}", existing, wrapper_rule),
        None => wrapper_rule,
    };

    (query, Some(rules))
}

/// Extract unique uppercase variable names from a datalog condition body,
/// preserving first-seen order. Skips `_` (anonymous variable).
fn extract_variables(condition: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut vars = Vec::new();

    for token in condition.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if token.is_empty() || token == "_" {
            continue;
        }
        if let Some(first) = token.chars().next() {
            if first.is_uppercase()
                && token.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !seen.contains(token)
            {
                seen.insert(token.to_string());
                vars.push(token.to_string());
            }
        }
    }

    vars
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "create_trigger".to_string(),
            description: "Create a new datalog trigger. Triggers evaluate a datalog condition periodically and execute an action when new results appear. Supports $0, $1 variable substitution from query result columns.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Human-readable name for this trigger"
                    },
                    "description": {
                        "type": "string",
                        "description": "Description of what this trigger does"
                    },
                    "condition": {
                        "type": "string",
                        "description": "Datalog query that defines the trigger condition (e.g., 'follows(Self, X, _), !high_context(X, _)')"
                    },
                    "condition_rules": {
                        "type": "string",
                        "description": "Optional extra datalog rules for the condition query"
                    },
                    "action": {
                        "type": "object",
                        "description": "Action to perform when condition yields new results",
                        "properties": {
                            "type": {
                                "type": "string",
                                "enum": ["create_fact", "create_inbox_item", "delete_fact"],
                                "description": "Action type"
                            },
                            "predicate": {
                                "type": "string",
                                "description": "For create_fact: the predicate name"
                            },
                            "args": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "For create_fact: arguments (supports $0, $1 substitution)"
                            },
                            "tags": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "For create_fact: optional tags"
                            },
                            "message": {
                                "type": "string",
                                "description": "For create_inbox_item: message text (supports $0, $1 substitution)"
                            },
                            "rkey": {
                                "type": "string",
                                "description": "For delete_fact: rkey of fact to delete (supports $0 substitution)"
                            }
                        },
                        "required": ["type"]
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "Whether this trigger is enabled (default: true)",
                        "default": true
                    },
                    "args": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "Argument name" },
                                "type": { "type": "string", "description": "Soufflé type (symbol, number, unsigned, float). Default: symbol" },
                                "description": { "type": "string", "description": "What this argument represents" }
                            },
                            "required": ["name"]
                        },
                        "description": "Type annotations for _trigger_result predicate columns. Enables numeric comparisons instead of lexicographic string ordering."
                    }
                },
                "required": ["name", "description", "condition", "action"]
            }),
        },
        ToolDefinition {
            name: "update_trigger".to_string(),
            description: "Update an existing trigger. Fetches the current trigger, overlays provided fields, and saves.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the trigger to update"
                    },
                    "name": {
                        "type": "string",
                        "description": "New name"
                    },
                    "description": {
                        "type": "string",
                        "description": "New description"
                    },
                    "condition": {
                        "type": "string",
                        "description": "New condition query"
                    },
                    "condition_rules": {
                        "type": ["string", "null"],
                        "description": "New extra rules (null to clear)"
                    },
                    "action": {
                        "type": "object",
                        "description": "New action"
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "Enable or disable the trigger"
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
                        "description": "Type annotations for _trigger_result (replaces existing)"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "delete_trigger".to_string(),
            description: "Delete a trigger.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the trigger to delete"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "list_triggers".to_string(),
            description: "List all triggers, optionally filtered by enabled status.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "enabled": {
                        "type": "boolean",
                        "description": "Filter by enabled status (omit to show all)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "test_trigger".to_string(),
            description: "Dry-run a trigger condition to see what would fire. Accepts either an existing trigger rkey or an ad-hoc condition. Does not execute any actions.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of an existing trigger to test"
                    },
                    "condition": {
                        "type": "string",
                        "description": "Ad-hoc condition query to test (used if rkey not provided)"
                    },
                    "condition_rules": {
                        "type": "string",
                        "description": "Ad-hoc extra rules (used with ad-hoc condition)"
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
                        "description": "Type annotations for ad-hoc testing (used with ad-hoc condition)"
                    }
                },
                "required": []
            }),
        },
    ]
}

pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

pub async fn create_trigger(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };
    let description = match arguments.get("description").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: description"),
    };
    let condition = match arguments.get("condition").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: condition"),
    };
    let condition_rules = arguments
        .get("condition_rules")
        .and_then(|v| v.as_str())
        .map(String::from);
    let enabled = arguments
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let action = match arguments.get("action") {
        Some(v) => match parse_trigger_action(v) {
            Ok(a) => a,
            Err(e) => return CallToolResult::error(e),
        },
        None => return CallToolResult::error("Missing required parameter: action"),
    };

    let args = match arguments.get("args").and_then(|v| v.as_array()) {
        Some(arr) => match parse_args(arr) {
            Ok(a) => a,
            Err(e) => return e,
        },
        None => Vec::new(),
    };

    let atproto = &state.atproto;

    let trigger = Trigger {
        name: name.to_string(),
        description: description.to_string(),
        condition: condition.to_string(),
        condition_rules,
        action,
        enabled,
        args,
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();
    match atproto
        .create_record(TRIGGER_COLLECTION, Some(&rkey), &trigger)
        .await
    {
        Ok(_) => CallToolResult::success(
            json!({
                "rkey": rkey,
                "name": name,
                "enabled": enabled,
                "status": "created"
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to create trigger: {}", e)),
    }
}

pub async fn update_trigger(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    let cache = match state.cache.as_ref() {
        Some(c) => c,
        None => return CallToolResult::error("Cache not available"),
    };
    let atproto = &state.atproto;

    // Fetch existing trigger
    let existing = match cache.get_trigger(rkey) {
        Some(cached) => cached.value,
        None => return CallToolResult::error(format!("Trigger not found: {}", rkey)),
    };

    // Overlay provided fields
    let name = arguments
        .get("name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or(existing.name);
    let description = arguments
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or(existing.description);
    let condition = arguments
        .get("condition")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or(existing.condition);
    let condition_rules = if arguments.contains_key("condition_rules") {
        arguments
            .get("condition_rules")
            .and_then(|v| v.as_str())
            .map(String::from)
    } else {
        existing.condition_rules
    };
    let enabled = arguments
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(existing.enabled);
    let action = if let Some(action_val) = arguments.get("action") {
        match parse_trigger_action(action_val) {
            Ok(a) => a,
            Err(e) => return CallToolResult::error(e),
        }
    } else {
        existing.action
    };

    let args = if let Some(arr) = arguments.get("args").and_then(|v| v.as_array()) {
        match parse_args(arr) {
            Ok(a) => a,
            Err(e) => return e,
        }
    } else {
        existing.args
    };

    let trigger = Trigger {
        name: name.clone(),
        description,
        condition,
        condition_rules,
        action,
        enabled,
        args,
        created_at: existing.created_at,
    };

    match atproto
        .put_record(TRIGGER_COLLECTION, rkey, &trigger)
        .await
    {
        Ok(_) => CallToolResult::success(
            json!({
                "rkey": rkey,
                "name": name,
                "enabled": enabled,
                "status": "updated"
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to update trigger: {}", e)),
    }
}

pub async fn delete_trigger(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    let atproto = &state.atproto;

    match atproto.delete_record(TRIGGER_COLLECTION, rkey).await {
        Ok(_) => CallToolResult::success(
            json!({
                "deleted": true,
                "rkey": rkey
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to delete trigger: {}", e)),
    }
}

pub async fn list_triggers(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let cache = match state.cache.as_ref() {
        Some(c) => c,
        None => return CallToolResult::error("Cache not available"),
    };

    let enabled_filter = arguments.get("enabled").and_then(|v| v.as_bool());

    let triggers: Vec<Value> = cache
        .list_triggers()
        .into_iter()
        .filter(|(_, cached)| {
            if let Some(filter) = enabled_filter {
                cached.value.enabled == filter
            } else {
                true
            }
        })
        .map(|(rkey, cached)| {
            let t = &cached.value;
            let mut entry = json!({
                "rkey": rkey,
                "name": t.name,
                "description": t.description,
                "condition": t.condition,
                "condition_rules": t.condition_rules,
                "action": format_action(&t.action),
                "enabled": t.enabled,
                "created_at": t.created_at.to_rfc3339(),
            });
            if !t.args.is_empty() {
                entry["args"] = json!(t.args.iter().map(|a| {
                    json!({
                        "name": a.name,
                        "type": a.r#type,
                        "description": a.description
                    })
                }).collect::<Vec<_>>());
            }
            entry
        })
        .collect();

    CallToolResult::success(
        json!({
            "count": triggers.len(),
            "triggers": triggers
        })
        .to_string(),
    )
}

pub async fn test_trigger(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let cache = match state.cache.as_ref() {
        Some(c) => c,
        None => return CallToolResult::error("Cache not available"),
    };
    // Get condition either from existing trigger or ad-hoc
    let (condition, condition_rules, trigger_name, trigger_args) = if let Some(rkey) =
        arguments.get("rkey").and_then(|v| v.as_str())
    {
        match cache.get_trigger(rkey) {
            Some(cached) => (
                cached.value.condition.clone(),
                cached.value.condition_rules.clone(),
                Some(cached.value.name.clone()),
                cached.value.args.clone(),
            ),
            None => return CallToolResult::error(format!("Trigger not found: {}", rkey)),
        }
    } else if let Some(condition) = arguments.get("condition").and_then(|v| v.as_str()) {
        let rules = arguments
            .get("condition_rules")
            .and_then(|v| v.as_str())
            .map(String::from);
        let args = match arguments.get("args").and_then(|v| v.as_array()) {
            Some(arr) => match parse_args(arr) {
                Ok(a) => a,
                Err(e) => return e,
            },
            None => Vec::new(),
        };
        (condition.to_string(), rules, None, args)
    } else {
        return CallToolResult::error("Either rkey or condition must be provided");
    };

    // Wrap condition body into a proper query rule
    let (query, rules) = build_trigger_query(&condition, condition_rules.as_deref());

    // Build typed declaration for _trigger_result if trigger has args
    let extra_decls: Vec<String> = if !trigger_args.is_empty() {
        let params: Vec<String> = trigger_args
            .iter()
            .map(|a| format!("{}: {}", a.name, a.r#type))
            .collect();
        vec![format!("_trigger_result({})", params.join(", "))]
    } else {
        Vec::new()
    };
    let extra_decls_option: Option<&[String]> = if extra_decls.is_empty() {
        None
    } else {
        Some(&extra_decls)
    };

    // Run the query
    let query_result = if let Some(ref datalog_cache) = state.datalog_cache {
        datalog_cache
            .execute_query_with_facts_and_declarations(
                &query,
                rules.as_deref(),
                None,
                extra_decls_option,
            )
            .await
    } else {
        return CallToolResult::error("Datalog not available");
    };

    match query_result {
        Ok(results) => {
            let result_count = results.len();
            let sample: Vec<Value> = results
                .into_iter()
                .take(20)
                .map(|tuple| json!(tuple))
                .collect();

            let mut response = json!({
                "result_count": result_count,
                "results": sample,
                "status": if result_count > 0 { "would_fire" } else { "no_match" },
            });

            if let Some(name) = trigger_name {
                response["trigger_name"] = json!(name);
            }
            if result_count > 20 {
                response["truncated"] = json!(true);
                response["showing"] = json!(20);
            }

            CallToolResult::success(response.to_string())
        }
        Err(e) => CallToolResult::error(format!("Condition query failed: {}", e)),
    }
}

/// Parse a trigger action from a JSON value.
fn parse_trigger_action(value: &Value) -> Result<TriggerAction, String> {
    let action_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Action missing 'type' field".to_string())?;

    match action_type {
        "create_fact" => {
            let predicate = value
                .get("predicate")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "create_fact action missing 'predicate'".to_string())?
                .to_string();
            let args = value
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let tags = value
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            Ok(TriggerAction::CreateFact {
                predicate,
                args,
                tags,
            })
        }
        "create_inbox_item" => {
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "create_inbox_item action missing 'message'".to_string())?
                .to_string();
            Ok(TriggerAction::CreateInboxItem { message })
        }
        "delete_fact" => {
            let rkey = value
                .get("rkey")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "delete_fact action missing 'rkey'".to_string())?
                .to_string();
            Ok(TriggerAction::DeleteFact { rkey })
        }
        other => Err(format!("Unknown action type: {}", other)),
    }
}

/// Format a TriggerAction for display.
fn format_action(action: &TriggerAction) -> Value {
    match action {
        TriggerAction::CreateFact {
            predicate,
            args,
            tags,
        } => json!({
            "type": "create_fact",
            "predicate": predicate,
            "args": args,
            "tags": tags,
        }),
        TriggerAction::CreateInboxItem { message } => json!({
            "type": "create_inbox_item",
            "message": message,
        }),
        TriggerAction::DeleteFact { rkey } => json!({
            "type": "delete_fact",
            "rkey": rkey,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definitions() {
        let defs = definitions();
        assert_eq!(defs.len(), 5);
        assert_eq!(defs[0].name, "create_trigger");
        assert_eq!(defs[1].name, "update_trigger");
        assert_eq!(defs[2].name, "delete_trigger");
        assert_eq!(defs[3].name, "list_triggers");
        assert_eq!(defs[4].name, "test_trigger");
    }

    #[test]
    fn test_tools_have_permissions() {
        let tools = tools();
        assert_eq!(tools.len(), 5);
        for tool in &tools {
            assert!(tool.agent_allowed);
        }
    }

    #[test]
    fn test_parse_create_fact_action() {
        let value = json!({
            "type": "create_fact",
            "predicate": "high_context",
            "args": ["$0", "$1"],
            "tags": ["auto"]
        });
        let action = parse_trigger_action(&value).unwrap();
        match action {
            TriggerAction::CreateFact {
                predicate,
                args,
                tags,
            } => {
                assert_eq!(predicate, "high_context");
                assert_eq!(args, vec!["$0", "$1"]);
                assert_eq!(tags, vec!["auto"]);
            }
            _ => panic!("Expected CreateFact"),
        }
    }

    #[test]
    fn test_parse_create_inbox_item_action() {
        let value = json!({
            "type": "create_inbox_item",
            "message": "New follow from $0"
        });
        let action = parse_trigger_action(&value).unwrap();
        match action {
            TriggerAction::CreateInboxItem { message } => {
                assert_eq!(message, "New follow from $0");
            }
            _ => panic!("Expected CreateInboxItem"),
        }
    }

    #[test]
    fn test_parse_delete_fact_action() {
        let value = json!({
            "type": "delete_fact",
            "rkey": "$0"
        });
        let action = parse_trigger_action(&value).unwrap();
        match action {
            TriggerAction::DeleteFact { rkey } => {
                assert_eq!(rkey, "$0");
            }
            _ => panic!("Expected DeleteFact"),
        }
    }

    #[test]
    fn test_parse_action_missing_type() {
        let value = json!({ "predicate": "test" });
        assert!(parse_trigger_action(&value).is_err());
    }

    #[test]
    fn test_parse_action_unknown_type() {
        let value = json!({ "type": "unknown" });
        assert!(parse_trigger_action(&value).is_err());
    }
}
