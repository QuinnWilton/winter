//! Fact tools for MCP.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tracing::debug;

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Fact, ListRecordItem, Rule, SyncState, Tid, WriteOp, WriteResult};
use winter_datalog::{DerivedFactGenerator, FactExtractor, RuleCompiler, SouffleExecutor};

use super::{MAX_BATCH_SIZE, ToolMeta, ToolState, parse_string_array};

/// Collection name for facts.
const FACT_COLLECTION: &str = "diy.razorgirl.winter.fact";

/// Collection name for rules.
const RULE_COLLECTION: &str = "diy.razorgirl.winter.rule";

/// Parse `expires_at` or `ttl_seconds` from a HashMap (for create_fact, update_fact).
fn parse_expires_at(arguments: &HashMap<String, Value>) -> Option<DateTime<Utc>> {
    if let Some(ts) = arguments.get("expires_at").and_then(|v| v.as_str()) {
        if !ts.is_empty() {
            if let Ok(dt) = ts.parse::<DateTime<Utc>>() {
                return Some(dt);
            }
        }
    }
    if let Some(ttl) = arguments.get("ttl_seconds").and_then(|v| v.as_i64()) {
        if ttl > 0 {
            return Some(Utc::now() + chrono::Duration::seconds(ttl));
        }
    }
    None
}

/// Parse `expires_at` or `ttl_seconds` from a JSON object (for create_facts batch items).
fn parse_expires_at_from_obj(obj: &serde_json::Map<String, Value>) -> Option<DateTime<Utc>> {
    if let Some(ts) = obj.get("expires_at").and_then(|v| v.as_str()) {
        if !ts.is_empty() {
            if let Ok(dt) = ts.parse::<DateTime<Utc>>() {
                return Some(dt);
            }
        }
    }
    if let Some(ttl) = obj.get("ttl_seconds").and_then(|v| v.as_i64()) {
        if ttl > 0 {
            return Some(Utc::now() + chrono::Duration::seconds(ttl));
        }
    }
    None
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "create_fact".to_string(),
            description: "Create a new fact. Facts are atomic, structured knowledge with a predicate and arguments. Use DIDs for account references, never handles.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "predicate": {
                        "type": "string",
                        "description": "The predicate name (e.g., 'follows', 'interested_in', 'mentioned')"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments to the predicate (e.g., ['did:plc:abc', 'did:plc:xyz'] for follows)"
                    },
                    "confidence": {
                        "type": "number",
                        "description": "Confidence level 0.0-1.0 (default 1.0)"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for categorization"
                    },
                    "expires_at": {
                        "type": "string",
                        "description": "Optional ISO 8601 expiration timestamp. Facts past this time are excluded from default queries."
                    },
                    "ttl_seconds": {
                        "type": "integer",
                        "description": "Optional time-to-live in seconds (convenience alternative to expires_at). Computed to expires_at at creation time."
                    }
                },
                "required": ["predicate", "args"]
            }),
        },
        ToolDefinition {
            name: "create_facts".to_string(),
            description: "Create multiple facts in a single atomic transaction. All facts are created together or none are. Use DIDs for account references, never handles.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "facts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "predicate": {
                                    "type": "string",
                                    "description": "The predicate name"
                                },
                                "args": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Arguments to the predicate"
                                },
                                "confidence": {
                                    "type": "number",
                                    "description": "Confidence level 0.0-1.0 (default 1.0)"
                                },
                                "tags": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Optional tags for categorization"
                                },
                                "expires_at": {
                                    "type": "string",
                                    "description": "Optional ISO 8601 expiration timestamp"
                                },
                                "ttl_seconds": {
                                    "type": "integer",
                                    "description": "Optional time-to-live in seconds"
                                }
                            },
                            "required": ["predicate", "args"]
                        },
                        "description": "Array of facts to create"
                    }
                },
                "required": ["facts"]
            }),
        },
        ToolDefinition {
            name: "update_fact".to_string(),
            description: "Update an existing fact by creating a new one that supersedes it. The old fact is preserved for historical queries via _all_{predicate} and _supersedes relations.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the fact to update"
                    },
                    "predicate": {
                        "type": "string",
                        "description": "The predicate name"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments to the predicate"
                    },
                    "confidence": {
                        "type": "number",
                        "description": "Confidence level 0.0-1.0"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags"
                    },
                    "expires_at": {
                        "type": "string",
                        "description": "Optional ISO 8601 expiration timestamp for the new fact"
                    },
                    "ttl_seconds": {
                        "type": "integer",
                        "description": "Optional time-to-live in seconds for the new fact"
                    }
                },
                "required": ["rkey", "predicate", "args"]
            }),
        },
        ToolDefinition {
            name: "delete_fact".to_string(),
            description: "Delete a fact by its record key.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the fact to delete"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "query_facts".to_string(),
            description: r#"Query facts using datalog. By default, queries return only current facts (superseded facts are excluded).

## Available Relations

**User predicates** (current facts, with rkey at end):
- `follows(A, B, Rkey)`, `interested_in(A, B, Rkey)`, etc.

**Historical predicates** (all facts with rkey at end):
- `_all_follows(A, B, Rkey)` - includes superseded facts

**Metadata relations**:
- `_fact(Rkey, Predicate, Cid)` - base relation for all facts
- `_confidence(Rkey, Value)` - only facts with confidence ≠ 1.0
- `_source(Rkey, SourceCid)` - only facts with source set
- `_supersedes(NewRkey, OldRkey)` - supersession chain
- `_created_at(Rkey, Timestamp)` - when each fact was created (ISO8601)
- `_expires_at(Rkey, Timestamp)` - only facts with expiration set (ISO8601)
- `_now(Timestamp)` - current time, auto-injected at query time
- `_expired(Rkey)` - derived: facts past their expiration (computed via `_expires_at` + `_now`)

## Example Queries

- Current follows: `follows(X, Y, _)` or `follows(X, Y, R)` to get rkey
- All historical follows: `_all_follows(X, Y, Rkey)`
- Find what a fact superseded: `_supersedes(NewRkey, OldRkey)`
- Low-confidence facts: `_all_follows(X, Y, R), _confidence(R, C), C < 0.8`
- Facts with sources: `_fact(Rkey, _, _), _source(Rkey, Src)`

**Temporal queries**:
- Facts after a date: `_all_follows(X, Y, R), _created_at(R, T), T > "2026-01-15T00:00:00Z"`
- Recent facts: `_fact(R, P, _), _created_at(R, T), T > "2026-01-01T00:00:00Z"`

**Ephemeral facts** (extra_facts parameter):
Inject runtime context without persisting to the PDS. Useful for thread state, time-based reasoning, etc.
Example: `extra_facts: ["thread_depth(\"at://...\", \"7\")", "my_reply_count(\"at://...\", \"4\")"]`

**Ad-hoc declarations** (extra_declarations parameter):
Declare predicates at query time for predicates not yet stored.
Example: `extra_declarations: ["my_pred(arg1: symbol, arg2: symbol)"]`"#.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The query predicate to evaluate (e.g., 'follows(X, Y, _)' for current facts, '_all_follows(X, Y, Rkey)' for historical)"
                    },
                    "extra_rules": {
                        "type": "string",
                        "description": "Optional ad-hoc rules to include in the query (Soufflé syntax)"
                    },
                    "extra_facts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional ephemeral facts to inject at query time (e.g., [\"thread_depth(\\\"uri\\\", \\\"5\\\")\"]). Not persisted."
                    },
                    "extra_declarations": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional ad-hoc predicate declarations (e.g., [\"my_pred(arg1: symbol, arg2: symbol)\"]). For predicates not yet stored."
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "list_predicates".to_string(),
            description: r#"List all available predicates with their arities.

Shows three categories:
- **User predicates**: From your facts in the PDS (e.g., `thread_completed(arg0, arg1, arg2)`)
- **Derived predicates**: Auto-generated from records (e.g., `follows`, `has_note`)
- **Metadata predicates**: System predicates for querying fact metadata

For each predicate, shows:
- Name and arity
- The `_all_` variant (includes rkey for historical queries)
- Example usage pattern"#.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "search": {
                        "type": "string",
                        "description": "Filter predicate names (case-insensitive substring)"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "list_validation_errors".to_string(),
            description: "List facts that don't conform to their declared schema. Returns rkey, predicate, and error message for each invalid fact.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}

/// Get all fact tools with their permission metadata.
/// All fact tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

pub async fn create_fact(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let predicate = match arguments.get("predicate").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return CallToolResult::error("Missing required parameter: predicate"),
    };

    // Check if this predicate is derived (automatically generated from PDS records)
    if DerivedFactGenerator::is_derived(predicate) {
        return CallToolResult::error(format!(
            "Cannot create derived fact. '{}' is automatically generated from PDS records.",
            predicate
        ));
    }

    let args: Vec<String> = match arguments.get("args").and_then(|v| v.as_array()) {
        Some(a) => match parse_string_array(a, "args") {
            Ok(args) => args,
            Err(e) => return e,
        },
        None => return CallToolResult::error("Missing required parameter: args"),
    };

    let confidence = arguments
        .get("confidence")
        .and_then(|v| v.as_f64())
        .map(|c| c.clamp(0.0, 1.0));

    let tags: Vec<String> = arguments
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let expires_at = parse_expires_at(arguments);

    let fact = Fact {
        predicate: predicate.to_string(),
        args,
        confidence,
        source: None,
        supersedes: None,
        tags,
        created_at: Utc::now(),
        expires_at,
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(FACT_COLLECTION, Some(&rkey), &fact)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_fact(rkey.clone(), fact.clone(), response.cid.clone());
            }
            let mut result = json!({
                "rkey": rkey,
                "uri": response.uri,
                "cid": response.cid,
                "predicate": predicate,
                "args": fact.args
            });
            if let Some(ref ea) = fact.expires_at {
                result["expires_at"] = json!(ea.to_rfc3339());
            }
            CallToolResult::success(result.to_string())
        }
        Err(e) => CallToolResult::error(format!("Failed to create fact: {}", e)),
    }
}

pub async fn create_facts(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let facts_array = match arguments.get("facts").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return CallToolResult::error("Missing required parameter: facts"),
    };

    if facts_array.is_empty() {
        return CallToolResult::error("facts array cannot be empty");
    }

    if facts_array.len() > MAX_BATCH_SIZE {
        return CallToolResult::error(format!(
            "Batch size {} exceeds maximum of {}",
            facts_array.len(),
            MAX_BATCH_SIZE
        ));
    }

    // Validate and parse all facts first
    let mut validated: Vec<(String, Fact)> = Vec::with_capacity(facts_array.len());
    let now = Utc::now();

    for (i, fact_val) in facts_array.iter().enumerate() {
        let obj = match fact_val.as_object() {
            Some(o) => o,
            None => return CallToolResult::error(format!("facts[{}]: expected object", i)),
        };

        let predicate = match obj.get("predicate").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return CallToolResult::error(format!("facts[{}]: missing predicate", i)),
        };

        // Check if this predicate is derived
        if DerivedFactGenerator::is_derived(predicate) {
            return CallToolResult::error(format!(
                "facts[{}]: cannot create derived fact. '{}' is automatically generated from PDS records.",
                i, predicate
            ));
        }

        let args: Vec<String> = match obj.get("args").and_then(|v| v.as_array()) {
            Some(a) => match parse_string_array(a, &format!("facts[{}].args", i)) {
                Ok(args) => args,
                Err(e) => return e,
            },
            None => return CallToolResult::error(format!("facts[{}]: missing args", i)),
        };

        let confidence = obj
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|c| c.clamp(0.0, 1.0));

        let tags: Vec<String> = obj
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let expires_at = parse_expires_at_from_obj(obj);

        let fact = Fact {
            predicate: predicate.to_string(),
            args,
            confidence,
            source: None,
            supersedes: None,
            tags,
            created_at: now,
            expires_at,
        };

        let rkey = Tid::now().to_string();
        validated.push((rkey, fact));
    }

    // Build WriteOp list
    let writes: Vec<WriteOp> = validated
        .iter()
        .map(|(rkey, fact)| WriteOp::Create {
            collection: FACT_COLLECTION.to_string(),
            rkey: rkey.clone(),
            value: serde_json::to_value(fact).expect("Fact struct should always serialize"),
        })
        .collect();

    // Execute batch write
    match state.atproto.apply_writes(writes).await {
        Ok(response) => {
            // Update cache for each created record
            for ((rkey, fact), result) in validated.iter().zip(response.results.iter()) {
                if let WriteResult::Create { cid, .. } = result
                    && let Some(cache) = &state.cache
                {
                    cache.upsert_fact(rkey.clone(), fact.clone(), cid.clone());
                }
            }

            let results: Vec<Value> = validated
                .iter()
                .zip(response.results.iter())
                .map(|((rkey, fact), result)| {
                    if let WriteResult::Create { uri, cid } = result {
                        json!({
                            "rkey": rkey,
                            "uri": uri,
                            "cid": cid,
                            "predicate": fact.predicate,
                            "args": fact.args
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

pub async fn update_fact(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    let predicate = match arguments.get("predicate").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return CallToolResult::error("Missing required parameter: predicate"),
    };

    let args: Vec<String> = match arguments.get("args").and_then(|v| v.as_array()) {
        Some(a) => match parse_string_array(a, "args") {
            Ok(args) => args,
            Err(e) => return e,
        },
        None => return CallToolResult::error("Missing required parameter: args"),
    };

    // Get the old fact to get its CID for the supersedes reference
    let old_record = match state
        .atproto
        .get_record::<Fact>(FACT_COLLECTION, rkey)
        .await
    {
        Ok(record) => record,
        Err(e) => return CallToolResult::error(format!("Failed to get existing fact: {}", e)),
    };

    // Check if the old fact's predicate is derived (automatically generated from PDS records)
    if DerivedFactGenerator::is_derived(&old_record.value.predicate) {
        return CallToolResult::error(format!(
            "Cannot update derived fact. '{}' is automatically generated from PDS records.",
            old_record.value.predicate
        ));
    }

    let old_cid = old_record.cid;

    let confidence = arguments
        .get("confidence")
        .and_then(|v| v.as_f64())
        .map(|c| c.clamp(0.0, 1.0));

    let tags: Vec<String> = arguments
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let expires_at = parse_expires_at(arguments);

    let fact = Fact {
        predicate: predicate.to_string(),
        args,
        confidence,
        source: None,
        supersedes: old_cid,
        tags,
        created_at: Utc::now(),
        expires_at,
    };

    // Create a new fact that supersedes the old one
    let new_rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(FACT_COLLECTION, Some(&new_rkey), &fact)
        .await
    {
        Ok(response) => {
            // Update cache with the new fact
            if let Some(cache) = &state.cache {
                cache.upsert_fact(new_rkey.clone(), fact.clone(), response.cid.clone());
            }
            // Old fact is preserved for historical queries.
            // The supersedes reference in the new fact links them.
            CallToolResult::success(
                json!({
                    "rkey": new_rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "supersedes_rkey": rkey,
                    "predicate": predicate
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to update fact: {}", e)),
    }
}

pub async fn delete_fact(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    // First, fetch the fact to check if it's a derived predicate
    let fact = match state
        .atproto
        .get_record::<Fact>(FACT_COLLECTION, rkey)
        .await
    {
        Ok(record) => record.value,
        Err(e) => return CallToolResult::error(format!("Failed to get fact: {}", e)),
    };

    // Check if this predicate is derived (automatically generated from PDS records)
    if DerivedFactGenerator::is_derived(&fact.predicate) {
        return CallToolResult::error(format!(
            "Cannot delete derived fact. '{}' is automatically generated from PDS records.",
            fact.predicate
        ));
    }

    match state.atproto.delete_record(FACT_COLLECTION, rkey).await {
        Ok(()) => {
            // Remove from cache
            if let Some(cache) = &state.cache {
                cache.delete_fact(rkey);
            }
            CallToolResult::success(
                json!({
                    "deleted": true,
                    "rkey": rkey
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to delete fact: {}", e)),
    }
}

/// Maximum query length to prevent abuse.
const MAX_QUERY_LENGTH: usize = 4096;

/// Patterns that could indicate shell injection attempts.
/// Note: > and < are allowed because they're valid datalog comparison operators.
/// Note: \n and \r are allowed because queries are written to a file, not interpolated
/// into shell commands, and multi-line rules/declarations are common.
const FORBIDDEN_PATTERNS: &[&str] = &["$(", "`", "&&", "||", ";", "|"];

pub async fn query_facts(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let query = match arguments.get("query").and_then(|v| v.as_str()) {
        Some(q) => q.trim(),
        None => return CallToolResult::error("Missing required parameter: query"),
    };

    // Validate query length
    if query.len() > MAX_QUERY_LENGTH {
        return CallToolResult::error(format!(
            "Query too long: {} chars (max {})",
            query.len(),
            MAX_QUERY_LENGTH
        ));
    }

    // Check for shell injection patterns
    for pattern in FORBIDDEN_PATTERNS {
        if query.contains(pattern) {
            return CallToolResult::error(format!(
                "Query contains forbidden pattern: {:?}",
                pattern
            ));
        }
    }

    let extra_rules = arguments.get("extra_rules").and_then(|v| v.as_str());

    // Validate extra_rules if provided
    if let Some(rules) = extra_rules {
        if rules.len() > MAX_QUERY_LENGTH {
            return CallToolResult::error(format!(
                "Extra rules too long: {} chars (max {})",
                rules.len(),
                MAX_QUERY_LENGTH
            ));
        }
        for pattern in FORBIDDEN_PATTERNS {
            if rules.contains(pattern) {
                return CallToolResult::error(format!(
                    "Extra rules contain forbidden pattern: {:?}",
                    pattern
                ));
            }
        }
    }

    // Parse and validate extra_facts if provided
    let extra_facts: Option<Vec<String>> = arguments
        .get("extra_facts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

    // Validate extra_facts if provided
    if let Some(ref facts) = extra_facts {
        let total_len: usize = facts.iter().map(|f| f.len()).sum();
        if total_len > MAX_QUERY_LENGTH {
            return CallToolResult::error(format!(
                "Extra facts too long: {} chars (max {})",
                total_len, MAX_QUERY_LENGTH
            ));
        }
        for fact in facts {
            for pattern in FORBIDDEN_PATTERNS {
                if fact.contains(pattern) {
                    return CallToolResult::error(format!(
                        "Extra fact contains forbidden pattern: {:?}",
                        pattern
                    ));
                }
            }
        }
    }

    // Parse and validate extra_declarations if provided
    let extra_declarations: Option<Vec<String>> = arguments
        .get("extra_declarations")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

    // Validate extra_declarations if provided
    if let Some(ref decls) = extra_declarations {
        let total_len: usize = decls.iter().map(|d| d.len()).sum();
        if total_len > MAX_QUERY_LENGTH {
            return CallToolResult::error(format!(
                "Extra declarations too long: {} chars (max {})",
                total_len, MAX_QUERY_LENGTH
            ));
        }
        for decl in decls {
            for pattern in FORBIDDEN_PATTERNS {
                if decl.contains(pattern) {
                    return CallToolResult::error(format!(
                        "Extra declaration contains forbidden pattern: {:?}",
                        pattern
                    ));
                }
            }
        }
    }

    // Auto-inject session metrics as ephemeral facts when available
    let mut extra_facts = extra_facts;
    let mut extra_declarations = extra_declarations;
    if let Some(ref metrics) = state.session_metrics {
        let m = metrics.read().await;
        let elapsed_min = (chrono::Utc::now() - m.session_start).num_minutes().max(0);
        let context_pct = if m.total_tokens > 0 {
            ((m.total_tokens as f64 / 200_000.0) * 100.0).round() as u64
        } else {
            0
        };
        let error_rate = if m.tool_call_count > 0 {
            ((m.tool_error_count as f64 / m.tool_call_count as f64) * 100.0).round() as u64
        } else {
            0
        };

        let session_facts = vec![
            format!("token_usage_pct({context_pct})"),
            format!("session_duration_min({elapsed_min})"),
            format!("tool_calls({})", m.tool_call_count),
            format!("tool_error_rate({error_rate})"),
        ];

        let session_decls = vec![
            "token_usage_pct(pct: number)".to_string(),
            "session_duration_min(minutes: number)".to_string(),
            "tool_calls(n: number)".to_string(),
            "tool_error_rate(pct: number)".to_string(),
        ];

        extra_facts.get_or_insert_with(Vec::new).extend(session_facts);
        extra_declarations.get_or_insert_with(Vec::new).extend(session_decls);
    }

    // Auto-inject _now(Timestamp) for expiration queries
    {
        let now_ts = Utc::now().to_rfc3339();
        extra_facts
            .get_or_insert_with(Vec::new)
            .push(format!("_now(\"{}\")", now_ts));
    }

    if let Some(ref datalog_cache) = state.datalog_cache {
        let tuples = match datalog_cache
            .execute_query_with_facts_and_declarations(
                query,
                extra_rules,
                extra_facts.as_deref(),
                extra_declarations.as_deref(),
            )
            .await
        {
            Ok(tuples) => tuples,
            Err(e) => return CallToolResult::error(format!("Failed to execute query: {}", e)),
        };

        let results: Vec<Value> = tuples.into_iter().map(|tuple| json!(tuple)).collect();

        return CallToolResult::success(
            json!({
                "query": query,
                "results": results,
                "count": results.len()
            })
            .to_string(),
        );
    }

    // Fall back to non-cached execution

    // Try to use RepoCache first, fall back to HTTP if unavailable
    let (facts, rules) = if let Some(ref cache) = state.cache {
        // Check if cache is synchronized
        if cache.state() == SyncState::Live {
            debug!("using RepoCache for query_facts");
            let cached_facts = cache.list_facts();
            let cached_rules = cache.list_rules();

            // Convert to the format expected by the datalog extractor
            let facts: Vec<winter_atproto::ListRecordItem<Fact>> = cached_facts
                .into_iter()
                .map(|(rkey, cached)| winter_atproto::ListRecordItem {
                    uri: format!("at://did/{}:{}", FACT_COLLECTION, rkey),
                    cid: cached.cid,
                    value: cached.value,
                })
                .collect();

            let rules: Vec<Rule> = cached_rules
                .into_iter()
                .map(|(_, cached)| cached.value)
                .collect();

            (facts, rules)
        } else {
            debug!("cache not live, falling back to HTTP");
            match fetch_facts_and_rules_http(state).await {
                Ok(data) => data,
                Err(err) => return err,
            }
        }
    } else {
        debug!("no cache available, using HTTP");
        match fetch_facts_and_rules_http(state).await {
            Ok(data) => data,
            Err(err) => return err,
        }
    };

    // If no facts, return empty results
    if facts.is_empty() {
        return CallToolResult::success(
            json!({
                "query": query,
                "results": [],
                "note": "No facts in knowledge base"
            })
            .to_string(),
        );
    }

    // Create temp directory for fact files
    let temp_dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => return CallToolResult::error(format!("Failed to create temp directory: {}", e)),
    };

    // Extract facts to TSV files
    if let Err(e) = FactExtractor::extract_to_dir(&facts, temp_dir.path()) {
        return CallToolResult::error(format!("Failed to extract facts: {}", e));
    }

    // Generate Soufflé program
    let mut program = String::new();

    // Input declarations for facts (base predicates)
    let (input_decls, mut declared_predicates) = FactExtractor::generate_input_declarations(&facts);
    program.push_str(&input_decls);

    // Declarations for derived predicates (from rule heads), skip already declared input predicates
    let (derived_decls, derived_predicates) =
        RuleCompiler::generate_derived_declarations(&rules, Some(&declared_predicates));
    program.push_str(&derived_decls);

    // Combine both sets of declared predicates
    declared_predicates.extend(derived_predicates);

    // Compile stored rules
    match RuleCompiler::compile_rules(&rules) {
        Ok(compiled) => program.push_str(&compiled),
        Err(e) => return CallToolResult::error(format!("Failed to compile rules: {}", e)),
    }

    // Generate declarations for ad-hoc rule heads BEFORE adding the rules
    if let Some(extra) = extra_rules {
        let heads = RuleCompiler::parse_extra_rules_heads(extra);
        for (name, arity) in heads {
            if !declared_predicates.contains(&name) {
                let params: Vec<String> = (0..arity).map(|i| format!("arg{}: symbol", i)).collect();
                program.push_str(&format!(".decl {}({})\n", name, params.join(", ")));
                declared_predicates.insert(name);
            }
        }

        program.push_str("// Ad-hoc rules\n");
        program.push_str(extra);
        program.push_str("\n\n");
    }

    // Inject extra_declarations and extra_facts into the program
    let mut predicate_types: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    if let Some(ref decls) = extra_declarations {
        for decl_str in decls {
            if let Some((name, types)) = parse_declaration_types(decl_str) {
                // Add the declaration to the program
                if !declared_predicates.contains(&name) {
                    let params: Vec<String> = types
                        .iter()
                        .enumerate()
                        .map(|(i, t)| format!("arg{}: {}", i, t))
                        .collect();
                    program.push_str(&format!(".decl {}({})\n", name, params.join(", ")));
                    declared_predicates.insert(name.clone());
                }
                predicate_types.entry(name).or_insert(types);
            }
        }
    }
    if let Some(ref facts) = extra_facts {
        for fact_str in facts {
            program.push_str(&format!("{}.\n", fact_str));
        }
    }

    // Generate wrapper rule that properly handles constants as filters
    let (wrapper, _result_arity) =
        generate_query_wrapper(query, Some(&declared_predicates), &predicate_types);
    program.push_str(&wrapper);

    debug!(program = %program, "Generated Soufflé program");

    // Execute Soufflé
    let executor = SouffleExecutor::new();
    let output = match executor.execute(&program, temp_dir.path()).await {
        Ok(out) => out,
        Err(e) => return CallToolResult::error(format!("Failed to execute query: {}", e)),
    };

    // Parse results
    let tuples = SouffleExecutor::parse_output(&output);

    // Format results
    let results: Vec<Value> = tuples.into_iter().map(|tuple| json!(tuple)).collect();

    CallToolResult::success(
        json!({
            "query": query,
            "results": results,
            "count": results.len()
        })
        .to_string(),
    )
}

pub async fn list_validation_errors(
    state: &ToolState,
    _arguments: &HashMap<String, Value>,
) -> CallToolResult {
    // Query the _validation_error predicate via datalog
    let query = "_validation_error(R, P, E)";

    let results = if let Some(ref datalog_cache) = state.datalog_cache {
        datalog_cache.execute_query(query, None).await
    } else {
        return CallToolResult::error("No datalog cache available");
    };

    match results {
        Ok(tuples) => {
            let errors: Vec<Value> = tuples
                .iter()
                .map(|row| {
                    json!({
                        "rkey": row.first().map(String::as_str).unwrap_or(""),
                        "predicate": row.get(1).map(String::as_str).unwrap_or(""),
                        "error": row.get(2).map(String::as_str).unwrap_or("")
                    })
                })
                .collect();

            CallToolResult::success(
                json!({
                    "count": errors.len(),
                    "errors": errors
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to query validation errors: {}", e)),
    }
}

pub async fn list_predicates(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let search_filter = arguments.get("search").and_then(|v| v.as_str());
    let mut user_predicates: Vec<Value> = Vec::new();
    let mut derived_predicates: Vec<Value> = Vec::new();
    let mut metadata_predicates: Vec<Value> = Vec::new();

    // Get derived predicates with full info from DerivedFactGenerator
    for (name, info) in DerivedFactGenerator::predicate_info() {
        // Apply search filter to derived predicates
        if let Some(search) = search_filter
            && !name.to_lowercase().contains(&search.to_lowercase())
        {
            continue;
        }

        let signature = format!("{}({})", name, info.args.join(", "));
        let all_signature = format!("_all_{}({})", name, info.args.join(", "));

        derived_predicates.push(json!({
            "name": name,
            "arity": info.arity,
            "signature": signature,
            "all_signature": all_signature,
            "description": info.description,
            "note": if name == "is_followed_by" { "No rkey (from external API)" } else { "Last arg is rkey" },
        }));
    }

    // Metadata predicates (fixed arities)
    let meta = vec![
        ("_fact", 3, "rkey, predicate, cid"),
        ("_confidence", 2, "rkey, value"),
        ("_source", 2, "rkey, source_cid"),
        ("_supersedes", 2, "new_rkey, old_rkey"),
        ("_created_at", 2, "rkey, timestamp"),
        ("_expires_at", 2, "rkey, timestamp"),
        ("_now", 1, "timestamp (auto-injected at query time)"),
        ("_expired", 1, "rkey (derived: facts past expiration)"),
    ];
    for (name, arity, args) in meta {
        // Apply search filter to metadata predicates
        if let Some(search) = search_filter
            && !name.to_lowercase().contains(&search.to_lowercase())
        {
            continue;
        }

        metadata_predicates.push(json!({
            "name": name,
            "arity": arity,
            "signature": format!("{}({})", name, args),
        }));
    }

    // Get user predicates by querying _fact relation
    let fact_results = if let Some(ref datalog_cache) = state.datalog_cache {
        datalog_cache
            .execute_query("_fact(R, P, C)", None)
            .await
            .ok()
    } else {
        None
    };

    if let Some(results) = fact_results {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for row in &results {
            if row.len() >= 2 {
                seen.insert(row[1].clone());
            }
        }

        for pred in seen {
            // Skip derived predicates (listed separately)
            if DerivedFactGenerator::is_derived(&pred) {
                continue;
            }
            // Apply search filter to user predicates
            if let Some(search) = search_filter
                && !pred.to_lowercase().contains(&search.to_lowercase())
            {
                continue;
            }
            user_predicates.push(json!({
                "name": pred,
                "tip": format!("To check arity, query: {}(A, B, ...) or check {}.facts TSV file", pred, pred),
            }));
        }
    }

    CallToolResult::success(
        json!({
            "user_predicates": user_predicates,
            "derived_predicates": derived_predicates,
            "metadata_predicates": metadata_predicates,
            "usage_notes": {
                "current_facts": "Use predicate(args..., Rkey) for current (non-superseded, non-expired) facts - rkey is last arg",
                "historical_facts": "Use _all_predicate(args..., Rkey) to include superseded and expired facts - same format",
                "rkey_wildcard": "Use _ for rkey if you don't need it: follows(X, Y, _)",
                "arity_check": "If unsure of arity, query: predicate(A, B, ..., R) with enough variables",
                "expiring_facts": "Create facts with expires_at or ttl_seconds. Expired facts excluded from default queries, visible in _all_ variants. Query _expired(R) for expired facts."
            }
        })
        .to_string(),
    )
}

/// Represents a query argument - either a variable or a constant.
#[derive(Debug, Clone, PartialEq)]
enum QueryArg {
    Variable(String),
    Constant(String),
}

/// Parsed query with predicate name and arguments.
#[derive(Debug)]
struct ParsedQuery {
    #[allow(dead_code)]
    name: String,
    args: Vec<QueryArg>,
}

impl ParsedQuery {
    /// Get the variables in this query (for use in result predicate).
    fn variables(&self) -> Vec<&str> {
        self.args
            .iter()
            .filter_map(|arg| match arg {
                QueryArg::Variable(v) => Some(v.as_str()),
                QueryArg::Constant(_) => None,
            })
            .collect()
    }

    /// Get the arity (number of arguments).
    fn arity(&self) -> usize {
        self.args.len()
    }
}

/// Parse a query to extract predicate name and typed arguments.
/// e.g., "should_engage(\"did:plc:abc\")" -> ParsedQuery { name: "should_engage", args: [Constant("\"did:plc:abc\"")] }
/// e.g., "follows(X, Y)" -> ParsedQuery { name: "follows", args: [Variable("X"), Variable("Y")] }
fn parse_query(query: &str) -> Option<ParsedQuery> {
    let paren_idx = query.find('(')?;
    let name = query[..paren_idx].trim().to_string();

    let close_paren = query.rfind(')')?;
    let args_str = &query[paren_idx + 1..close_paren];

    if args_str.trim().is_empty() {
        return Some(ParsedQuery { name, args: vec![] });
    }

    // Parse arguments, handling quoted strings and nested parens
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut depth = 0;

    for c in args_str.chars() {
        match c {
            '"' if depth == 0 => {
                in_string = !in_string;
                current.push(c);
            }
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if !in_string && depth == 0 => {
                let arg = current.trim().to_string();
                if !arg.is_empty() {
                    args.push(parse_single_arg(&arg));
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }

    // Don't forget the last argument
    let arg = current.trim().to_string();
    if !arg.is_empty() {
        args.push(parse_single_arg(&arg));
    }

    Some(ParsedQuery { name, args })
}

/// Parse a single argument to determine if it's a variable or constant.
fn parse_single_arg(arg: &str) -> QueryArg {
    let arg = arg.trim();
    if arg.starts_with('"') && arg.ends_with('"') {
        // String constant
        QueryArg::Constant(arg.to_string())
    } else if arg
        .chars()
        .next()
        .map(|c| c.is_uppercase() || c == '_')
        .unwrap_or(false)
    {
        // Starts with uppercase or underscore = variable in Datalog convention
        QueryArg::Variable(arg.to_string())
    } else {
        // Numeric constant or other literal
        QueryArg::Constant(arg.to_string())
    }
}

/// Parse a declaration string like `"tool_calls(n: number)"` into (name, types).
fn parse_declaration_types(decl: &str) -> Option<(String, Vec<String>)> {
    let decl = decl.trim().strip_prefix(".decl ").unwrap_or(decl);
    let paren_idx = decl.find('(')?;
    let close_paren = decl.rfind(')')?;
    let name = decl[..paren_idx].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let args_str = &decl[paren_idx + 1..close_paren];
    if args_str.trim().is_empty() {
        return Some((name, vec![]));
    }
    let types = args_str
        .split(',')
        .map(|arg| {
            let arg = arg.trim();
            if let Some(colon_idx) = arg.find(':') {
                arg[colon_idx + 1..].trim().to_string()
            } else {
                "symbol".to_string()
            }
        })
        .collect();
    Some((name, types))
}

/// Generate a wrapper rule and output declaration for a query.
/// This ensures constants in the query are properly used as filters.
fn generate_query_wrapper(
    query: &str,
    declared_predicates: Option<&std::collections::HashSet<String>>,
    predicate_types: &std::collections::HashMap<String, Vec<String>>,
) -> (String, usize) {
    let parsed = match parse_query(query) {
        Some(p) => p,
        None => {
            // Fallback: treat as nullary predicate
            return (
                format!(
                    ".decl _query_result()\n.output _query_result\n_query_result() :- {}.\n",
                    query
                ),
                0,
            );
        }
    };

    let variables = parsed.variables();
    let result_arity = if variables.is_empty() {
        // No variables - but if there are constants, return them so user sees what matched
        parsed.arity()
    } else {
        variables.len()
    };

    // Look up the source predicate types to determine result column types.
    let source_types = predicate_types.get(&parsed.name);

    let result_column_types: Vec<String> = if variables.is_empty() {
        (0..parsed.arity())
            .map(|pos| {
                source_types
                    .and_then(|ts| ts.get(pos))
                    .cloned()
                    .unwrap_or_else(|| "symbol".to_string())
            })
            .collect()
    } else {
        parsed
            .args
            .iter()
            .enumerate()
            .filter_map(|(pos, a)| match a {
                QueryArg::Variable(v) if v != "_" => {
                    let t = source_types
                        .and_then(|ts| ts.get(pos))
                        .cloned()
                        .unwrap_or_else(|| "symbol".to_string());
                    Some(t)
                }
                _ => None,
            })
            .collect()
    };

    // Build the result predicate declaration
    let decl = if result_arity > 0 {
        let params: Vec<String> = result_column_types
            .iter()
            .enumerate()
            .map(|(i, t)| format!("arg{}: {}", i, t))
            .collect();
        format!(".decl _query_result({})\n", params.join(", "))
    } else {
        ".decl _query_result()\n".to_string()
    };

    // Build the wrapper rule head
    let head_args = if variables.is_empty() {
        // All constants - include them in the output
        parsed
            .args
            .iter()
            .map(|a| match a {
                QueryArg::Constant(c) => c.as_str(),
                QueryArg::Variable(v) => v.as_str(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        variables.join(", ")
    };

    let head = if head_args.is_empty() {
        "_query_result()".to_string()
    } else {
        format!("_query_result({})", head_args)
    };

    // Check if the base predicate needs declaration (for direct predicate queries without rules)
    let base_decl = if let Some(declared) = declared_predicates {
        if !declared.contains(&parsed.name) && parsed.arity() > 0 {
            let params: Vec<String> = if let Some(types) = source_types {
                types
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("arg{}: {}", i, t))
                    .collect()
            } else {
                (0..parsed.arity())
                    .map(|i| format!("arg{}: symbol", i))
                    .collect()
            };
            format!(".decl {}({})\n", parsed.name, params.join(", "))
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let wrapper = format!(
        "{}{}.output _query_result\n{} :- {}.\n",
        base_decl, decl, head, query
    );

    (wrapper, result_arity)
}

/// Fetch facts and rules via HTTP (fallback when cache is unavailable).
async fn fetch_facts_and_rules_http(
    state: &ToolState,
) -> Result<(Vec<ListRecordItem<Fact>>, Vec<Rule>), CallToolResult> {
    let facts = match state
        .atproto
        .list_all_records::<Fact>(FACT_COLLECTION)
        .await
    {
        Ok(records) => records,
        Err(e) => {
            return Err(CallToolResult::error(format!(
                "Failed to list facts: {}",
                e
            )));
        }
    };

    let rules = match state
        .atproto
        .list_all_records::<Rule>(RULE_COLLECTION)
        .await
    {
        Ok(records) => records.into_iter().map(|r| r.value).collect::<Vec<_>>(),
        Err(e) => {
            return Err(CallToolResult::error(format!(
                "Failed to list rules: {}",
                e
            )));
        }
    };

    Ok((facts, rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_query_with_variables() {
        let parsed = parse_query("mutual_follow(X, Y)").unwrap();
        assert_eq!(parsed.name, "mutual_follow");
        assert_eq!(parsed.arity(), 2);
        assert_eq!(
            parsed.args,
            vec![
                QueryArg::Variable("X".to_string()),
                QueryArg::Variable("Y".to_string())
            ]
        );
        assert_eq!(parsed.variables(), vec!["X", "Y"]);
    }

    #[test]
    fn test_parse_query_with_constant() {
        let parsed = parse_query(r#"should_engage("did:plc:abc")"#).unwrap();
        assert_eq!(parsed.name, "should_engage");
        assert_eq!(parsed.arity(), 1);
        assert_eq!(
            parsed.args,
            vec![QueryArg::Constant(r#""did:plc:abc""#.to_string())]
        );
        assert!(parsed.variables().is_empty());
    }

    #[test]
    fn test_parse_query_mixed_args() {
        let parsed = parse_query(r#"follows(X, "did:plc:abc")"#).unwrap();
        assert_eq!(parsed.name, "follows");
        assert_eq!(parsed.arity(), 2);
        assert_eq!(
            parsed.args,
            vec![
                QueryArg::Variable("X".to_string()),
                QueryArg::Constant(r#""did:plc:abc""#.to_string())
            ]
        );
        assert_eq!(parsed.variables(), vec!["X"]);
    }

    #[test]
    fn test_parse_query_nullary() {
        let parsed = parse_query("has_data()").unwrap();
        assert_eq!(parsed.name, "has_data");
        assert_eq!(parsed.arity(), 0);
        assert!(parsed.args.is_empty());
    }

    #[test]
    fn test_parse_query_underscore_variable() {
        let parsed = parse_query("foo(_X, Y)").unwrap();
        assert_eq!(
            parsed.args,
            vec![
                QueryArg::Variable("_X".to_string()),
                QueryArg::Variable("Y".to_string())
            ]
        );
    }

    #[test]
    fn test_parse_query_numeric_constant() {
        let parsed = parse_query("age(X, 42)").unwrap();
        assert_eq!(
            parsed.args,
            vec![
                QueryArg::Variable("X".to_string()),
                QueryArg::Constant("42".to_string())
            ]
        );
    }

    #[test]
    fn test_generate_query_wrapper_all_variables() {
        let empty_types = std::collections::HashMap::new();
        let (wrapper, arity) = generate_query_wrapper("follows(X, Y)", None, &empty_types);
        assert_eq!(arity, 2);
        assert!(wrapper.contains(".decl _query_result(arg0: symbol, arg1: symbol)"));
        assert!(wrapper.contains(".output _query_result"));
        assert!(wrapper.contains("_query_result(X, Y) :- follows(X, Y)."));
    }

    #[test]
    fn test_generate_query_wrapper_with_constant() {
        let empty_types = std::collections::HashMap::new();
        let (wrapper, arity) =
            generate_query_wrapper(r#"should_engage("did:plc:abc")"#, None, &empty_types);
        // When all args are constants, we still output them so user sees what matched
        assert_eq!(arity, 1);
        assert!(wrapper.contains(".decl _query_result(arg0: symbol)"));
        assert!(wrapper.contains(".output _query_result"));
        assert!(
            wrapper.contains(r#"_query_result("did:plc:abc") :- should_engage("did:plc:abc")."#)
        );
    }

    #[test]
    fn test_generate_query_wrapper_mixed() {
        let empty_types = std::collections::HashMap::new();
        let (wrapper, arity) =
            generate_query_wrapper(r#"follows(X, "did:plc:abc")"#, None, &empty_types);
        // Only variables in output
        assert_eq!(arity, 1);
        assert!(wrapper.contains(".decl _query_result(arg0: symbol)"));
        assert!(wrapper.contains(r#"_query_result(X) :- follows(X, "did:plc:abc")."#));
    }

    #[test]
    fn test_generate_query_wrapper_nullary() {
        let empty_types = std::collections::HashMap::new();
        let (wrapper, arity) = generate_query_wrapper("has_data()", None, &empty_types);
        assert_eq!(arity, 0);
        assert!(wrapper.contains(".decl _query_result()"));
        assert!(wrapper.contains("_query_result() :- has_data()."));
    }

    #[test]
    fn test_parse_string_array_valid() {
        let arr = vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ];
        let result = parse_string_array(&arr, "test");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn test_parse_string_array_empty() {
        let arr: Vec<Value> = vec![];
        let result = parse_string_array(&arr, "test");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Vec::<String>::new());
    }

    #[test]
    fn test_parse_string_array_with_number() {
        let arr = vec![Value::String("a".to_string()), Value::Number(42.into())];
        let result = parse_string_array(&arr, "args");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_string_array_with_null() {
        let arr = vec![Value::Null];
        let result = parse_string_array(&arr, "args");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_string_array_with_object() {
        let arr = vec![Value::Object(serde_json::Map::new())];
        let result = parse_string_array(&arr, "args");
        assert!(result.is_err());
    }

    #[test]
    fn test_forbidden_patterns() {
        // Test that the forbidden patterns list contains expected entries
        assert!(FORBIDDEN_PATTERNS.contains(&"$("));
        assert!(FORBIDDEN_PATTERNS.contains(&"`"));
        assert!(FORBIDDEN_PATTERNS.contains(&";"));
        assert!(FORBIDDEN_PATTERNS.contains(&"|"));

        // > and < are intentionally allowed for datalog comparisons
        assert!(!FORBIDDEN_PATTERNS.contains(&">"));
        assert!(!FORBIDDEN_PATTERNS.contains(&"<"));

        // \n and \r are intentionally allowed for multi-line rules
        assert!(!FORBIDDEN_PATTERNS.contains(&"\n"));
        assert!(!FORBIDDEN_PATTERNS.contains(&"\r"));
    }

    #[test]
    fn test_max_query_length() {
        assert_eq!(MAX_QUERY_LENGTH, 4096);
    }
}
