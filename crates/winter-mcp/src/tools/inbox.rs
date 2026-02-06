//! Inbox infrastructure for the persistent session model.
//!
//! The inbox is an in-memory collection of items (notifications, DMs, jobs, system messages)
//! that the daemon pushes and Winter polls via MCP tools. Items persist until explicitly
//! acknowledged.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use winter_atproto::{Facet, Tid};

use crate::protocol::{CallToolResult, ToolDefinition};

use super::ToolMeta;

// ============================================================================
// Data Structures
// ============================================================================

/// A single inbox item representing work for Winter to consider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxItem {
    /// Unique ID (TID-based).
    pub id: String,
    /// What kind of item this is.
    pub kind: InboxItemKind,
    /// Priority hint (200=operator DM, 100=notification, 50=job).
    pub priority: u8,
    /// When this item was added to the inbox.
    pub created_at: DateTime<Utc>,
    /// Pre-computed tag for thought scoping (e.g., "notification:{uri}:root={root}").
    pub context_tag: String,
    /// Full type-specific data.
    pub payload: InboxPayload,
}

/// The kind of inbox item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxItemKind {
    Notification,
    DirectMessage,
    Job,
    System,
}

impl std::fmt::Display for InboxItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Notification => write!(f, "notification"),
            Self::DirectMessage => write!(f, "direct_message"),
            Self::Job => write!(f, "job"),
            Self::System => write!(f, "system"),
        }
    }
}

/// A reference to a post (URI + CID).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostRef {
    pub uri: String,
    pub cid: String,
}

/// A message in DM conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationHistoryMessage {
    pub sender_label: String,
    pub text: String,
    pub sent_at: DateTime<Utc>,
}

/// Type-specific payload for an inbox item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InboxPayload {
    Notification {
        author_did: String,
        author_handle: String,
        /// "mention", "reply", "quote", etc.
        kind: String,
        text: Option<String>,
        uri: String,
        cid: String,
        parent: Option<PostRef>,
        root: Option<PostRef>,
        #[serde(default)]
        facets: Vec<Facet>,
    },
    DirectMessage {
        sender_did: String,
        sender_handle: String,
        convo_id: String,
        message_id: String,
        text: String,
        #[serde(default)]
        facets: Vec<Facet>,
        #[serde(default)]
        history: Vec<ConversationHistoryMessage>,
    },
    Job {
        name: String,
        instructions: String,
    },
    System {
        message: String,
    },
    ToolApproved {
        tool_name: String,
        tool_rkey: String,
        approval_rkey: String,
    },
}

// ============================================================================
// Inbox
// ============================================================================

/// Default maximum inbox size before overflow trimming kicks in.
const DEFAULT_MAX_SIZE: usize = 200;

/// In-memory inbox that holds pending items for Winter to process.
///
/// Items are pushed by the daemon (pollers, scheduler) and read/acknowledged
/// by Winter via MCP tools. Not a queue — Winter sees all items at once.
pub struct Inbox {
    items: RwLock<Vec<InboxItem>>,
    max_size: usize,
}

impl Inbox {
    /// Create a new inbox with default max size.
    pub fn new() -> Self {
        Self {
            items: RwLock::new(Vec::new()),
            max_size: DEFAULT_MAX_SIZE,
        }
    }

    /// Create a new inbox with a custom max size.
    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            items: RwLock::new(Vec::new()),
            max_size,
        }
    }

    /// Push an item into the inbox.
    ///
    /// If the inbox exceeds max_size, drops oldest items from the lowest
    /// priority tier first.
    pub async fn push(&self, item: InboxItem) {
        let mut items = self.items.write().await;
        items.push(item);

        // Overflow trimming: drop oldest items from lowest priority tier
        if items.len() > self.max_size {
            // Find the lowest priority present
            if let Some(min_priority) = items.iter().map(|i| i.priority).min() {
                // Remove the oldest item with that priority
                if let Some(pos) = items.iter().position(|i| i.priority == min_priority) {
                    items.remove(pos);
                }
            }
        }
    }

    /// Get all pending items, sorted by priority (descending) then time (ascending).
    pub async fn items(&self) -> Vec<InboxItem> {
        let items = self.items.read().await;
        let mut sorted = items.clone();
        sorted.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        sorted
    }

    /// Remove items with matching IDs. Returns count of items removed.
    pub async fn acknowledge(&self, ids: &[String]) -> usize {
        let mut items = self.items.write().await;
        let before = items.len();
        items.retain(|item| !ids.contains(&item.id));
        before - items.len()
    }

    /// Get the number of pending items.
    pub async fn len(&self) -> usize {
        self.items.read().await.len()
    }

    /// Check if the inbox is empty.
    pub async fn is_empty(&self) -> bool {
        self.items.read().await.is_empty()
    }

    /// Check if any items have a priority >= threshold.
    pub async fn has_urgent(&self, min_priority: u8) -> bool {
        self.items
            .read()
            .await
            .iter()
            .any(|i| i.priority >= min_priority)
    }
}

impl Default for Inbox {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Helper: Create inbox items with auto-generated IDs
// ============================================================================

impl InboxItem {
    /// Create a new notification inbox item.
    pub fn notification(
        author_did: String,
        author_handle: String,
        kind: String,
        text: Option<String>,
        uri: String,
        cid: String,
        parent: Option<PostRef>,
        root: Option<PostRef>,
        facets: Vec<Facet>,
    ) -> Self {
        let root_uri = root.as_ref().map(|r| r.uri.as_str()).unwrap_or(&uri);
        let context_tag = format!("notification:{}:root={}", uri, root_uri);
        Self {
            id: Tid::now().to_string(),
            kind: InboxItemKind::Notification,
            priority: 100,
            created_at: Utc::now(),
            context_tag,
            payload: InboxPayload::Notification {
                author_did,
                author_handle,
                kind,
                text,
                uri,
                cid,
                parent,
                root,
                facets,
            },
        }
    }

    /// Create a new DM inbox item.
    pub fn direct_message(
        sender_did: String,
        sender_handle: String,
        convo_id: String,
        message_id: String,
        text: String,
        facets: Vec<Facet>,
        history: Vec<ConversationHistoryMessage>,
    ) -> Self {
        let context_tag = format!("dm:{}:{}", convo_id, message_id);
        Self {
            id: Tid::now().to_string(),
            kind: InboxItemKind::DirectMessage,
            priority: 200,
            created_at: Utc::now(),
            context_tag,
            payload: InboxPayload::DirectMessage {
                sender_did,
                sender_handle,
                convo_id,
                message_id,
                text,
                facets,
                history,
            },
        }
    }

    /// Create a new job inbox item.
    pub fn job(name: String, instructions: String) -> Self {
        let context_tag = format!("job:{}", name);
        Self {
            id: Tid::now().to_string(),
            kind: InboxItemKind::Job,
            priority: 50,
            created_at: Utc::now(),
            context_tag,
            payload: InboxPayload::Job { name, instructions },
        }
    }

    /// Create a new tool approved inbox item.
    pub fn tool_approved(tool_name: String, tool_rkey: String, approval_rkey: String) -> Self {
        let context_tag = format!("tool_approved:{}", tool_rkey);
        Self {
            id: Tid::now().to_string(),
            kind: InboxItemKind::System,
            priority: 75,
            created_at: Utc::now(),
            context_tag,
            payload: InboxPayload::ToolApproved {
                tool_name,
                tool_rkey,
                approval_rkey,
            },
        }
    }

    /// Create a new system message inbox item.
    pub fn system(message: String) -> Self {
        Self {
            id: Tid::now().to_string(),
            kind: InboxItemKind::System,
            priority: 50,
            created_at: Utc::now(),
            context_tag: "system".to_string(),
            payload: InboxPayload::System { message },
        }
    }
}

// ============================================================================
// MCP Tool Definitions
// ============================================================================

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "check_inbox".to_string(),
            description: "Check the inbox for pending items (notifications, DMs, jobs). Returns all pending items sorted by priority then time. Call this regularly between tasks and during free time.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "types": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["notification", "direct_message", "job", "system"] },
                        "description": "Filter by item kind. If omitted, returns all types."
                    },
                    "min_priority": {
                        "type": "integer",
                        "description": "Only return items with priority >= this value. Default: 0."
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "acknowledge_inbox".to_string(),
            description: "Remove items from the inbox by ID. Call this after handling items or deciding they don't need attention. Returns count of items removed.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "IDs of inbox items to acknowledge/remove."
                    }
                },
                "required": ["ids"]
            }),
        },
    ]
}

pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

// ============================================================================
// MCP Tool Implementations
// ============================================================================

/// Handle the `check_inbox` tool call.
pub async fn check_inbox(
    inbox: Option<&Inbox>,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let inbox = match inbox {
        Some(inbox) => inbox,
        None => {
            return CallToolResult::success(
                json!({
                    "items": [],
                    "count": 0,
                    "message": "No inbox configured"
                })
                .to_string(),
            );
        }
    };

    let items = inbox.items().await;

    // Apply filters
    let type_filter: Option<Vec<String>> = arguments
        .get("types")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let min_priority: u8 = arguments
        .get("min_priority")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u8;

    let filtered: Vec<&InboxItem> = items
        .iter()
        .filter(|item| {
            if let Some(ref types) = type_filter {
                if !types.contains(&item.kind.to_string()) {
                    return false;
                }
            }
            item.priority >= min_priority
        })
        .collect();

    let count = filtered.len();
    let items_json: Vec<Value> = filtered.iter().map(|item| json!(item)).collect();

    CallToolResult::success(
        json!({
            "items": items_json,
            "count": count
        })
        .to_string(),
    )
}

/// Handle the `acknowledge_inbox` tool call.
pub async fn acknowledge_inbox(
    inbox: Option<&Inbox>,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let inbox = match inbox {
        Some(inbox) => inbox,
        None => {
            return CallToolResult::error("No inbox configured".to_string());
        }
    };

    let ids: Vec<String> = match arguments.get("ids") {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(ids) => ids,
            Err(e) => {
                return CallToolResult::error(format!("Invalid ids parameter: {}", e));
            }
        },
        None => {
            return CallToolResult::error("Missing required parameter: ids".to_string());
        }
    };

    let removed = inbox.acknowledge(&ids).await;

    CallToolResult::success(
        json!({
            "removed": removed,
            "remaining": inbox.len().await
        })
        .to_string(),
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definitions() {
        let defs = definitions();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "check_inbox");
        assert_eq!(defs[1].name, "acknowledge_inbox");
    }

    #[tokio::test]
    async fn test_inbox_push_and_items() {
        let inbox = Inbox::new();

        inbox
            .push(InboxItem::job("test_job".into(), "do something".into()))
            .await;
        inbox
            .push(InboxItem::system("hello".into()))
            .await;

        assert_eq!(inbox.len().await, 2);
        assert!(!inbox.is_empty().await);
    }

    #[tokio::test]
    async fn test_inbox_priority_ordering() {
        let inbox = Inbox::new();

        // Push low priority first, high priority second
        inbox
            .push(InboxItem::job("low".into(), "low priority".into()))
            .await;
        inbox
            .push(InboxItem::notification(
                "did:plc:test".into(),
                "test.bsky.social".into(),
                "reply".into(),
                Some("hello".into()),
                "at://did:plc:test/app.bsky.feed.post/123".into(),
                "bafytest".into(),
                None,
                None,
                vec![],
            ))
            .await;

        let items = inbox.items().await;
        assert_eq!(items.len(), 2);
        // Notification (100) should come before job (50)
        assert_eq!(items[0].priority, 100);
        assert_eq!(items[1].priority, 50);
    }

    #[tokio::test]
    async fn test_inbox_acknowledge() {
        let inbox = Inbox::new();

        inbox
            .push(InboxItem::job("a".into(), "first".into()))
            .await;
        inbox
            .push(InboxItem::job("b".into(), "second".into()))
            .await;

        let items = inbox.items().await;
        let first_id = items[0].id.clone();

        let removed = inbox.acknowledge(&[first_id]).await;
        assert_eq!(removed, 1);
        assert_eq!(inbox.len().await, 1);
    }

    #[tokio::test]
    async fn test_inbox_overflow_trimming() {
        let inbox = Inbox::with_max_size(3);

        // Push 3 low-priority items
        for i in 0..3 {
            inbox
                .push(InboxItem::job(format!("job_{}", i), "test".into()))
                .await;
        }
        assert_eq!(inbox.len().await, 3);

        // Push a 4th item — should trim oldest low-priority
        inbox
            .push(InboxItem::notification(
                "did:plc:test".into(),
                "test.bsky.social".into(),
                "reply".into(),
                None,
                "at://test".into(),
                "bafytest".into(),
                None,
                None,
                vec![],
            ))
            .await;
        assert_eq!(inbox.len().await, 3);
    }

    #[tokio::test]
    async fn test_check_inbox_tool() {
        let inbox = Inbox::new();
        inbox
            .push(InboxItem::job("test".into(), "do it".into()))
            .await;

        let args = HashMap::new();
        let result = check_inbox(Some(&inbox), &args).await;
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_acknowledge_inbox_tool() {
        let inbox = Inbox::new();
        inbox
            .push(InboxItem::job("test".into(), "do it".into()))
            .await;

        let items = inbox.items().await;
        let id = items[0].id.clone();

        let mut args = HashMap::new();
        args.insert("ids".to_string(), json!([id]));

        let result = acknowledge_inbox(Some(&inbox), &args).await;
        assert!(!result.is_error.unwrap_or(false));
        assert!(inbox.is_empty().await);
    }
}
