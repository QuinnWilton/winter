//! Tool definitions and implementations for the MCP server.

mod blog;
mod bluesky;
mod facts;
mod identity;
mod jobs;
mod notes;
mod rules;
mod thoughts;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use tokio::sync::{RwLock, mpsc};
use tracing::warn;

use crate::bluesky::BlueskyClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{AtprotoClient, RepoCache, Thought, ThoughtKind, Tid};
use winter_datalog::DatalogCache;

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
    /// Channel for async thought recording (fire-and-forget).
    pub thought_tx: Option<mpsc::Sender<Thought>>,
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
                thought_tx: Some(thought_tx),
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

        defs
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, arguments: &HashMap<String, Value>) -> CallToolResult {
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

                // Fact tools
                "create_fact" => facts::create_fact(&state, arguments).await,
                "update_fact" => facts::update_fact(&state, arguments).await,
                "delete_fact" => facts::delete_fact(&state, arguments).await,
                "query_facts" => facts::query_facts(&state, arguments).await,

                // Rule tools
                "create_rule" => rules::create_rule(&state, arguments).await,
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

                // Identity tools
                "get_identity" => identity::get_identity(&state, arguments).await,
                "update_identity" => identity::update_identity(&state, arguments).await,

                // Thought tools
                "record_thought" => thoughts::record_thought(&state, arguments).await,

                // Blog tools
                "publish_blog_post" => blog::publish_blog_post(&state, arguments).await,
                "update_blog_post" => blog::update_blog_post(&state, arguments).await,
                "list_blog_posts" => blog::list_blog_posts(&state, arguments).await,

                _ => CallToolResult::error(format!("Unknown tool: {}", name)),
            }
        };

        // Record a tool_call thought (skip for record_thought to avoid recursion)
        if name != "record_thought" {
            self.record_tool_call(name, arguments, &result).await;
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
    ) {
        let is_error = result.is_error.unwrap_or(false);

        // Format arguments concisely
        let args_summary = format_arguments_summary(arguments);

        let content = if is_error {
            format!("Called {} [{}] - FAILED", name, args_summary)
        } else {
            format!("Called {} [{}]", name, args_summary)
        };

        let thought = Thought {
            kind: ThoughtKind::ToolCall,
            content,
            trigger: None,
            duration_ms: None,
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
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
}

/// Format tool arguments into a concise summary string.
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
}
