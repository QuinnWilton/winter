//! Tool definitions and implementations for the MCP server.

mod blog;
mod bluesky;
mod custom_tools;
mod declarations;
mod directives;
mod facts;
mod identity;
mod jobs;
mod notes;
mod pds;
mod rules;
mod thoughts;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use serde_json::Value;
use tokio::sync::{RwLock, mpsc};
use tracing::warn;

use crate::bluesky::BlueskyClient;
use crate::deno::DenoExecutor;
use crate::protocol::{CallToolResult, ToolContent, ToolDefinition};
use crate::secrets::SecretManager;
use winter_atproto::{AtprotoClient, RepoCache, Thought, ThoughtKind, Tid};
use winter_datalog::{DatalogCache, DatalogCoordinatorHandle};

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
        "list_predicates" => List {
            count_field: "total",
            items_field: "predicates",
            sample_key: "name",
        },

        // === Query ===
        "query_facts" => Query,

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

/// Truncate a string for summary display.
fn truncate_for_summary(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
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
    if let Some(path) = web_path {
        if let Some(rkey) = extract_string(result, "rkey", None) {
            if let Some(link) = make_web_link(path, &rkey) {
                summary.push_str(&format!("\nView: {}", link));
            }
        }
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
                    extract_string(item, sample_key, Some(30))
                        .map(|s| format!("\"{}\"", s))
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
    if let Some(field) = size_field {
        if let Some(Value::String(content)) = result.get(field) {
            parts.push(format!("{}_length={}", field, content.len()));
        }
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
                parts.push(format!("root_author={}", truncate_for_summary(root_author, 25)));
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

/// Shared state for tools.
pub struct ToolState {
    pub atproto: Arc<AtprotoClient>,
    pub bluesky: Option<BlueskyClient>,
    /// In-memory cache for facts and rules (optional).
    pub cache: Option<Arc<RepoCache>>,
    /// Datalog query cache for efficient query execution (optional).
    pub datalog_cache: Option<Arc<DatalogCache>>,
    /// Handle to the datalog coordinator for serialized TSV access (optional).
    /// When set, queries go through the coordinator instead of direct cache access.
    pub datalog_coordinator: Option<DatalogCoordinatorHandle>,
    /// Channel for async thought recording (fire-and-forget).
    pub thought_tx: Option<mpsc::Sender<Thought>>,
    /// Secret manager for custom tool secrets (optional).
    pub secrets: Option<Arc<RwLock<SecretManager>>>,
    /// Deno executor for custom tool sandboxing (optional).
    pub deno: Option<DenoExecutor>,
}

/// Registry of available tools.
pub struct ToolRegistry {
    state: Arc<RwLock<ToolState>>,
}

impl ToolRegistry {
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
                datalog_coordinator: None,
                thought_tx: Some(thought_tx),
                secrets: None,
                deno: None,
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
                datalog_coordinator: None,
                thought_tx: Some(thought_tx),
                secrets: None,
                deno: None,
            })),
        }
    }

    /// Set the datalog cache asynchronously.
    pub async fn set_datalog_cache(&self, datalog_cache: Arc<DatalogCache>) {
        let mut guard = self.state.write().await;
        guard.datalog_cache = Some(datalog_cache);
    }

    /// Set the datalog coordinator handle for serialized TSV access.
    pub async fn set_datalog_coordinator(&self, coordinator: DatalogCoordinatorHandle) {
        let mut guard = self.state.write().await;
        guard.datalog_coordinator = Some(coordinator);
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

    /// Get all tool definitions.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = Vec::new();

        // Bluesky tools
        defs.extend(bluesky::definitions());

        // Fact tools
        defs.extend(facts::definitions());

        // Rule tools
        defs.extend(rules::definitions());

        // Note tools
        defs.extend(notes::definitions());

        // Job tools
        defs.extend(jobs::definitions());

        // Identity tools
        defs.extend(identity::definitions());

        // Thought tools
        defs.extend(thoughts::definitions());

        // Blog tools
        defs.extend(blog::definitions());

        // Custom tools
        defs.extend(custom_tools::definitions());

        // Directive tools
        defs.extend(directives::definitions());

        // Fact declaration tools
        defs.extend(declarations::definitions());

        // PDS raw access tools
        defs.extend(pds::definitions());

        defs
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, arguments: &HashMap<String, Value>) -> CallToolResult {
        let start = Instant::now();

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
                return self.finalize_result(name, arguments, result, duration_ms).await;
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

                // Fact tools
                "create_fact" => facts::create_fact(&state, arguments).await,
                "create_facts" => facts::create_facts(&state, arguments).await,
                "update_fact" => facts::update_fact(&state, arguments).await,
                "delete_fact" => facts::delete_fact(&state, arguments).await,
                "query_facts" => facts::query_facts(&state, arguments).await,
                "list_predicates" => facts::list_predicates(&state, arguments).await,

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

                // Directive tools
                "create_directive" => directives::create_directive(&state, arguments).await,
                "create_directives" => directives::create_directives(&state, arguments).await,
                "update_directive" => directives::update_directive(&state, arguments).await,
                "deactivate_directive" => directives::deactivate_directive(&state, arguments).await,
                "list_directives" => directives::list_directives(&state, arguments).await,

                // PDS raw access tools
                "pds_list_records" => pds::pds_list_records(&state, arguments).await,
                "pds_get_record" => pds::pds_get_record(&state, arguments).await,
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

                _ => CallToolResult::error(format!("Unknown tool: {}", name)),
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        self.finalize_result(name, arguments, result, duration_ms).await
    }

    /// Finalize a result by recording the tool call thought.
    async fn finalize_result(
        &self,
        name: &str,
        arguments: &HashMap<String, Value>,
        result: CallToolResult,
        duration_ms: u64,
    ) -> CallToolResult {
        // Record a tool_call thought (skip for record_thought to avoid recursion)
        if name != "record_thought" {
            self.record_tool_call(name, arguments, &result, duration_ms).await;
        }

        result
    }

    /// Record a thought about a tool call for transparency.
    ///
    /// This uses fire-and-forget semantics via a bounded channel.
    /// The thought is sent asynchronously and written by a background task.
    async fn record_tool_call(
        &self,
        name: &str,
        arguments: &HashMap<String, Value>,
        result: &CallToolResult,
        duration_ms: u64,
    ) {
        let is_error = result.is_error.unwrap_or(false);

        // Format the tool call in structured format for web UI rendering
        let content = format_tool_call_content(name, arguments, result, is_error);

        // Use static trigger for tool calls - they're recorded for debugging
        // but shouldn't appear in any conversation context (not relevant to
        // notification handling, DM responses, or awaken reflections)
        let thought = Thought {
            kind: ThoughtKind::ToolCall,
            content,
            trigger: Some("internal:tool_call".to_string()),
            tags: Vec::new(),
            duration_ms: Some(duration_ms),
            created_at: Utc::now(),
        };

        let state = self.state.read().await;

        // Fire and forget - don't block on write
        if let Some(ref tx) = state.thought_tx {
            if let Err(e) = tx.try_send(thought) {
                warn!(error = %e, tool = %name, "failed to queue tool_call thought");
            }
        }
    }
}

/// Format a tool call into structured content for web UI rendering.
fn format_tool_call_content(
    name: &str,
    arguments: &HashMap<String, Value>,
    result: &CallToolResult,
    is_error: bool,
) -> String {
    let mut content = format!("Called {}", name);
    if is_error {
        content.push_str(" - FAILED");
    }
    content.push('\n');

    // Pretty-print arguments as JSON
    if !arguments.is_empty() {
        let args_json = serde_json::to_value(arguments).unwrap_or(Value::Null);
        if let Ok(pretty) = serde_json::to_string_pretty(&args_json) {
            content.push_str("Args:\n");
            content.push_str(&pretty);
            content.push('\n');
        }
    }

    // Include result summary (or full error)
    if let Some(ToolContent::Text { text }) = result.content.first() {
        if is_error {
            // Show full error message
            content.push_str("Error:\n");
            content.push_str(text);
        } else if let Ok(json) = serde_json::from_str::<Value>(text) {
            // Generate category-based summary
            let summary = summarize_result(name, &json);
            if !summary.is_empty() {
                content.push_str("Result: ");
                content.push_str(&summary);
            }
        }
    }

    content
}

/// Background task that writes thoughts to the PDS.
async fn thought_writer_loop(client: Arc<AtprotoClient>, mut rx: mpsc::Receiver<Thought>) {
    while let Some(thought) = rx.recv().await {
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
        assert_eq!(summary, "count=15, sample=[\"Note 1\", \"Note 2\", \"Note 3\"]");
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
        assert_eq!(extract_string(&result, "string_field", None), Some("hello".to_string()));
        assert_eq!(extract_string(&result, "bool_field", None), Some("true".to_string()));
        assert_eq!(extract_string(&result, "number_field", None), Some("42".to_string()));
        assert_eq!(extract_string(&result, "null_field", None), None);
        assert_eq!(extract_string(&result, "missing", None), None);
    }

    #[test]
    fn format_tool_call_content_with_summary() {
        let args = HashMap::new();
        let result = CallToolResult::success(serde_json::to_string(&json!({
            "rkey": "3abc123",
            "predicate": "test_pred"
        })).unwrap());

        let content = format_tool_call_content("create_fact", &args, &result, false);
        assert!(content.contains("Called create_fact"));
        assert!(content.contains("Result: rkey=3abc123, predicate=test_pred"));
    }

    #[test]
    fn format_tool_call_content_error_shows_full_message() {
        let args = HashMap::new();
        let error_text = "Detailed error message that should not be truncated";
        let result = CallToolResult::error(error_text);

        let content = format_tool_call_content("create_fact", &args, &result, true);
        assert!(content.contains("Called create_fact - FAILED"));
        assert!(content.contains("Error:"));
        assert!(content.contains(error_text));
    }

    #[test]
    fn format_tool_call_content_excluded_tool_no_result() {
        let args = HashMap::new();
        let result = CallToolResult::success(serde_json::to_string(&json!({
            "rkey": "3abc123",
            "kind": "insight"
        })).unwrap());

        let content = format_tool_call_content("record_thought", &args, &result, false);
        assert!(content.contains("Called record_thought"));
        assert!(!content.contains("Result:"));
    }
}
