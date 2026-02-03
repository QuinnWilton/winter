//! Agent context for Claude prompts.

use winter_atproto::Facet;
use winter_atproto::{Directive, Identity, Thought};

/// Context assembled for a Claude prompt.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Current identity.
    pub identity: Identity,
    /// Active directives (identity components).
    pub directives: Vec<Directive>,
    /// Recent thoughts (limited, not all).
    pub recent_thoughts: Vec<Thought>,
    /// Rule heads for querying (e.g., "mutual_follow(X, Y)").
    pub rule_heads: Vec<String>,
    /// Summary of custom tools available.
    pub custom_tools: Vec<CustomToolSummary>,
    /// Trigger for this context (notification, job, etc.).
    pub trigger: Option<ContextTrigger>,
}

/// Summary of a custom tool for prompt context.
#[derive(Debug, Clone)]
pub struct CustomToolSummary {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// Whether the tool is approved.
    pub approved: bool,
}

/// Reference to a post for threading.
#[derive(Debug, Clone)]
pub struct PostRef {
    /// AT URI of the post.
    pub uri: String,
    /// CID of the post.
    pub cid: String,
}

/// What triggered this agent invocation.
#[derive(Debug, Clone)]
pub enum ContextTrigger {
    /// A Bluesky notification.
    Notification {
        kind: String,
        author_did: String,
        author_handle: String,
        text: Option<String>,
        /// AT URI of the notification post.
        uri: String,
        /// CID of the notification post (needed for replying).
        cid: String,
        /// Parent post reference (for threading context).
        /// If this notification is a reply, this is the post it replied to.
        parent: Option<PostRef>,
        /// Root post reference (for threading context).
        /// The original post that started the thread.
        root: Option<PostRef>,
        /// Rich text facets (mentions, links, tags).
        facets: Vec<Facet>,
    },
    /// A direct message.
    DirectMessage {
        /// Conversation ID.
        convo_id: String,
        /// Message ID.
        message_id: String,
        /// DID of the sender.
        sender_did: String,
        /// Handle of the sender.
        sender_handle: String,
        /// Message text.
        text: String,
        /// Rich text facets (mentions, links, tags).
        facets: Vec<Facet>,
    },
    /// A scheduled job.
    Job { id: String, name: String },
    /// An awaken cycle.
    Awaken,
}

/// Scope for filtering thoughts by conversation context.
///
/// When multiple workers process notifications concurrently, thoughts need to be
/// filtered by conversation scope to prevent cross-contamination.
#[derive(Debug, Clone)]
pub enum ConversationScope {
    /// A thread on Bluesky, identified by root post URI.
    Thread { root_uri: String },
    /// A direct message conversation.
    DirectMessage { convo_id: String },
    /// A scheduled job execution.
    Job { name: String },
    /// Global context (awaken cycles) - matches thoughts with no trigger.
    Global,
}

impl ContextTrigger {
    /// Extract the conversation scope for thought filtering.
    pub fn conversation_scope(&self) -> ConversationScope {
        match self {
            ContextTrigger::Notification { root, uri, .. } => {
                // Use root URI if available, otherwise the post is its own root
                let root_uri = root
                    .as_ref()
                    .map(|r| r.uri.clone())
                    .unwrap_or_else(|| uri.clone());
                ConversationScope::Thread { root_uri }
            }
            ContextTrigger::DirectMessage { convo_id, .. } => ConversationScope::DirectMessage {
                convo_id: convo_id.clone(),
            },
            ContextTrigger::Job { name, .. } => ConversationScope::Job { name: name.clone() },
            ContextTrigger::Awaken => ConversationScope::Global,
        }
    }

    /// Generate trigger string for thought records.
    ///
    /// Format includes root URI for notifications to enable thread-scoped filtering:
    /// - Notification: `notification:{uri}:root={root_uri}`
    /// - DM: `dm:{convo_id}:{message_id}`
    /// - Job: `job:{name}`
    /// - Awaken: None (global thoughts)
    pub fn trigger_string(&self) -> Option<String> {
        match self {
            ContextTrigger::Notification { uri, root, .. } => {
                // Enhanced format: include root URI for thread continuity
                let root_uri = root.as_ref().map(|r| &r.uri).unwrap_or(uri);
                Some(format!("notification:{}:root={}", uri, root_uri))
            }
            ContextTrigger::DirectMessage {
                convo_id,
                message_id,
                ..
            } => Some(format!("dm:{}:{}", convo_id, message_id)),
            ContextTrigger::Job { name, .. } => Some(format!("job:{}", name)),
            ContextTrigger::Awaken => None,
        }
    }
}

impl AgentContext {
    /// Create a new context.
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            directives: Vec::new(),
            recent_thoughts: Vec::new(),
            rule_heads: Vec::new(),
            custom_tools: Vec::new(),
            trigger: None,
        }
    }

    /// Add directives.
    pub fn with_directives(mut self, directives: Vec<Directive>) -> Self {
        self.directives = directives;
        self
    }

    /// Add recent thoughts.
    pub fn with_thoughts(mut self, thoughts: Vec<Thought>) -> Self {
        self.recent_thoughts = thoughts;
        self
    }

    /// Add rule heads (e.g., "mutual_follow(X, Y)") for querying.
    pub fn with_rule_heads(mut self, heads: Vec<String>) -> Self {
        self.rule_heads = heads;
        self
    }

    /// Add custom tool summaries.
    pub fn with_custom_tools(mut self, tools: Vec<CustomToolSummary>) -> Self {
        self.custom_tools = tools;
        self
    }

    /// Set the trigger.
    pub fn with_trigger(mut self, trigger: ContextTrigger) -> Self {
        self.trigger = Some(trigger);
        self
    }

    /// Get a short description of the trigger for tracing.
    pub fn trigger_description(&self) -> String {
        match &self.trigger {
            Some(ContextTrigger::Notification {
                kind,
                author_handle,
                ..
            }) => {
                format!("notification:{}:@{}", kind, author_handle)
            }
            Some(ContextTrigger::DirectMessage { sender_handle, .. }) => {
                format!("dm:@{}", sender_handle)
            }
            Some(ContextTrigger::Job { name, .. }) => {
                format!("job:{}", name)
            }
            Some(ContextTrigger::Awaken) => "awaken".to_string(),
            None => "none".to_string(),
        }
    }
}
