//! Fact tools for MCP.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};
use tracing::debug;

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Fact, ListRecordItem, Rule, SyncState, Tid};
use winter_datalog::{FactExtractor, RuleCompiler, SouffleExecutor};

use super::ToolState;

/// Collection name for facts.
const FACT_COLLECTION: &str = "diy.razorgirl.winter.fact";

/// Collection name for rules.
const RULE_COLLECTION: &str = "diy.razorgirl.winter.rule";

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
                    }
                },
                "required": ["predicate", "args"]
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

**User predicates** (current facts only):
- `follows(A, B)`, `interested_in(A, B)`, etc.

**Historical predicates** (all facts with rkey):
- `_all_follows(Rkey, A, B)` - includes superseded facts

**Metadata relations**:
- `_fact(Rkey, Predicate, Cid)` - base relation for all facts
- `_confidence(Rkey, Value)` - only facts with confidence ≠ 1.0
- `_source(Rkey, SourceCid)` - only facts with source set
- `_supersedes(NewRkey, OldRkey)` - supersession chain

## Example Queries

- Current follows: `follows(X, Y)`
- All historical follows: `_all_follows(Rkey, X, Y)`
- Find what a fact superseded: `_supersedes(NewRkey, OldRkey)`
- Low-confidence facts: `_all_follows(Rkey, X, Y), _confidence(Rkey, C), C < 0.8`
- Facts with sources: `_fact(Rkey, _, _), _source(Rkey, Src)`"#.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The query predicate to evaluate (e.g., 'follows(X, Y)' for current facts, '_all_follows(Rkey, X, Y)' for historical)"
                    },
                    "extra_rules": {
                        "type": "string",
                        "description": "Optional ad-hoc rules to include in the query (Soufflé syntax)"
                    }
                },
                "required": ["query"]
            }),
        },
    ]
}

pub async fn create_fact(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
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

    let fact = Fact {
        predicate: predicate.to_string(),
        args,
        confidence,
        source: None,
        supersedes: None,
        tags,
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(FACT_COLLECTION, Some(&rkey), &fact)
        .await
    {
        Ok(response) => CallToolResult::success(
            json!({
                "rkey": rkey,
                "uri": response.uri,
                "cid": response.cid,
                "predicate": predicate,
                "args": fact.args
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to create fact: {}", e)),
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
    let old_cid = match state
        .atproto
        .get_record::<Fact>(FACT_COLLECTION, rkey)
        .await
    {
        Ok(record) => record.cid,
        Err(e) => return CallToolResult::error(format!("Failed to get existing fact: {}", e)),
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

    let fact = Fact {
        predicate: predicate.to_string(),
        args,
        confidence,
        source: None,
        supersedes: old_cid,
        tags,
        created_at: Utc::now(),
    };

    // Create a new fact that supersedes the old one
    let new_rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(FACT_COLLECTION, Some(&new_rkey), &fact)
        .await
    {
        Ok(response) => {
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

    match state.atproto.delete_record(FACT_COLLECTION, rkey).await {
        Ok(()) => CallToolResult::success(
            json!({
                "deleted": true,
                "rkey": rkey
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to delete fact: {}", e)),
    }
}

/// Maximum query length to prevent abuse.
const MAX_QUERY_LENGTH: usize = 4096;

/// Patterns that could indicate shell injection attempts.
const FORBIDDEN_PATTERNS: &[&str] = &["$(", "`", "&&", "||", ";", "|", ">", "<", "\n", "\r"];

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

    // Try to use DatalogCache for efficient incremental queries
    if let Some(ref datalog_cache) = state.datalog_cache {
        debug!("using DatalogCache for query_facts");

        // Execute query using the cached datalog state
        let tuples = match datalog_cache.execute_query(query, extra_rules).await {
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

    // Add extra ad-hoc rules if provided
    if let Some(extra) = extra_rules {
        program.push_str("// Ad-hoc rules\n");
        program.push_str(extra);
        program.push_str("\n\n");
    }

    // Parse the query to extract predicate name and arity
    let (query_pred, query_arity) = parse_query_predicate(query);

    // Generate output declaration for the query predicate
    // Skip .decl if predicate was already declared by input facts or rules
    program.push_str(&RuleCompiler::generate_output_declaration(
        &query_pred,
        query_arity,
        Some(&declared_predicates),
    ));

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

/// Parse a query predicate to extract the name and arity.
/// e.g., "mutual_follow(X, Y)" -> ("mutual_follow", 2)
fn parse_query_predicate(query: &str) -> (String, usize) {
    if let Some(paren_idx) = query.find('(') {
        let name = query[..paren_idx].trim().to_string();
        let args_part = &query[paren_idx..];

        // Count arguments by counting commas + 1
        let arity = if args_part.contains(',') {
            args_part.matches(',').count() + 1
        } else if args_part.contains("()") || args_part.trim() == "()" {
            0
        } else {
            1
        };

        (name, arity)
    } else {
        // No parentheses, assume it's just a predicate name with arity 0
        (query.trim().to_string(), 0)
    }
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
    fn test_parse_query_predicate_binary() {
        let (name, arity) = parse_query_predicate("mutual_follow(X, Y)");
        assert_eq!(name, "mutual_follow");
        assert_eq!(arity, 2);
    }

    #[test]
    fn test_parse_query_predicate_unary() {
        let (name, arity) = parse_query_predicate("is_active(X)");
        assert_eq!(name, "is_active");
        assert_eq!(arity, 1);
    }

    #[test]
    fn test_parse_query_predicate_nullary() {
        let (name, arity) = parse_query_predicate("has_data()");
        assert_eq!(name, "has_data");
        assert_eq!(arity, 0);
    }

    #[test]
    fn test_parse_query_predicate_no_parens() {
        let (name, arity) = parse_query_predicate("some_fact");
        assert_eq!(name, "some_fact");
        assert_eq!(arity, 0);
    }

    #[test]
    fn test_parse_query_predicate_ternary() {
        let (name, arity) = parse_query_predicate("relationship(A, B, C)");
        assert_eq!(name, "relationship");
        assert_eq!(arity, 3);
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
    }

    #[test]
    fn test_max_query_length() {
        assert_eq!(MAX_QUERY_LENGTH, 4096);
    }
}
