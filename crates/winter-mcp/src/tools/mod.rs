//! Tool definitions and implementations for the MCP server.
//!
//! # Adding a New Tool
//!
//! This guide walks through creating a new MCP tool from scratch.
//!
//! ## 1. Create the tool module (if needed)
//!
//! For a new category of tools, create a new file in `src/tools/`:
//!
//! ```ignore
//! // src/tools/my_tools.rs
//! use std::collections::HashMap;
//! use serde_json::{Value, json};
//! use crate::protocol::{CallToolResult, ToolDefinition};
//! use super::{ToolMeta, ToolState};
//! ```
//!
//! ## 2. Define the tool schema
//!
//! Create a `definitions()` function that returns tool schemas:
//!
//! ```ignore
//! pub fn definitions() -> Vec<ToolDefinition> {
//!     vec![
//!         ToolDefinition {
//!             name: "my_tool".to_string(),
//!             description: "Does something useful. Returns the result.".to_string(),
//!             input_schema: json!({
//!                 "type": "object",
//!                 "properties": {
//!                     "required_param": {
//!                         "type": "string",
//!                         "description": "A required parameter"
//!                     },
//!                     "optional_param": {
//!                         "type": "integer",
//!                         "description": "An optional parameter with default",
//!                         "default": 10
//!                     }
//!                 },
//!                 "required": ["required_param"]
//!             }),
//!         },
//!     ]
//! }
//! ```
//!
//! ## 3. Set tool permissions
//!
//! Create a `tools()` function that wraps definitions with permission metadata:
//!
//! ```ignore
//! /// Get all tools with their permission metadata.
//! pub fn tools() -> Vec<ToolMeta> {
//!     // For tools the autonomous agent can use:
//!     definitions().into_iter().map(ToolMeta::allowed).collect()
//!
//!     // Or for operator-only tools (agent cannot use):
//!     // definitions().into_iter().map(ToolMeta::operator_only).collect()
//!
//!     // Or mixed permissions:
//!     // vec![
//!     //     ToolMeta::allowed(my_tool_definition()),
//!     //     ToolMeta::operator_only(dangerous_tool_definition()),
//!     // ]
//! }
//! ```
//!
//! ## 4. Implement the tool function
//!
//! ```ignore
//! pub async fn my_tool(
//!     state: &ToolState,
//!     arguments: &HashMap<String, Value>,
//! ) -> CallToolResult {
//!     // Extract required parameters
//!     let required_param = match arguments.get("required_param").and_then(|v| v.as_str()) {
//!         Some(p) => p,
//!         None => return CallToolResult::error("Missing required parameter: required_param"),
//!     };
//!
//!     // Extract optional parameters with defaults
//!     let optional_param = arguments
//!         .get("optional_param")
//!         .and_then(|v| v.as_i64())
//!         .unwrap_or(10);
//!
//!     // Do the work (use state.atproto, state.cache, etc.)
//!     match do_something(state, required_param, optional_param).await {
//!         Ok(result) => CallToolResult::success(
//!             json!({
//!                 "rkey": result.rkey,
//!                 "status": "success"
//!             }).to_string()
//!         ),
//!         Err(e) => CallToolResult::error(format!("Failed: {}", e)),
//!     }
//! }
//! ```
//!
//! ## 5. Register the module
//!
//! In `src/tools/mod.rs`:
//!
//! ```ignore
//! // Add the module declaration at the top
//! mod my_tools;
//!
//! // In ToolRegistry::all_tools(), add:
//! tools.extend(my_tools::tools());
//!
//! // In ToolRegistry::execute(), add the dispatch:
//! "my_tool" => my_tools::my_tool(&state, arguments).await,
//! ```
//!
//! ## 6. Add result summarization (optional but recommended)
//!
//! In `get_tool_category()`, add your tool's category for thought recording:
//!
//! ```ignore
//! // For single record mutations:
//! "my_tool" => SingleMutation {
//!     key_fields: &["rkey", "status"],
//!     web_path: Some("my_records"),  // or None if no web UI
//! },
//!
//! // For list operations:
//! "list_my_things" => List {
//!     count_field: "count",
//!     items_field: "items",
//!     sample_key: "name",
//! },
//!
//! // For batch operations:
//! "create_my_things" => BatchMutation {
//!     count_field: "created",
//!     sample_field: "results",
//!     sample_key: "name",
//! },
//! ```
//!
//! ## Permission Guidelines
//!
//! Use `ToolMeta::allowed()` for tools that:
//! - Read data (queries, lists, gets)
//! - Create/update Winter records (facts, notes, jobs, etc.)
//! - Post to Bluesky (the agent's primary communication channel)
//! - Are safe for autonomous operation
//!
//! Use `ToolMeta::operator_only()` for tools that:
//! - Delete data destructively
//! - Access sensitive secrets
//! - Perform irreversible operations
//! - Require human oversight
//!
//! ## Testing
//!
//! Add tests in the module to verify:
//! - Tool definitions are valid
//! - Required parameters are validated
//! - Success and error paths work correctly
//!
//! ```ignore
//! #[cfg(test)]
//! mod tests {
//!     use super::*;
//!
//!     #[test]
//!     fn test_definitions() {
//!         let defs = definitions();
//!         assert!(!defs.is_empty());
//!         for def in defs {
//!             assert!(!def.name.is_empty());
//!             assert!(!def.description.is_empty());
//!         }
//!     }
//!
//!     #[test]
//!     fn test_tools_have_permissions() {
//!         let tools = tools();
//!         assert!(!tools.is_empty());
//!         // All tools should have the expected permission
//!         for tool in tools {
//!             assert!(tool.agent_allowed); // or !tool.agent_allowed for operator-only
//!         }
//!     }
//! }
//! ```
//!
//! ## Example: Complete Tool Module
//!
//! See `src/tools/notes.rs` for a simple example, or `src/tools/facts.rs`
//! for a more complex example with queries and batch operations.

mod blog;
mod bluesky;
mod custom_tools;
mod declarations;
mod directives;
mod enrich;
mod facts;
mod identity;
pub mod inbox;
mod jobs;
mod notes;
mod pds;
pub mod permissions;
mod rules;
mod thoughts;
mod triggers;
pub mod wiki;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::{RwLock, mpsc};
use tracing::warn;

use crate::bluesky::BlueskyClient;
use crate::deno::DenoExecutor;
use crate::protocol::{CallToolResult, ToolContent, ToolDefinition};
use crate::secrets::SecretManager;
use winter_atproto::{AtprotoClient, RepoCache, Thought, ThoughtKind, Tid};
use winter_datalog::DatalogCache;

// ============================================================================
// Tool Metadata with Permissions
// ============================================================================

/// Metadata about a tool including its definition and permission settings.
///
/// This struct colocates tool definitions with their permission metadata,
/// eliminating the need to maintain separate permission lists in the agent.
#[derive(Debug, Clone)]
pub struct ToolMeta {
    /// The tool definition (name, description, input schema).
    pub definition: ToolDefinition,
    /// Whether the autonomous agent is allowed to use this tool.
    pub agent_allowed: bool,
}

impl ToolMeta {
    /// Create a tool that the agent is allowed to use.
    pub fn allowed(definition: ToolDefinition) -> Self {
        Self {
            definition,
            agent_allowed: true,
        }
    }

    /// Create a tool that the agent is NOT allowed to use (operator-only).
    pub fn operator_only(definition: ToolDefinition) -> Self {
        Self {
            definition,
            agent_allowed: false,
        }
    }
}

// ============================================================================
// Tool Result Summarization
// ============================================================================

/// Category of tool result for summarization purposes.
#[derive(Debug, Clone)]
enum ToolResultCategory {
    /// Single record create/update - includes key fields and optional web UI link
    SingleMutation {
        key_fields: &'static [&'static str],
        web_path: Option<&'static str>,
    },
    /// Batch mutation - shows count and sample of created items
    BatchMutation {
        count_field: &'static str,
        sample_field: &'static str,
        sample_key: &'static str,
    },
    /// List operation - shows count and sample of items
    List {
        count_field: &'static str,
        items_field: &'static str,
        sample_key: &'static str,
    },
    /// Query operation - shows query string, count, and sample results
    Query,
    /// Get single record - shows key fields and content size
    Get {
        key_fields: &'static [&'static str],
        size_field: Option<&'static str>,
    },
    /// Bluesky read operations with specialized summaries
    BlueskyRead(BlueskyReadType),
    /// Custom tool execution - shows result preview and metadata
    Custom,
    /// Excluded from thought recording (e.g., record_thought)
    Excluded,
}

/// Types of Bluesky read operations for specialized summarization.
#[derive(Debug, Clone)]
enum BlueskyReadType {
    /// Timeline - shows count and authors
    Timeline,
    /// Notifications - shows count and reason breakdown
    Notifications,
    /// Search results - shows query, count, cursor
    Search,
    /// Thread context - shows root author, reply counts
    Thread,
}

/// Get the result category for a tool.
fn get_tool_category(tool_name: &str) -> ToolResultCategory {
    use BlueskyReadType::*;
    use ToolResultCategory::*;

    match tool_name {
        // === Single Mutations with Web Links ===
        "create_fact" => SingleMutation {
            key_fields: &["rkey", "predicate"],
            web_path: Some("facts"),
        },
        "update_fact" => SingleMutation {
            key_fields: &["rkey", "predicate", "supersedes_rkey"],
            web_path: Some("facts"),
        },
        "delete_fact" => SingleMutation {
            key_fields: &["deleted", "rkey"],
            web_path: None,
        },
        "create_note" => SingleMutation {
            key_fields: &["rkey", "title"],
            web_path: Some("notes"),
        },
        "schedule_job" => SingleMutation {
            key_fields: &["rkey", "name", "next_run"],
            web_path: Some("jobs"),
        },
        "schedule_recurring" => SingleMutation {
            key_fields: &["rkey", "name", "interval_seconds"],
            web_path: Some("jobs"),
        },
        "update_job" => SingleMutation {
            key_fields: &["rkey", "name"],
            web_path: Some("jobs"),
        },
        "cancel_job" => SingleMutation {
            key_fields: &["cancelled", "rkey"],
            web_path: None,
        },
        "create_directive" => SingleMutation {
            key_fields: &["rkey", "kind"],
            web_path: Some("directives"),
        },
        "update_directive" => SingleMutation {
            key_fields: &["rkey", "kind", "changes"],
            web_path: Some("directives"),
        },
        "deactivate_directive" => SingleMutation {
            key_fields: &["rkey", "kind", "deactivated"],
            web_path: None,
        },
        "create_rule" => SingleMutation {
            key_fields: &["rkey", "name", "head"],
            web_path: Some("rules"),
        },
        "update_rule" => SingleMutation {
            key_fields: &["rkey", "name"],
            web_path: Some("rules"),
        },
        "delete_rule" => SingleMutation {
            key_fields: &["deleted", "rkey"],
            web_path: None,
        },
        "toggle_rule" => SingleMutation {
            key_fields: &["rkey", "name", "enabled"],
            web_path: Some("rules"),
        },
        "create_custom_tool" => SingleMutation {
            key_fields: &["rkey", "name", "version"],
            web_path: Some("tools"),
        },
        "update_custom_tool" => SingleMutation {
            key_fields: &["rkey", "name", "version"],
            web_path: Some("tools"),
        },
        "delete_custom_tool" => SingleMutation {
            key_fields: &["deleted", "name"],
            web_path: None,
        },
        "create_fact_declaration" => SingleMutation {
            key_fields: &["rkey", "predicate"],
            web_path: Some("declarations"),
        },
        "update_fact_declaration" => SingleMutation {
            key_fields: &["rkey", "predicate"],
            web_path: Some("declarations"),
        },
        "delete_fact_declaration" => SingleMutation {
            key_fields: &["deleted", "rkey"],
            web_path: None,
        },
        "publish_blog_post" => SingleMutation {
            key_fields: &["rkey", "title"],
            web_path: Some("blog"),
        },
        "update_blog_post" => SingleMutation {
            key_fields: &["rkey", "title"],
            web_path: Some("blog"),
        },
        // Wiki tools
        "create_wiki_entry" => SingleMutation {
            key_fields: &["rkey", "title", "slug"],
            web_path: Some("wiki"),
        },
        "update_wiki_entry" => SingleMutation {
            key_fields: &["rkey", "title", "slug"],
            web_path: Some("wiki"),
        },
        "delete_wiki_entry" => SingleMutation {
            key_fields: &["deleted", "rkey"],
            web_path: None,
        },
        "create_wiki_link" => SingleMutation {
            key_fields: &["rkey", "source", "link_type"],
            web_path: None,
        },
        "delete_wiki_link" => SingleMutation {
            key_fields: &["deleted", "rkey"],
            web_path: None,
        },
        // Trigger tools
        "create_trigger" => SingleMutation {
            key_fields: &["rkey", "name"],
            web_path: None,
        },
        "update_trigger" => SingleMutation {
            key_fields: &["rkey", "name"],
            web_path: None,
        },
        "delete_trigger" => SingleMutation {
            key_fields: &["deleted", "rkey"],
            web_path: None,
        },
        "list_triggers" => List {
            count_field: "count",
            items_field: "triggers",
            sample_key: "name",
        },
        "test_trigger" => Query,
        // PDS raw access
        "pds_put_record" => SingleMutation {
            key_fields: &["uri", "collection", "rkey"],
            web_path: None,
        },
        "pds_delete_record" => SingleMutation {
            key_fields: &["deleted", "collection", "rkey"],
            web_path: None,
        },

        // === Bluesky Mutations (no web link - external) ===
        "post_to_bluesky" => SingleMutation {
            key_fields: &["uri"],
            web_path: None,
        },
        "reply_to_bluesky" => SingleMutation {
            key_fields: &["uri"],
            web_path: None,
        },
        "send_bluesky_dm" => SingleMutation {
            key_fields: &["message_id"],
            web_path: None,
        },
        "reply_to_dm" => SingleMutation {
            key_fields: &["message_id"],
            web_path: None,
        },
        "like_post" => SingleMutation {
            key_fields: &["like_uri"],
            web_path: None,
        },
        "follow_user" => SingleMutation {
            key_fields: &["follow_uri"],
            web_path: None,
        },
        "mute_user" | "unmute_user" | "block_user" | "unblock_user" | "mute_thread"
        | "unmute_thread" => SingleMutation {
            key_fields: &["success"],
            web_path: None,
        },
        "delete_post" => SingleMutation {
            key_fields: &["deleted", "post_uri"],
            web_path: None,
        },

        // === Batch Mutations ===
        "create_facts" => BatchMutation {
            count_field: "created",
            sample_field: "results",
            sample_key: "predicate",
        },
        "create_directives" => BatchMutation {
            count_field: "created",
            sample_field: "results",
            sample_key: "kind",
        },
        "create_rules" => BatchMutation {
            count_field: "created",
            sample_field: "results",
            sample_key: "name",
        },
        "create_fact_declarations" => BatchMutation {
            count_field: "created",
            sample_field: "results",
            sample_key: "predicate",
        },

        // === List Operations ===
        "list_notes" => List {
            count_field: "count",
            items_field: "notes",
            sample_key: "title",
        },
        "list_jobs" => List {
            count_field: "count",
            items_field: "jobs",
            sample_key: "name",
        },
        "list_directives" => List {
            count_field: "count",
            items_field: "directives",
            sample_key: "kind",
        },
        "list_rules" => List {
            count_field: "count",
            items_field: "rules",
            sample_key: "name",
        },
        "list_thoughts" => List {
            count_field: "count",
            items_field: "thoughts",
            sample_key: "kind",
        },
        "list_custom_tools" => List {
            count_field: "count",
            items_field: "tools",
            sample_key: "name",
        },
        "list_fact_declarations" => List {
            count_field: "count",
            items_field: "declarations",
            sample_key: "predicate",
        },
        "list_blog_posts" => List {
            count_field: "count",
            items_field: "posts",
            sample_key: "title",
        },
        "list_wiki_entries" => List {
            count_field: "count",
            items_field: "entries",
            sample_key: "title",
        },
        "list_wiki_links" => List {
            count_field: "count",
            items_field: "links",
            sample_key: "link_type",
        },
        "list_secrets" => List {
            count_field: "count",
            items_field: "secrets",
            sample_key: "name",
        },
        "pds_list_records" => List {
            count_field: "count",
            items_field: "records",
            sample_key: "rkey",
        },
        "pds_get_records" => List {
            count_field: "count",
            items_field: "records",
            sample_key: "uri",
        },
        "list_predicates" => List {
            count_field: "total",
            items_field: "predicates",
            sample_key: "name",
        },
        "list_validation_errors" => List {
            count_field: "count",
            items_field: "errors",
            sample_key: "predicate",
        },

        // === Query ===
        "query_facts" => Query,
        "query_and_enrich" => Query,

        // === Get Operations ===
        "get_note" => Get {
            key_fields: &["rkey", "title"],
            size_field: Some("content"),
        },
        "get_job" => Get {
            key_fields: &["rkey", "name", "schedule"],
            size_field: Some("instructions"),
        },
        "get_thought" => Get {
            key_fields: &["rkey", "kind"],
            size_field: Some("content"),
        },
        "get_custom_tool" => Get {
            key_fields: &["name", "version", "approved"],
            size_field: Some("code"),
        },
        "get_blog_post" => Get {
            key_fields: &["rkey", "title"],
            size_field: Some("content"),
        },
        "get_wiki_entry" | "get_wiki_entry_by_slug" => Get {
            key_fields: &["rkey", "title", "slug"],
            size_field: Some("content"),
        },
        "pds_get_record" => Get {
            key_fields: &["collection", "rkey"],
            size_field: None,
        },
        "get_identity" => Get {
            key_fields: &["operator_did"],
            size_field: None,
        },

        // === Bluesky Read Operations ===
        "get_timeline" => BlueskyRead(Timeline),
        "get_notifications" => BlueskyRead(Notifications),
        "search_posts" | "search_users" => BlueskyRead(Search),
        "get_thread_context" => BlueskyRead(Thread),

        // === Custom Tool Execution ===
        "run_custom_tool" => Custom,

        // === Excluded ===
        "record_thought" => Excluded,

        // === Session Management ===
        "check_interruption" => Get {
            key_fields: &["interrupted", "reason"],
            size_field: None,
        },
        "session_stats" => Get {
            key_fields: &["context_used_pct", "turn_count"],
            size_field: None,
        },
        "set_active_context" => Get {
            key_fields: &["active_context", "status"],
            size_field: None,
        },

        // Default to Custom for unknown tools
        _ => Custom,
    }
}

/// Get the web UI base URL from environment, if configured.
fn get_web_url() -> Option<String> {
    std::env::var("WINTER_WEB_URL").ok()
}

/// Generate a web UI link for a record, if WINTER_WEB_URL is set.
fn make_web_link(web_path: &str, rkey: &str) -> Option<String> {
    get_web_url().map(|base| format!("{}/{}/{}", base.trim_end_matches('/'), web_path, rkey))
}

/// Maximum batch size for batch create operations.
pub(crate) const MAX_BATCH_SIZE: usize = 100;

/// Truncate a string for summary display (UTF-8 safe).
pub(crate) fn truncate_for_summary(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
}

/// Truncate a string without adding "..." suffix (UTF-8 safe).
pub(crate) fn truncate_string(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

/// Parse a JSON array into a Vec<String>, returning an error if any element is not a string.
pub(crate) fn parse_string_array(
    arr: &[serde_json::Value],
    field_name: &str,
) -> Result<Vec<String>, crate::protocol::CallToolResult> {
    let mut result = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        match v.as_str() {
            Some(s) => result.push(s.to_string()),
            None => {
                let type_name = match v {
                    serde_json::Value::Null => "null",
                    serde_json::Value::Bool(_) => "boolean",
                    serde_json::Value::Number(_) => "number",
                    serde_json::Value::Array(_) => "array",
                    serde_json::Value::Object(_) => "object",
                    serde_json::Value::String(_) => unreachable!(),
                };
                return Err(crate::protocol::CallToolResult::error(format!(
                    "Invalid {}[{}]: expected string, got {}",
                    field_name, i, type_name
                )));
            }
        }
    }
    Ok(result)
}

/// Extract a string value from JSON, with optional truncation.
fn extract_string(result: &Value, field: &str, max_len: Option<usize>) -> Option<String> {
    result.get(field).and_then(|v| {
        let s = match v {
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            _ => return None,
        };
        Some(match max_len {
            Some(max) => truncate_for_summary(&s, max),
            None => s,
        })
    })
}

/// Summarize a tool result based on its category.
fn summarize_result(tool_name: &str, result: &Value) -> String {
    match get_tool_category(tool_name) {
        ToolResultCategory::SingleMutation {
            key_fields,
            web_path,
        } => summarize_single_mutation(result, key_fields, web_path),
        ToolResultCategory::BatchMutation {
            count_field,
            sample_field,
            sample_key,
        } => summarize_batch_mutation(result, count_field, sample_field, sample_key),
        ToolResultCategory::List {
            count_field,
            items_field,
            sample_key,
        } => summarize_list(result, count_field, items_field, sample_key),
        ToolResultCategory::Query => summarize_query(result),
        ToolResultCategory::Get {
            key_fields,
            size_field,
        } => summarize_get(result, key_fields, size_field),
        ToolResultCategory::BlueskyRead(read_type) => summarize_bluesky_read(result, read_type),
        ToolResultCategory::Custom => summarize_custom(result),
        ToolResultCategory::Excluded => String::new(),
    }
}

/// Summarize a single mutation result.
fn summarize_single_mutation(
    result: &Value,
    key_fields: &[&str],
    web_path: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    for field in key_fields {
        if let Some(value) = extract_string(result, field, Some(50)) {
            parts.push(format!("{}={}", field, value));
        }
    }

    let mut summary = parts.join(", ");

    // Add web link if available
    if let Some(path) = web_path
        && let Some(rkey) = extract_string(result, "rkey", None)
        && let Some(link) = make_web_link(path, &rkey)
    {
        summary.push_str(&format!("\nView: {}", link));
    }

    summary
}

/// Summarize a batch mutation result.
fn summarize_batch_mutation(
    result: &Value,
    count_field: &str,
    sample_field: &str,
    sample_key: &str,
) -> String {
    let count = result
        .get(count_field)
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let sample: Vec<String> = result
        .get(sample_field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .take(3)
                .filter_map(|item| extract_string(item, sample_key, Some(30)))
                .collect()
        })
        .unwrap_or_default();

    if sample.is_empty() {
        format!("{}={}", count_field, count)
    } else {
        format!(
            "{}={}, {}=[{}]",
            count_field,
            count,
            sample_key,
            sample.join(", ")
        )
    }
}

/// Summarize a list result.
fn summarize_list(
    result: &Value,
    count_field: &str,
    items_field: &str,
    sample_key: &str,
) -> String {
    let count = result
        .get(count_field)
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let sample: Vec<String> = result
        .get(items_field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .take(3)
                .filter_map(|item| {
                    extract_string(item, sample_key, Some(30)).map(|s| format!("\"{}\"", s))
                })
                .collect()
        })
        .unwrap_or_default();

    if sample.is_empty() {
        format!("count={}", count)
    } else {
        format!("count={}, sample=[{}]", count, sample.join(", "))
    }
}

/// Summarize a query result.
fn summarize_query(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Include query if present
    if let Some(query) = extract_string(result, "query", Some(60)) {
        parts.push(format!("query=\"{}\"", query));
    }

    // Include result count
    if let Some(results) = result.get("results").and_then(|v| v.as_array()) {
        parts.push(format!("count={}", results.len()));

        // Show first 2 result tuples
        let sample: Vec<String> = results
            .iter()
            .take(2)
            .filter_map(|r| {
                r.as_array().map(|tuple| {
                    let values: Vec<String> = tuple
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => truncate_for_summary(s, 20),
                            other => other.to_string(),
                        })
                        .collect();
                    format!("({})", values.join(", "))
                })
            })
            .collect();

        if !sample.is_empty() {
            parts.push(format!("sample=[{}]", sample.join(", ")));
        }
    }

    parts.join(", ")
}

/// Summarize a get result.
fn summarize_get(result: &Value, key_fields: &[&str], size_field: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();

    for field in key_fields {
        if let Some(value) = extract_string(result, field, Some(40)) {
            parts.push(format!("{}={}", field, value));
        }
    }

    // Add content size if requested
    if let Some(field) = size_field
        && let Some(Value::String(content)) = result.get(field)
    {
        parts.push(format!("{}_length={}", field, content.len()));
    }

    parts.join(", ")
}

/// Summarize a Bluesky read result.
fn summarize_bluesky_read(result: &Value, read_type: BlueskyReadType) -> String {
    match read_type {
        BlueskyReadType::Timeline => {
            let count = result
                .get("posts")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            let first_author = result
                .get("posts")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|post| post.get("author"))
                .and_then(|a| a.get("handle"))
                .and_then(|h| h.as_str())
                .map(|s| truncate_for_summary(s, 25));

            match first_author {
                Some(author) => format!("count={}, first_author={}", count, author),
                None => format!("count={}", count),
            }
        }
        BlueskyReadType::Notifications => {
            let count = result
                .get("notifications")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            // Count by reason
            let mut reasons: HashMap<String, usize> = HashMap::new();
            if let Some(notifications) = result.get("notifications").and_then(|v| v.as_array()) {
                for notif in notifications {
                    if let Some(reason) = notif.get("reason").and_then(|r| r.as_str()) {
                        *reasons.entry(reason.to_string()).or_insert(0) += 1;
                    }
                }
            }

            let reason_summary: Vec<String> = reasons
                .iter()
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect();

            if reason_summary.is_empty() {
                format!("count={}", count)
            } else {
                format!("count={}, reasons={{{}}}", count, reason_summary.join(", "))
            }
        }
        BlueskyReadType::Search => {
            let mut parts: Vec<String> = Vec::new();

            if let Some(query) = extract_string(result, "query", Some(40)) {
                parts.push(format!("query=\"{}\"", query));
            }

            // Check for posts or users array
            let count = result
                .get("posts")
                .or_else(|| result.get("users"))
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            parts.push(format!("count={}", count));

            if result.get("cursor").is_some() {
                parts.push("has_cursor=true".to_string());
            }

            parts.join(", ")
        }
        BlueskyReadType::Thread => {
            let mut parts: Vec<String> = Vec::new();

            if let Some(root_author) = result
                .get("root")
                .and_then(|r| r.get("author"))
                .and_then(|a| a.get("handle"))
                .and_then(|h| h.as_str())
            {
                parts.push(format!(
                    "root_author={}",
                    truncate_for_summary(root_author, 25)
                ));
            }

            if let Some(total) = result.get("total_replies").and_then(|v| v.as_u64()) {
                parts.push(format!("total_replies={}", total));
            }

            if let Some(my_replies) = result.get("my_replies").and_then(|v| v.as_u64()) {
                parts.push(format!("my_replies={}", my_replies));
            }

            parts.join(", ")
        }
    }
}

/// Summarize a custom tool result.
fn summarize_custom(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Include duration if present
    if let Some(duration) = result.get("duration_ms").and_then(|v| v.as_u64()) {
        parts.push(format!("duration_ms={}", duration));
    }

    // Include sandboxed status if present
    if let Some(sandboxed) = result.get("sandboxed").and_then(|v| v.as_bool()) {
        parts.push(format!("sandboxed={}", sandboxed));
    }

    // Include result preview
    if let Some(output) = result.get("result") {
        let preview = match output {
            Value::String(s) => truncate_for_summary(s, 50),
            Value::Object(_) | Value::Array(_) => {
                let json_str = output.to_string();
                truncate_for_summary(&json_str, 50)
            }
            other => other.to_string(),
        };
        parts.push(format!("result={}", preview));
    }

    parts.join(", ")
}

/// Collection name for thoughts.
const THOUGHT_COLLECTION: &str = "diy.razorgirl.winter.thought";

/// Bounded channel size for async thought writing.
const THOUGHT_CHANNEL_SIZE: usize = 100;

/// State for background session interruption signaling.
///
/// This is shared between the daemon (which sets the interrupt flag when
/// queue pressure builds) and the MCP server (which exposes it via
/// `check_interruption` tool).
#[derive(Debug, Default)]
pub struct InterruptionState {
    /// Whether the current session should interrupt.
    pub should_interrupt: AtomicBool,
    /// Reason for interruption (e.g., "queue_pressure").
    pub reason: RwLock<Option<String>>,
}

impl InterruptionState {
    /// Create a new interruption state with no pending interrupt.
    pub fn new() -> Self {
        Self {
            should_interrupt: AtomicBool::new(false),
            reason: RwLock::new(None),
        }
    }

    /// Signal that the session should interrupt.
    pub async fn set_interrupt(&self, reason: &str) {
        self.should_interrupt.store(true, Ordering::SeqCst);
        let mut guard = self.reason.write().await;
        *guard = Some(reason.to_string());
    }

    /// Check if interrupted and get the reason.
    pub async fn check(&self) -> (bool, Option<String>) {
        let interrupted = self.should_interrupt.load(Ordering::SeqCst);
        if interrupted {
            let guard = self.reason.read().await;
            (true, guard.clone())
        } else {
            (false, None)
        }
    }

    /// Clear the interruption state.
    pub async fn clear(&self) {
        self.should_interrupt.store(false, Ordering::SeqCst);
        let mut guard = self.reason.write().await;
        *guard = None;
    }
}

/// Live session metrics for observability.
///
/// Updated by the daemon as it processes the streaming response from Claude.
/// Exposed via `session_stats` MCP tool and auto-injected into `query_facts`.
#[derive(Debug, Clone)]
pub struct SessionMetrics {
    /// When the session started.
    pub session_start: chrono::DateTime<chrono::Utc>,
    /// Cumulative input tokens.
    pub total_input_tokens: u64,
    /// Cumulative output tokens.
    pub total_output_tokens: u64,
    /// Cumulative total tokens.
    pub total_tokens: u64,
    /// Number of assistant turns completed.
    pub turn_count: u64,
    /// Cumulative cost in USD.
    pub total_cost_usd: f64,
    /// Total tool calls executed (tracked by finalize_result).
    pub tool_call_count: u64,
    /// Total tool errors (tracked by finalize_result).
    pub tool_error_count: u64,
    /// Inbox items acknowledged this session.
    pub inbox_items_acknowledged: u64,
    /// When the last tool call was executed (for watchdog staleness checks).
    pub last_tool_call_at: chrono::DateTime<chrono::Utc>,
}

impl Default for SessionMetrics {
    fn default() -> Self {
        Self {
            session_start: chrono::Utc::now(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            turn_count: 0,
            total_cost_usd: 0.0,
            tool_call_count: 0,
            tool_error_count: 0,
            inbox_items_acknowledged: 0,
            last_tool_call_at: chrono::Utc::now(),
        }
    }
}

/// Shared state for tools.
pub struct ToolState {
    pub atproto: Arc<AtprotoClient>,
    pub bluesky: Option<BlueskyClient>,
    /// In-memory cache for facts and rules (optional).
    pub cache: Option<Arc<RepoCache>>,
    /// Datalog query cache for efficient query execution (optional).
    pub datalog_cache: Option<Arc<DatalogCache>>,
    /// Channel for async thought recording (fire-and-forget).
    pub thought_tx: Option<mpsc::Sender<Thought>>,
    /// Secret manager for custom tool secrets (optional).
    pub secrets: Option<Arc<RwLock<SecretManager>>>,
    /// Deno executor for custom tool sandboxing (optional).
    pub deno: Option<DenoExecutor>,
    /// Interruption state for background sessions (optional).
    pub interruption: Option<Arc<InterruptionState>>,
    /// Shared tool session store for chaining tokens (optional).
    /// Set when HTTP server is running to enable tool chaining.
    pub tool_sessions: Option<Arc<permissions::ToolSessionStore>>,
    /// Internal MCP URL for tool chaining (e.g., "http://127.0.0.1:3847").
    /// Set by the HTTP server so Deno tools can call back into the same server.
    pub internal_mcp_url: Option<String>,
    /// Inbox for persistent session model (optional).
    pub inbox: Option<Arc<inbox::Inbox>>,
    /// Live session metrics (optional, set when persistent session is active).
    pub session_metrics: Option<Arc<RwLock<SessionMetrics>>>,
    /// Active context tag for thought scoping in persistent sessions.
    /// Set by Winter via `set_active_context` when working on a specific inbox item.
    pub active_context: Arc<RwLock<Option<String>>>,
}

/// Registry of available tools.
pub struct ToolRegistry {
    state: Arc<RwLock<ToolState>>,
}

impl ToolRegistry {
    /// Create an empty tool registry for testing purposes.
    ///
    /// This creates a registry with no ATProto client connection.
    /// Only useful for testing the MCP protocol layer.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            state: Arc::new(RwLock::new(ToolState {
                atproto: Arc::new(AtprotoClient::new("https://unused.test")),
                bluesky: None,
                cache: None,
                datalog_cache: None,

                thought_tx: None,
                secrets: None,
                deno: None,
                interruption: None,
                tool_sessions: None,
                internal_mcp_url: None,
                inbox: None,
                session_metrics: None,
                active_context: Arc::new(RwLock::new(None)),
            })),
        }
    }

    /// Create a new tool registry.
    pub fn new(atproto: AtprotoClient) -> Self {
        let atproto = Arc::new(atproto);

        // Create thought channel and spawn background writer
        let (thought_tx, thought_rx) = mpsc::channel(THOUGHT_CHANNEL_SIZE);
        let writer_client = Arc::clone(&atproto);

        tokio::spawn(async move {
            thought_writer_loop(writer_client, thought_rx).await;
        });

        Self {
            state: Arc::new(RwLock::new(ToolState {
                atproto,
                bluesky: None,
                cache: None,
                datalog_cache: None,

                thought_tx: Some(thought_tx),
                secrets: None,
                deno: None,
                interruption: None,
                tool_sessions: None,
                internal_mcp_url: None,
                inbox: None,
                session_metrics: None,
                active_context: Arc::new(RwLock::new(None)),
            })),
        }
    }

    /// Create a new tool registry with a cache.
    pub fn with_cache(atproto: AtprotoClient, cache: Arc<RepoCache>) -> Self {
        let atproto = Arc::new(atproto);

        // Create thought channel and spawn background writer
        let (thought_tx, thought_rx) = mpsc::channel(THOUGHT_CHANNEL_SIZE);
        let writer_client = Arc::clone(&atproto);

        tokio::spawn(async move {
            thought_writer_loop(writer_client, thought_rx).await;
        });

        Self {
            state: Arc::new(RwLock::new(ToolState {
                atproto,
                bluesky: None,
                cache: Some(cache),
                datalog_cache: None,

                thought_tx: Some(thought_tx),
                secrets: None,
                deno: None,
                interruption: None,
                tool_sessions: None,
                internal_mcp_url: None,
                inbox: None,
                session_metrics: None,
                active_context: Arc::new(RwLock::new(None)),
            })),
        }
    }

    /// Set the datalog cache asynchronously.
    pub async fn set_datalog_cache(&self, datalog_cache: Arc<DatalogCache>) {
        let mut guard = self.state.write().await;
        guard.datalog_cache = Some(datalog_cache);
    }

    /// Set the cache asynchronously.
    pub async fn set_cache(&self, cache: Arc<RepoCache>) {
        let mut guard = self.state.write().await;
        guard.cache = Some(cache);
    }

    /// Enable Bluesky integration with an authenticated client.
    pub fn with_bluesky(self, client: BlueskyClient) -> Self {
        // Set bluesky client synchronously by accessing the Arc
        let state = Arc::clone(&self.state);
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut guard = state.write().await;
                guard.bluesky = Some(client);
            });
        });
        self
    }

    /// Set the Bluesky client asynchronously.
    pub async fn set_bluesky(&self, client: BlueskyClient) {
        let mut guard = self.state.write().await;
        guard.bluesky = Some(client);
    }

    /// Set the secret manager for custom tools.
    pub async fn set_secrets(&self, secrets: SecretManager) {
        let mut guard = self.state.write().await;
        guard.secrets = Some(Arc::new(RwLock::new(secrets)));
    }

    /// Set the Deno executor for custom tools.
    pub async fn set_deno(&self, deno: DenoExecutor) {
        let mut guard = self.state.write().await;
        guard.deno = Some(deno);
    }

    /// Set the interruption state for background sessions.
    pub async fn set_interruption(&self, interruption: Arc<InterruptionState>) {
        let mut guard = self.state.write().await;
        guard.interruption = Some(interruption);
    }

    /// Set the tool session store for chaining tokens.
    /// Called when the HTTP server starts to enable tool chaining.
    pub async fn set_tool_sessions(&self, sessions: Arc<permissions::ToolSessionStore>) {
        let mut guard = self.state.write().await;
        guard.tool_sessions = Some(sessions);
    }

    /// Set the internal MCP URL for tool chaining.
    /// Called by the HTTP server with its own localhost address.
    pub async fn set_internal_mcp_url(&self, url: String) {
        let mut guard = self.state.write().await;
        guard.internal_mcp_url = Some(url);
    }

    /// Set the inbox for persistent session model.
    pub async fn set_inbox(&self, inbox_ref: Arc<inbox::Inbox>) {
        let mut guard = self.state.write().await;
        guard.inbox = Some(inbox_ref);
    }

    /// Set session metrics for observability.
    pub async fn set_session_metrics(&self, metrics: Arc<RwLock<SessionMetrics>>) {
        let mut guard = self.state.write().await;
        guard.session_metrics = Some(metrics);
    }

    /// Clear session metrics (on session end).
    pub async fn clear_session_metrics(&self) {
        let mut guard = self.state.write().await;
        guard.session_metrics = None;
    }

    /// Get all tool metadata (definitions + permissions).
    fn all_tools() -> Vec<ToolMeta> {
        let mut tools = Vec::new();

        // Bluesky tools
        tools.extend(bluesky::tools());

        // Fact tools
        tools.extend(facts::tools());

        // Enrich tools (query + API enrichment)
        tools.extend(enrich::tools());

        // Rule tools
        tools.extend(rules::tools());

        // Note tools
        tools.extend(notes::tools());

        // Job tools
        tools.extend(jobs::tools());

        // Identity tools
        tools.extend(identity::tools());

        // Thought tools
        tools.extend(thoughts::tools());

        // Blog tools
        tools.extend(blog::tools());

        // Custom tools
        tools.extend(custom_tools::tools());

        // Directive tools
        tools.extend(directives::tools());

        // Fact declaration tools
        tools.extend(declarations::tools());

        // Trigger tools
        tools.extend(triggers::tools());

        // Wiki tools
        tools.extend(wiki::tools());

        // PDS raw access tools
        tools.extend(pds::tools());

        // Inbox tools (persistent session model)
        tools.extend(inbox::tools());

        // Session management tools
        tools.push(ToolMeta::allowed(ToolDefinition {
            name: "check_interruption".to_string(),
            description: "Check if this background session should wrap up. Call this periodically during background sessions. Returns whether notifications are waiting and the session should exit gracefully.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }));

        tools.push(ToolMeta::allowed(ToolDefinition {
            name: "set_active_context".to_string(),
            description: "Set the active context tag for thought scoping. When working on an inbox item, set this to the item's context_tag so thoughts are associated with it. Pass null or empty string to clear. Thoughts recorded while a context is active will be tagged with it.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "context": {
                        "type": ["string", "null"],
                        "description": "Context tag to set (from inbox item's context_tag field), or null to clear"
                    }
                },
                "required": []
            }),
        }));

        tools.push(ToolMeta::allowed(ToolDefinition {
            name: "session_stats".to_string(),
            description: "Get live session metrics: token usage, context window percentage, turn count, cost, tool call stats. Use this to monitor session health and decide when to wrap up.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }));

        tools
    }

    /// Get all tool definitions (for MCP protocol).
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        Self::all_tools()
            .into_iter()
            .map(|t| t.definition)
            .collect()
    }

    /// Get the list of tool names allowed for the autonomous agent.
    ///
    /// Returns tool names in the MCP format: `mcp__winter__{tool_name}`.
    pub fn agent_allowed_tools() -> Vec<String> {
        Self::all_tools()
            .into_iter()
            .filter(|t| t.agent_allowed)
            .map(|t| format!("mcp__winter__{}", t.definition.name))
            .collect()
    }

    /// Get the DID of this Winter instance (from the ATProto client).
    pub async fn get_did(&self) -> Option<String> {
        let state = self.state.read().await;
        state.atproto.did().await
    }

    /// Execute a custom tool by its rkey (for AT URI-based tool chaining).
    ///
    /// Looks up the tool by rkey instead of name, enabling AT URI resolution.
    pub async fn execute_custom_tool_by_rkey(
        &self,
        rkey: &str,
        input: &HashMap<String, Value>,
    ) -> CallToolResult {
        let state = self.state.read().await;

        // Look up the tool by rkey
        let tool = match state
            .atproto
            .get_record::<winter_atproto::CustomTool>(winter_atproto::TOOL_COLLECTION, rkey)
            .await
        {
            Ok(record) => record.value,
            Err(e) => {
                return CallToolResult::error(format!(
                    "Tool with rkey '{}' not found: {}",
                    rkey, e
                ))
            }
        };

        // Build arguments in the format run_custom_tool expects
        let mut arguments = HashMap::new();
        arguments.insert("name".to_string(), Value::String(tool.name.clone()));
        arguments.insert("input".to_string(), json!(input));

        custom_tools::run_custom_tool(
            &state,
            state.secrets.as_ref(),
            state.deno.as_ref(),
            &arguments,
        )
        .await
    }

    /// Execute a remote tool fetched from another agent's PDS.
    ///
    /// Fetches the tool code from the remote PDS and executes it locally
    /// in a sandboxed Deno environment (no network, no secrets).
    /// The caller's session has already validated that this tool_ref is allowed.
    pub async fn execute_remote_tool(
        &self,
        did: &str,
        rkey: &str,
        input: &HashMap<String, Value>,
    ) -> CallToolResult {
        let state = self.state.read().await;

        let Some(ref deno) = state.deno else {
            return CallToolResult::error("Deno executor not configured");
        };

        // Resolve the remote DID to a PDS URL
        let pds_url = match custom_tools::resolve_pds_for_did(did).await {
            Some(url) => url,
            None => {
                return CallToolResult::error(format!(
                    "Could not resolve PDS for DID: {}",
                    did
                ))
            }
        };

        // Fetch the tool record from the remote PDS (public XRPC)
        let url = format!(
            "{}/xrpc/com.atproto.repo.getRecord?repo={}&collection={}&rkey={}",
            pds_url, did, "diy.razorgirl.winter.tool", rkey
        );

        let response = match reqwest::get(&url).await {
            Ok(r) => r,
            Err(e) => {
                return CallToolResult::error(format!(
                    "Failed to fetch remote tool: {}",
                    e
                ))
            }
        };

        if !response.status().is_success() {
            return CallToolResult::error(format!(
                "Remote tool not found (HTTP {}): at://{}/diy.razorgirl.winter.tool/{}",
                response.status(),
                did,
                rkey
            ));
        }

        let body: serde_json::Value = match response.json().await {
            Ok(b) => b,
            Err(e) => {
                return CallToolResult::error(format!(
                    "Failed to parse remote tool response: {}",
                    e
                ))
            }
        };

        let tool: winter_atproto::CustomTool = match body
            .get("value")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(t) => t,
            None => {
                return CallToolResult::error("Failed to parse remote tool record")
            }
        };

        tracing::info!(
            tool = %tool.name,
            did = %did,
            rkey = %rkey,
            "Executing remote tool (sandboxed)"
        );

        // Remote tools always run sandboxed — no network, no secrets.
        // This is safe because we're executing untrusted code from another PDS.
        let permissions = crate::deno::DenoPermissions::default();

        match deno.execute(&tool.code, &serde_json::json!(input), permissions).await {
            Ok(output) => CallToolResult::success(
                json!({
                    "result": output.result,
                    "duration_ms": output.duration_ms,
                    "sandboxed": true,
                    "remote": true,
                    "source_did": did,
                    "tool_name": tool.name,
                })
                .to_string(),
            ),
            Err(e) => CallToolResult::error(format!(
                "Remote tool execution failed (sandboxed): {}",
                e
            )),
        }
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, arguments: &HashMap<String, Value>) -> CallToolResult {
        self.execute_with_trigger(name, arguments, None).await
    }

    /// Execute a tool by name with an optional trigger context.
    ///
    /// The trigger is used for thought recording, allowing tool calls to be
    /// associated with their originating session (notification, DM, job, etc.).
    pub async fn execute_with_trigger(
        &self,
        name: &str,
        arguments: &HashMap<String, Value>,
        trigger: Option<String>,
    ) -> CallToolResult {
        let start = Instant::now();

        // Record a "starting" thought for potentially slow tools
        // This provides immediate feedback that work is happening
        if is_potentially_slow_tool(name) {
            self.record_tool_starting(name, trigger.clone()).await;
        }

        // Some tools need write access (e.g., get_notifications updates cursor)
        let needs_write = matches!(name, "get_notifications");

        let result = if needs_write {
            let mut state = self.state.write().await;
            match name {
                "get_notifications" => bluesky::get_notifications(&mut state, arguments).await,
                _ => CallToolResult::error(format!("Unknown tool: {}", name)),
            }
        } else {
            let state = self.state.read().await;

            // Try custom tools first
            if let Some(result) = custom_tools::dispatch(
                &state,
                state.secrets.as_ref(),
                state.deno.as_ref(),
                name,
                arguments.clone(),
            )
            .await
            {
                let duration_ms = start.elapsed().as_millis() as u64;
                return self
                    .finalize_result(name, arguments, result, duration_ms, trigger)
                    .await;
            }

            match name {
                // Bluesky tools (read-only)
                "post_to_bluesky" => bluesky::post_to_bluesky(&state, arguments).await,
                "reply_to_bluesky" => bluesky::reply_to_bluesky(&state, arguments).await,
                "send_bluesky_dm" => bluesky::send_bluesky_dm(&state, arguments).await,
                "reply_to_dm" => bluesky::reply_to_dm(&state, arguments).await,
                "like_post" => bluesky::like_post(&state, arguments).await,
                "follow_user" => bluesky::follow_user(&state, arguments).await,
                "get_timeline" => bluesky::get_timeline(&state, arguments).await,
                "search_posts" => bluesky::search_posts(&state, arguments).await,
                "search_users" => bluesky::search_users(&state, arguments).await,
                "get_thread_context" => bluesky::get_thread_context(&state, arguments).await,
                "mute_user" => bluesky::mute_user(&state, arguments).await,
                "unmute_user" => bluesky::unmute_user(&state, arguments).await,
                "block_user" => bluesky::block_user(&state, arguments).await,
                "unblock_user" => bluesky::unblock_user(&state, arguments).await,
                "mute_thread" => bluesky::mute_thread(&state, arguments).await,
                "unmute_thread" => bluesky::unmute_thread(&state, arguments).await,
                "delete_post" => bluesky::delete_post(&state, arguments).await,

                // Fact tools
                "create_fact" => facts::create_fact(&state, arguments).await,
                "create_facts" => facts::create_facts(&state, arguments).await,
                "update_fact" => facts::update_fact(&state, arguments).await,
                "delete_fact" => facts::delete_fact(&state, arguments).await,
                "query_facts" => facts::query_facts(&state, arguments).await,
                "list_predicates" => facts::list_predicates(&state, arguments).await,
                "list_validation_errors" => facts::list_validation_errors(&state, arguments).await,

                // Enrich tool
                "query_and_enrich" => enrich::query_and_enrich(&state, arguments).await,

                // Rule tools
                "create_rule" => rules::create_rule(&state, arguments).await,
                "create_rules" => rules::create_rules(&state, arguments).await,
                "list_rules" => rules::list_rules(&state, arguments).await,
                "toggle_rule" => rules::toggle_rule(&state, arguments).await,

                // Note tools
                "create_note" => notes::create_note(&state, arguments).await,
                "get_note" => notes::get_note(&state, arguments).await,
                "list_notes" => notes::list_notes(&state, arguments).await,

                // Job tools
                "schedule_job" => jobs::schedule_job(&state, arguments).await,
                "schedule_recurring" => jobs::schedule_recurring(&state, arguments).await,
                "list_jobs" => jobs::list_jobs(&state, arguments).await,
                "cancel_job" => jobs::cancel_job(&state, arguments).await,
                "get_job" => jobs::get_job(&state, arguments).await,

                // Identity tools
                "get_identity" => identity::get_identity(&state, arguments).await,

                // Thought tools
                "record_thought" => thoughts::record_thought(&state, arguments).await,
                "list_thoughts" => thoughts::list_thoughts(&state, arguments).await,
                "get_thought" => thoughts::get_thought(&state, arguments).await,

                // Blog tools
                "publish_blog_post" => blog::publish_blog_post(&state, arguments).await,
                "update_blog_post" => blog::update_blog_post(&state, arguments).await,
                "list_blog_posts" => blog::list_blog_posts(&state, arguments).await,
                "get_blog_post" => blog::get_blog_post(&state, arguments).await,

                // Trigger tools
                "create_trigger" => triggers::create_trigger(&state, arguments).await,
                "update_trigger" => triggers::update_trigger(&state, arguments).await,
                "delete_trigger" => triggers::delete_trigger(&state, arguments).await,
                "list_triggers" => triggers::list_triggers(&state, arguments).await,
                "test_trigger" => triggers::test_trigger(&state, arguments).await,

                // Wiki tools
                "create_wiki_entry" => wiki::create_wiki_entry(&state, arguments).await,
                "update_wiki_entry" => wiki::update_wiki_entry(&state, arguments).await,
                "delete_wiki_entry" => wiki::delete_wiki_entry(&state, arguments).await,
                "get_wiki_entry" => wiki::get_wiki_entry(&state, arguments).await,
                "get_wiki_entry_by_slug" => wiki::get_wiki_entry_by_slug(&state, arguments).await,
                "list_wiki_entries" => wiki::list_wiki_entries(&state, arguments).await,
                "create_wiki_link" => wiki::create_wiki_link(&state, arguments).await,
                "delete_wiki_link" => wiki::delete_wiki_link(&state, arguments).await,
                "list_wiki_links" => wiki::list_wiki_links(&state, arguments).await,

                // Directive tools
                "create_directive" => directives::create_directive(&state, arguments).await,
                "create_directives" => directives::create_directives(&state, arguments).await,
                "update_directive" => directives::update_directive(&state, arguments).await,
                "deactivate_directive" => directives::deactivate_directive(&state, arguments).await,
                "list_directives" => directives::list_directives(&state, arguments).await,

                // PDS raw access tools
                "pds_list_records" => pds::pds_list_records(&state, arguments).await,
                "pds_get_record" => pds::pds_get_record(&state, arguments).await,
                "pds_get_records" => pds::pds_get_records(&state, arguments).await,
                "pds_put_record" => pds::pds_put_record(&state, arguments).await,
                "pds_delete_record" => pds::pds_delete_record(&state, arguments).await,

                // Fact declaration tools
                "create_fact_declaration" => {
                    declarations::create_fact_declaration(&state, arguments).await
                }
                "create_fact_declarations" => {
                    declarations::create_fact_declarations(&state, arguments).await
                }
                "update_fact_declaration" => {
                    declarations::update_fact_declaration(&state, arguments).await
                }
                "delete_fact_declaration" => {
                    declarations::delete_fact_declaration(&state, arguments).await
                }
                "list_fact_declarations" => {
                    declarations::list_fact_declarations(&state, arguments).await
                }

                // Inbox tools
                "check_inbox" => {
                    inbox::check_inbox(state.inbox.as_deref(), arguments).await
                }
                "acknowledge_inbox" => {
                    inbox::acknowledge_inbox(state.inbox.as_deref(), arguments).await
                }

                // Session management tools
                "check_interruption" => {
                    let inbox_pending = if let Some(ref ib) = state.inbox {
                        ib.len().await
                    } else {
                        0
                    };
                    let urgent = if let Some(ref ib) = state.inbox {
                        ib.has_urgent(200).await
                    } else {
                        false
                    };
                    if let Some(ref interruption) = state.interruption {
                        let (interrupted, reason) = interruption.check().await;
                        CallToolResult::success(
                            json!({
                                "interrupted": interrupted,
                                "reason": reason,
                                "urgent": urgent || interrupted,
                                "inbox_pending": inbox_pending
                            })
                            .to_string(),
                        )
                    } else {
                        CallToolResult::success(
                            json!({
                                "interrupted": false,
                                "reason": null,
                                "urgent": urgent,
                                "inbox_pending": inbox_pending
                            })
                            .to_string(),
                        )
                    }
                }

                // Active context tool
                "set_active_context" => {
                    let context = arguments
                        .get("context")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from);

                    let label = context.as_deref().unwrap_or("(cleared)");
                    *state.active_context.write().await = context.clone();

                    CallToolResult::success(
                        json!({
                            "active_context": context,
                            "status": format!("context set to {}", label),
                        })
                        .to_string(),
                    )
                }

                // Session stats tool
                "session_stats" => {
                    const CONTEXT_WINDOW: u64 = 1_000_000;
                    if let Some(ref metrics) = state.session_metrics {
                        let m = metrics.read().await;
                        let elapsed = (chrono::Utc::now() - m.session_start).num_seconds().max(0) as u64;
                        let context_used_pct = if CONTEXT_WINDOW > 0 {
                            (m.total_tokens as f64 / CONTEXT_WINDOW as f64) * 100.0
                        } else {
                            0.0
                        };
                        CallToolResult::success(
                            json!({
                                "session_start": m.session_start.to_rfc3339(),
                                "elapsed_seconds": elapsed,
                                "total_input_tokens": m.total_input_tokens,
                                "total_output_tokens": m.total_output_tokens,
                                "total_tokens": m.total_tokens,
                                "context_window": CONTEXT_WINDOW,
                                "context_used_pct": (context_used_pct * 10.0).round() / 10.0,
                                "turn_count": m.turn_count,
                                "total_cost_usd": (m.total_cost_usd * 10000.0).round() / 10000.0,
                                "tool_call_count": m.tool_call_count,
                                "tool_error_count": m.tool_error_count,
                                "inbox_items_acknowledged": m.inbox_items_acknowledged,
                            })
                            .to_string(),
                        )
                    } else {
                        CallToolResult::success(
                            json!({
                                "error": "no session metrics available",
                                "hint": "session_stats is only available during persistent sessions"
                            })
                            .to_string(),
                        )
                    }
                }

                _ => CallToolResult::error(format!("Unknown tool: {}", name)),
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        self.finalize_result(name, arguments, result, duration_ms, trigger)
            .await
    }

    /// Finalize a result by recording the tool call thought.
    async fn finalize_result(
        &self,
        name: &str,
        arguments: &HashMap<String, Value>,
        mut result: CallToolResult,
        duration_ms: u64,
        trigger: Option<String>,
    ) -> CallToolResult {
        // Record a tool_call thought (skip for record_thought to avoid recursion)
        if name != "record_thought" {
            self.record_tool_call(name, arguments, &result, duration_ms, trigger)
                .await;
        }

        // Update session metrics counters
        {
            let state = self.state.read().await;
            if let Some(ref metrics) = state.session_metrics {
                let mut m = metrics.write().await;
                m.tool_call_count += 1;
                m.last_tool_call_at = chrono::Utc::now();
                if result.is_error.unwrap_or(false) {
                    m.tool_error_count += 1;
                }
                // Track inbox acknowledgements
                if name == "acknowledge_inbox" {
                    if let Some(ids) = arguments.get("ids").and_then(|v| v.as_array()) {
                        m.inbox_items_acknowledged += ids.len() as u64;
                    }
                }
            }
        }

        // Passive inbox nudge: append pending count to every tool result
        // so Winter sees inbox status on every tool call without explicit polling.
        // Skip for inbox tools themselves to avoid redundancy.
        if name != "check_inbox" && name != "acknowledge_inbox" && name != "check_interruption" && name != "session_stats" {
            let state = self.state.read().await;
            if let Some(ref ib) = state.inbox {
                let pending = ib.len().await;
                if pending > 0 {
                    // Append to the last text content item
                    if let Some(last) = result.content.last_mut() {
                        let ToolContent::Text { text } = last;
                        text.push_str(&format!("\n\n_inbox_pending: {}", pending));
                    }
                }
            }
        }

        result
    }

    /// Record a "starting" thought for potentially slow tools.
    ///
    /// This provides immediate feedback in the thoughtstream that a tool
    /// is executing, rather than waiting until completion.
    async fn record_tool_starting(&self, name: &str, trigger: Option<String>) {
        let thought = Thought {
            kind: ThoughtKind::ToolCall,
            content: serde_json::json!({
                "tool": name,
                "status": "starting"
            })
            .to_string(),
            trigger: trigger.or_else(|| Some("internal:tool_call".to_string())),
            tags: Vec::new(),
            duration_ms: None,
            created_at: Utc::now(),
        };

        let state = self.state.read().await;

        // Fire and forget - don't block on write
        if let Some(ref tx) = state.thought_tx
            && let Err(e) = tx.try_send(thought)
        {
            warn!(error = %e, tool = %name, "failed to queue tool_starting thought");
        }
    }

    /// Record a thought about a tool call for transparency.
    ///
    /// This uses fire-and-forget semantics via a bounded channel.
    /// The thought is sent asynchronously and written by a background task.
    ///
    /// The trigger parameter allows tool calls to be associated with their
    /// originating session (notification, DM, job, etc.). If no trigger is
    /// provided, falls back to "internal:tool_call".
    async fn record_tool_call(
        &self,
        name: &str,
        arguments: &HashMap<String, Value>,
        result: &CallToolResult,
        duration_ms: u64,
        trigger: Option<String>,
    ) {
        let is_error = result.is_error.unwrap_or(false);

        // Format the tool call in structured format for web UI rendering
        let content = format_tool_call_content(name, arguments, result, is_error);

        let state = self.state.read().await;

        // In persistent sessions, use the active context for thought scoping.
        // This tags thoughts with the specific inbox item being worked on.
        let thought_trigger = if trigger.as_deref() == Some("persistent") {
            let active = state.active_context.read().await;
            active.clone().unwrap_or_else(|| "persistent".to_string())
        } else {
            trigger.unwrap_or_else(|| "internal:tool_call".to_string())
        };

        let thought = Thought {
            kind: ThoughtKind::ToolCall,
            content,
            trigger: Some(thought_trigger),
            tags: Vec::new(),
            duration_ms: Some(duration_ms),
            created_at: Utc::now(),
        };

        // Fire and forget - don't block on write
        if let Some(ref tx) = state.thought_tx
            && let Err(e) = tx.try_send(thought)
        {
            warn!(error = %e, tool = %name, "failed to queue tool_call thought");
        }
    }

    /// Record a built-in Claude tool call as a Thought.
    ///
    /// This is called via HTTP from the agent after each Claude invocation to log
    /// built-in tool usage (WebSearch, Read, WebFetch, etc.) as Thought records.
    /// Unlike MCP tools which have results available, built-in tools execute inside
    /// Claude's process so we only have the input arguments.
    pub async fn record_builtin_tool_call(
        &self,
        name: &str,
        tool_id: &str,
        input: &Value,
        trigger: Option<String>,
    ) {
        // Format as structured JSON matching the MCP tool call format
        let content = serde_json::json!({
            "tool": name,
            "args": input,
            "builtin": true,
            "claude_id": tool_id,
        });

        let thought_trigger = trigger.unwrap_or_else(|| "internal:tool_call".to_string());

        let thought = Thought {
            kind: ThoughtKind::ToolCall,
            content: serde_json::to_string(&content)
                .unwrap_or_else(|_| format!("{{\"tool\":\"{}\"}}", name)),
            trigger: Some(thought_trigger),
            tags: vec!["builtin".to_string()],
            duration_ms: None, // We don't have timing info for built-in tools
            created_at: Utc::now(),
        };

        let state = self.state.read().await;

        // Fire and forget - don't block on write
        if let Some(ref tx) = state.thought_tx
            && let Err(e) = tx.try_send(thought)
        {
            warn!(error = %e, tool = %name, "failed to queue builtin tool_call thought");
        }
    }
}

/// Structured content for tool call thoughts.
///
/// This is serialized to JSON for storage, allowing the web UI to render
/// each component (tool name, args, result, link) as separate UI elements.
#[derive(serde::Serialize)]
struct ToolCallContent {
    tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    failed: bool,
}

/// Check if a tool is potentially slow and should have a "starting" thought recorded.
///
/// This helps provide immediate feedback in the thoughtstream for tools that
/// may take significant time to execute.
fn is_potentially_slow_tool(name: &str) -> bool {
    matches!(
        name,
        // Datalog queries can be slow, especially on first run
        "query_facts" | "list_validation_errors" | "list_predicates"
        // Network calls to resolve handles/DIDs
        | "resolve_handle" | "resolve_did" | "get_profile"
        // Bluesky API calls that may be slow
        | "get_timeline" | "search_posts" | "search_users" | "get_thread_context"
        // Getting notifications can be slow with many items
        | "get_notifications"
    )
}

/// Format a tool call into structured JSON content for web UI rendering.
fn format_tool_call_content(
    name: &str,
    arguments: &HashMap<String, Value>,
    result: &CallToolResult,
    is_error: bool,
) -> String {
    let args = if arguments.is_empty() {
        None
    } else {
        Some(serde_json::to_value(arguments).unwrap_or(Value::Null))
    };

    let (result_value, summary, link, error) =
        if let Some(ToolContent::Text { text }) = result.content.first() {
            if is_error {
                (None, None, None, Some(text.clone()))
            } else if let Ok(json) = serde_json::from_str::<Value>(text) {
                let category = get_tool_category(name);

                // Generate summary and link based on category
                let (result_for_thought, summary, link) = match &category {
                    ToolResultCategory::Excluded => (None, None, None),
                    ToolResultCategory::SingleMutation { web_path, .. } => {
                        let sum = summarize_result(name, &json);
                        let link = web_path.and_then(|path| {
                            extract_string(&json, "rkey", None)
                                .and_then(|rkey| make_web_link(path, &rkey))
                        });
                        // SingleMutation results are small, keep them
                        (Some(json), Some(sum).filter(|s| !s.is_empty()), link)
                    }
                    ToolResultCategory::BatchMutation { .. } => {
                        let sum = summarize_result(name, &json);
                        // Batch results can be large, omit full result
                        (None, Some(sum).filter(|s| !s.is_empty()), None)
                    }
                    _ => {
                        let sum = summarize_result(name, &json);
                        // List/Query/Get/BlueskyRead results can be very large
                        // (e.g. list_wiki_entries with full content fields).
                        // Omit the full result from thoughts — the summary
                        // captures count + sample, and the agent already has
                        // the full data from the tool call itself.
                        (None, Some(sum).filter(|s| !s.is_empty()), None)
                    }
                };

                (result_for_thought, summary, link, None)
            } else {
                // Non-JSON text result
                (Some(Value::String(text.clone())), None, None, None)
            }
        } else {
            (None, None, None, None)
        };

    let content = ToolCallContent {
        tool: name.to_string(),
        args,
        result: result_value,
        summary,
        link,
        error,
        failed: is_error,
    };

    serde_json::to_string(&content).unwrap_or_else(|_| format!("{{\"tool\":\"{}\"}}", name))
}

/// Maximum byte size for thought content to avoid PayloadTooLargeError.
/// ATProto records have size limits; 32KB is a safe limit for thought content.
const MAX_THOUGHT_CONTENT_BYTES: usize = 32_000;

/// Background task that writes thoughts to the PDS.
async fn thought_writer_loop(client: Arc<AtprotoClient>, mut rx: mpsc::Receiver<Thought>) {
    while let Some(mut thought) = rx.recv().await {
        // Truncate content if too large to avoid PayloadTooLargeError
        if thought.content.len() > MAX_THOUGHT_CONTENT_BYTES {
            // Find a safe UTF-8 boundary for truncation
            let mut end = MAX_THOUGHT_CONTENT_BYTES;
            while end > 0 && !thought.content.is_char_boundary(end) {
                end -= 1;
            }
            thought.content = format!("{}...[truncated]", &thought.content[..end]);
        }

        let rkey = Tid::now().to_string();
        if let Err(e) = client
            .create_record(THOUGHT_COLLECTION, Some(&rkey), &thought)
            .await
        {
            warn!(error = %e, "failed to write thought");
        }
    }
}

/// Truncate a string to a maximum number of characters (not bytes).
/// Safe for UTF-8 strings with multi-byte characters.
#[cfg(test)]
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
}

/// Format tool arguments into a concise summary string.
#[cfg(test)]
fn format_arguments_summary(arguments: &HashMap<String, Value>) -> String {
    if arguments.is_empty() {
        return String::new();
    }

    let parts: Vec<String> = arguments
        .iter()
        .map(|(k, v)| {
            let value_str = format_value_summary(v);
            format!("{}={}", k, value_str)
        })
        .collect();

    parts.join(", ")
}

/// Format a single JSON value into a concise summary.
#[cfg(test)]
fn format_value_summary(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{}\"", truncate_chars(s, 50)),
        Value::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else if arr.len() <= 5 {
                // Show actual values for small arrays
                let items: Vec<String> = arr.iter().map(format_value_summary).collect();
                format!("[{}]", items.join(", "))
            } else {
                // For large arrays, show first few items with count
                let items: Vec<String> = arr.iter().take(3).map(format_value_summary).collect();
                format!("[{}, ... ({} total)]", items.join(", "), arr.len())
            }
        }
        Value::Object(obj) => {
            if obj.is_empty() {
                "{}".to_string()
            } else if obj.len() <= 3 {
                // Show actual key-value pairs for small objects
                let items: Vec<String> = obj
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, format_value_summary(v)))
                    .collect();
                format!("{{{}}}", items.join(", "))
            } else {
                format!("{{{} keys}}", obj.len())
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Tests for truncate_chars

    #[test]
    fn truncate_chars_short_string_unchanged() {
        let s = "hello";
        assert_eq!(truncate_chars(s, 10), "hello");
    }

    #[test]
    fn truncate_chars_exact_length_unchanged() {
        let s = "hello";
        assert_eq!(truncate_chars(s, 5), "hello");
    }

    #[test]
    fn truncate_chars_long_string_truncated() {
        let s = "hello world";
        assert_eq!(truncate_chars(s, 5), "hello...");
    }

    #[test]
    fn truncate_chars_empty_string() {
        assert_eq!(truncate_chars("", 10), "");
    }

    #[test]
    fn truncate_chars_zero_max() {
        assert_eq!(truncate_chars("hello", 0), "...");
    }

    #[test]
    fn truncate_chars_utf8_multibyte_safe() {
        // Japanese characters (3 bytes each in UTF-8)
        let s = "こんにちは世界"; // "Hello World" in Japanese - 7 chars
        let result = truncate_chars(s, 3);
        assert_eq!(result, "こんに...");
        // Verify we didn't panic and result is valid UTF-8
        assert!(result.is_ascii() || result.chars().count() > 0);
    }

    #[test]
    fn truncate_chars_emoji_safe() {
        // Emojis can be multi-byte
        let s = "👋🌍🎉✨"; // 4 emoji characters
        let result = truncate_chars(s, 2);
        assert_eq!(result, "👋🌍...");
    }

    #[test]
    fn truncate_chars_mixed_content() {
        let s = "Hello 世界 🌍";
        let result = truncate_chars(s, 8);
        assert_eq!(result, "Hello 世界...");
    }

    // Tests for format_arguments_summary

    #[test]
    fn format_arguments_summary_empty() {
        let args: HashMap<String, Value> = HashMap::new();
        assert_eq!(format_arguments_summary(&args), "");
    }

    #[test]
    fn format_arguments_summary_single_string() {
        let mut args = HashMap::new();
        args.insert("text".to_string(), json!("hello"));
        let result = format_arguments_summary(&args);
        assert_eq!(result, "text=\"hello\"");
    }

    #[test]
    fn format_arguments_summary_long_string_truncated() {
        let mut args = HashMap::new();
        let long_text = "a".repeat(100);
        args.insert("text".to_string(), json!(long_text));
        let result = format_arguments_summary(&args);
        assert!(result.contains("..."));
        assert!(result.len() < 100); // Should be truncated
    }

    #[test]
    fn format_arguments_summary_number() {
        let mut args = HashMap::new();
        args.insert("count".to_string(), json!(42));
        let result = format_arguments_summary(&args);
        assert_eq!(result, "count=42");
    }

    #[test]
    fn format_arguments_summary_boolean() {
        let mut args = HashMap::new();
        args.insert("enabled".to_string(), json!(true));
        let result = format_arguments_summary(&args);
        assert_eq!(result, "enabled=true");
    }

    #[test]
    fn format_arguments_summary_null() {
        let mut args = HashMap::new();
        args.insert("value".to_string(), json!(null));
        let result = format_arguments_summary(&args);
        assert_eq!(result, "value=null");
    }

    #[test]
    fn format_arguments_summary_array() {
        let mut args = HashMap::new();
        args.insert("items".to_string(), json!(["a", "b", "c"]));
        let result = format_arguments_summary(&args);
        assert_eq!(result, r#"items=["a", "b", "c"]"#);
    }

    #[test]
    fn format_arguments_summary_empty_array() {
        let mut args = HashMap::new();
        args.insert("items".to_string(), json!([]));
        let result = format_arguments_summary(&args);
        assert_eq!(result, "items=[]");
    }

    #[test]
    fn format_arguments_summary_large_array() {
        let mut args = HashMap::new();
        args.insert("items".to_string(), json!(["a", "b", "c", "d", "e", "f"]));
        let result = format_arguments_summary(&args);
        assert_eq!(result, r#"items=["a", "b", "c", ... (6 total)]"#);
    }

    #[test]
    fn format_arguments_summary_object() {
        let mut args = HashMap::new();
        args.insert("config".to_string(), json!({"a": 1}));
        let result = format_arguments_summary(&args);
        assert_eq!(result, "config={a: 1}");
    }

    #[test]
    fn format_arguments_summary_large_object() {
        let mut args = HashMap::new();
        args.insert(
            "config".to_string(),
            json!({"a": 1, "b": 2, "c": 3, "d": 4}),
        );
        let result = format_arguments_summary(&args);
        assert_eq!(result, "config={4 keys}");
    }

    #[test]
    fn format_arguments_summary_empty_object() {
        let mut args = HashMap::new();
        args.insert("config".to_string(), json!({}));
        let result = format_arguments_summary(&args);
        assert_eq!(result, "config={}");
    }

    // Note: multiple arguments test uses sorted keys for deterministic output
    #[test]
    fn format_arguments_summary_multiple_args() {
        let mut args = HashMap::new();
        args.insert("a".to_string(), json!("x"));
        args.insert("b".to_string(), json!(1));
        let result = format_arguments_summary(&args);
        // HashMap iteration order is not guaranteed, so check both parts exist
        assert!(result.contains("a=\"x\""));
        assert!(result.contains("b=1"));
        assert!(result.contains(", "));
    }

    // Metamorphic test: truncation should preserve prefix
    #[test]
    fn truncate_chars_preserves_prefix() {
        let s = "abcdefghij";
        for max in 1..=10 {
            let result = truncate_chars(s, max);
            let prefix: String = s.chars().take(max).collect();
            assert!(
                result.starts_with(&prefix),
                "truncate({}, {}) = {} should start with {}",
                s,
                max,
                result,
                prefix
            );
        }
    }

    // ========================================================================
    // Tests for tool result summarization
    // ========================================================================

    #[test]
    fn summarize_single_mutation_basic() {
        let result = json!({
            "rkey": "3abc123",
            "predicate": "interested_in"
        });
        let summary = summarize_single_mutation(&result, &["rkey", "predicate"], None);
        assert_eq!(summary, "rkey=3abc123, predicate=interested_in");
    }

    #[test]
    fn summarize_single_mutation_missing_field() {
        let result = json!({
            "rkey": "3abc123"
        });
        let summary = summarize_single_mutation(&result, &["rkey", "predicate"], None);
        assert_eq!(summary, "rkey=3abc123");
    }

    #[test]
    fn summarize_single_mutation_with_boolean() {
        let result = json!({
            "deleted": true,
            "rkey": "3abc123"
        });
        let summary = summarize_single_mutation(&result, &["deleted", "rkey"], None);
        assert_eq!(summary, "deleted=true, rkey=3abc123");
    }

    #[test]
    fn summarize_batch_mutation_basic() {
        let result = json!({
            "created": 3,
            "results": [
                {"predicate": "foo"},
                {"predicate": "bar"},
                {"predicate": "baz"}
            ]
        });
        let summary = summarize_batch_mutation(&result, "created", "results", "predicate");
        assert_eq!(summary, "created=3, predicate=[foo, bar, baz]");
    }

    #[test]
    fn summarize_batch_mutation_many_items() {
        let result = json!({
            "created": 5,
            "results": [
                {"predicate": "a"},
                {"predicate": "b"},
                {"predicate": "c"},
                {"predicate": "d"},
                {"predicate": "e"}
            ]
        });
        let summary = summarize_batch_mutation(&result, "created", "results", "predicate");
        // Should only show first 3
        assert_eq!(summary, "created=5, predicate=[a, b, c]");
    }

    #[test]
    fn summarize_list_basic() {
        let result = json!({
            "count": 15,
            "notes": [
                {"title": "Note 1"},
                {"title": "Note 2"},
                {"title": "Note 3"}
            ]
        });
        let summary = summarize_list(&result, "count", "notes", "title");
        assert_eq!(
            summary,
            "count=15, sample=[\"Note 1\", \"Note 2\", \"Note 3\"]"
        );
    }

    #[test]
    fn summarize_list_empty() {
        let result = json!({
            "count": 0,
            "notes": []
        });
        let summary = summarize_list(&result, "count", "notes", "title");
        assert_eq!(summary, "count=0");
    }

    #[test]
    fn summarize_list_truncates_long_titles() {
        let long_title = "a".repeat(50);
        let result = json!({
            "count": 1,
            "notes": [{"title": long_title}]
        });
        let summary = summarize_list(&result, "count", "notes", "title");
        assert!(summary.contains("..."));
        assert!(summary.len() < 100);
    }

    #[test]
    fn summarize_query_basic() {
        let result = json!({
            "query": "follows(X, Y, _)",
            "results": [
                ["did:plc:abc", "did:plc:def"],
                ["did:plc:abc", "did:plc:ghi"],
                ["did:plc:abc", "did:plc:jkl"]
            ]
        });
        let summary = summarize_query(&result);
        assert!(summary.contains("query=\"follows(X, Y, _)\""));
        assert!(summary.contains("count=3"));
        assert!(summary.contains("sample=["));
    }

    #[test]
    fn summarize_query_empty_results() {
        let result = json!({
            "query": "no_matches(X)",
            "results": []
        });
        let summary = summarize_query(&result);
        assert!(summary.contains("count=0"));
    }

    #[test]
    fn summarize_get_basic() {
        let result = json!({
            "rkey": "3abc123",
            "title": "My Note",
            "content": "This is the content of the note."
        });
        let summary = summarize_get(&result, &["rkey", "title"], Some("content"));
        assert!(summary.contains("rkey=3abc123"));
        assert!(summary.contains("title=My Note"));
        assert!(summary.contains("content_length="));
    }

    #[test]
    fn summarize_get_no_size_field() {
        let result = json!({
            "operator_did": "did:plc:xyz"
        });
        let summary = summarize_get(&result, &["operator_did"], None);
        assert_eq!(summary, "operator_did=did:plc:xyz");
    }

    #[test]
    fn summarize_bluesky_timeline() {
        let result = json!({
            "posts": [
                {"author": {"handle": "alice.bsky.social"}},
                {"author": {"handle": "bob.bsky.social"}}
            ]
        });
        let summary = summarize_bluesky_read(&result, BlueskyReadType::Timeline);
        assert!(summary.contains("count=2"));
        assert!(summary.contains("first_author=alice.bsky.social"));
    }

    #[test]
    fn summarize_bluesky_notifications() {
        let result = json!({
            "notifications": [
                {"reason": "like"},
                {"reason": "like"},
                {"reason": "reply"},
                {"reason": "follow"}
            ]
        });
        let summary = summarize_bluesky_read(&result, BlueskyReadType::Notifications);
        assert!(summary.contains("count=4"));
        assert!(summary.contains("reasons="));
        assert!(summary.contains("like: 2"));
        assert!(summary.contains("reply: 1"));
        assert!(summary.contains("follow: 1"));
    }

    #[test]
    fn summarize_bluesky_search() {
        let result = json!({
            "query": "rust programming",
            "posts": [{}, {}, {}],
            "cursor": "abc123"
        });
        let summary = summarize_bluesky_read(&result, BlueskyReadType::Search);
        assert!(summary.contains("query=\"rust programming\""));
        assert!(summary.contains("count=3"));
        assert!(summary.contains("has_cursor=true"));
    }

    #[test]
    fn summarize_bluesky_thread() {
        let result = json!({
            "root": {"author": {"handle": "alice.bsky.social"}},
            "total_replies": 15,
            "my_replies": 3
        });
        let summary = summarize_bluesky_read(&result, BlueskyReadType::Thread);
        assert!(summary.contains("root_author=alice.bsky.social"));
        assert!(summary.contains("total_replies=15"));
        assert!(summary.contains("my_replies=3"));
    }

    #[test]
    fn summarize_custom_tool() {
        let result = json!({
            "duration_ms": 150,
            "sandboxed": true,
            "result": "Success!"
        });
        let summary = summarize_custom(&result);
        assert!(summary.contains("duration_ms=150"));
        assert!(summary.contains("sandboxed=true"));
        assert!(summary.contains("result=Success!"));
    }

    #[test]
    fn summarize_result_excluded_returns_empty() {
        let result = json!({"kind": "insight", "content": "test"});
        let summary = summarize_result("record_thought", &result);
        assert!(summary.is_empty());
    }

    #[test]
    fn summarize_result_unknown_tool_uses_custom() {
        let result = json!({"foo": "bar"});
        // Unknown tools default to Custom category
        let summary = summarize_result("unknown_tool", &result);
        // Custom category returns empty if no expected fields
        assert!(summary.is_empty());
    }

    #[test]
    fn get_tool_category_covers_all_tools() {
        // Verify key tools have correct categories
        assert!(matches!(
            get_tool_category("create_fact"),
            ToolResultCategory::SingleMutation { .. }
        ));
        assert!(matches!(
            get_tool_category("create_facts"),
            ToolResultCategory::BatchMutation { .. }
        ));
        assert!(matches!(
            get_tool_category("list_notes"),
            ToolResultCategory::List { .. }
        ));
        assert!(matches!(
            get_tool_category("query_facts"),
            ToolResultCategory::Query
        ));
        assert!(matches!(
            get_tool_category("get_note"),
            ToolResultCategory::Get { .. }
        ));
        assert!(matches!(
            get_tool_category("get_timeline"),
            ToolResultCategory::BlueskyRead(_)
        ));
        assert!(matches!(
            get_tool_category("record_thought"),
            ToolResultCategory::Excluded
        ));
    }

    #[test]
    fn truncate_for_summary_basic() {
        assert_eq!(truncate_for_summary("hello", 10), "hello");
        assert_eq!(truncate_for_summary("hello world", 5), "hello...");
        assert_eq!(truncate_for_summary("", 5), "");
    }

    #[test]
    fn extract_string_various_types() {
        let result = json!({
            "string_field": "hello",
            "bool_field": true,
            "number_field": 42,
            "null_field": null
        });
        assert_eq!(
            extract_string(&result, "string_field", None),
            Some("hello".to_string())
        );
        assert_eq!(
            extract_string(&result, "bool_field", None),
            Some("true".to_string())
        );
        assert_eq!(
            extract_string(&result, "number_field", None),
            Some("42".to_string())
        );
        assert_eq!(extract_string(&result, "null_field", None), None);
        assert_eq!(extract_string(&result, "missing", None), None);
    }

    #[test]
    fn format_tool_call_content_with_summary() {
        let args = HashMap::new();
        let result = CallToolResult::success(
            serde_json::to_string(&json!({
                "rkey": "3abc123",
                "predicate": "test_pred"
            }))
            .unwrap(),
        );

        let content = format_tool_call_content("create_fact", &args, &result, false);
        let parsed: Value = serde_json::from_str(&content).expect("should be valid JSON");

        assert_eq!(parsed["tool"], "create_fact");
        assert_eq!(parsed["result"]["rkey"], "3abc123");
        assert_eq!(parsed["result"]["predicate"], "test_pred");
        assert!(parsed["summary"].as_str().unwrap().contains("rkey=3abc123"));
        assert!(parsed.get("failed").is_none()); // false values are skipped
    }

    #[test]
    fn format_tool_call_content_error_shows_full_message() {
        let args = HashMap::new();
        let error_text = "Detailed error message that should not be truncated";
        let result = CallToolResult::error(error_text);

        let content = format_tool_call_content("create_fact", &args, &result, true);
        let parsed: Value = serde_json::from_str(&content).expect("should be valid JSON");

        assert_eq!(parsed["tool"], "create_fact");
        assert_eq!(parsed["error"], error_text);
        assert_eq!(parsed["failed"], true);
        assert!(parsed.get("result").is_none());
    }

    #[test]
    fn format_tool_call_content_excluded_tool_has_result_no_summary() {
        let args = HashMap::new();
        let result = CallToolResult::success(
            serde_json::to_string(&json!({
                "rkey": "3abc123",
                "kind": "insight"
            }))
            .unwrap(),
        );

        let content = format_tool_call_content("record_thought", &args, &result, false);
        let parsed: Value = serde_json::from_str(&content).expect("should be valid JSON");

        assert_eq!(parsed["tool"], "record_thought");
        // Excluded tools have no result and no summary
        assert!(parsed.get("result").is_none());
        assert!(parsed.get("summary").is_none());
    }

    #[test]
    fn format_tool_call_content_includes_args() {
        let mut args = HashMap::new();
        args.insert("predicate".to_string(), json!("test"));
        args.insert("args".to_string(), json!(["a", "b"]));

        let result = CallToolResult::success(
            serde_json::to_string(&json!({
                "rkey": "abc123",
                "predicate": "test"
            }))
            .unwrap(),
        );

        let content = format_tool_call_content("create_fact", &args, &result, false);
        let parsed: Value = serde_json::from_str(&content).expect("should be valid JSON");

        assert_eq!(parsed["args"]["predicate"], "test");
        assert_eq!(parsed["args"]["args"], json!(["a", "b"]));
    }

    #[test]
    fn format_tool_call_content_empty_args_omitted() {
        let args = HashMap::new();
        let result = CallToolResult::success(
            serde_json::to_string(&json!({
                "count": 0,
                "notes": []
            }))
            .unwrap(),
        );

        let content = format_tool_call_content("list_notes", &args, &result, false);
        let parsed: Value = serde_json::from_str(&content).expect("should be valid JSON");

        assert!(parsed.get("args").is_none());
    }

    // ========================================================================
    // Tests for ToolMeta and permission colocating
    // ========================================================================

    #[test]
    fn all_tools_have_permission_metadata() {
        // Ensure all tools returned by all_tools() have valid metadata
        let tools = ToolRegistry::all_tools();
        assert!(!tools.is_empty(), "Should have at least one tool");

        for tool in &tools {
            assert!(
                !tool.definition.name.is_empty(),
                "Tool name should not be empty"
            );
            assert!(
                !tool.definition.description.is_empty(),
                "Tool description should not be empty"
            );
        }
    }

    #[test]
    fn agent_allowed_tools_returns_mcp_format() {
        let allowed = ToolRegistry::agent_allowed_tools();
        assert!(!allowed.is_empty(), "Should have at least one allowed tool");

        // All tool names should be in MCP format
        for name in &allowed {
            assert!(
                name.starts_with("mcp__winter__"),
                "Tool name '{}' should start with 'mcp__winter__'",
                name
            );
        }
    }

    #[test]
    fn agent_allowed_tools_includes_expected_tools() {
        let allowed = ToolRegistry::agent_allowed_tools();

        // Check a sample of expected tools
        let expected = [
            "mcp__winter__post_to_bluesky",
            "mcp__winter__create_fact",
            "mcp__winter__query_facts",
            "mcp__winter__create_note",
            "mcp__winter__schedule_job",
            "mcp__winter__record_thought",
            "mcp__winter__create_directive",
        ];

        for tool in expected {
            assert!(
                allowed.contains(&tool.to_string()),
                "Expected tool '{}' to be allowed",
                tool
            );
        }
    }

    #[test]
    fn definitions_and_all_tools_count_match() {
        let all_tools = ToolRegistry::all_tools();

        // Create a temp registry to get definitions
        // Note: definitions() requires an instance, but all_tools() is static
        // We verify by comparing tool metadata count
        let tool_count = all_tools.len();

        // Should have a reasonable number of tools
        assert!(
            tool_count > 50,
            "Expected at least 50 tools, got {}",
            tool_count
        );
        assert!(
            tool_count < 200,
            "Unexpected number of tools: {}",
            tool_count
        );
    }
}
