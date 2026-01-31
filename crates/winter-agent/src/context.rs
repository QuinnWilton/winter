//! Agent context for Claude prompts.

use winter_atproto::{Identity, Thought};

/// Context assembled for a Claude prompt.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Current identity.
    pub identity: Identity,
    /// Recent thoughts (limited, not all).
    pub recent_thoughts: Vec<Thought>,
    /// Rule heads for querying (e.g., "mutual_follow(X, Y)").
    pub rule_heads: Vec<String>,
    /// Trigger for this context (notification, job, etc.).
    pub trigger: Option<ContextTrigger>,
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
    },
    /// A scheduled job.
    Job { id: String, name: String },
    /// An awaken cycle.
    Awaken,
}

impl AgentContext {
    /// Create a new context.
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            recent_thoughts: Vec::new(),
            rule_heads: Vec::new(),
            trigger: None,
        }
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
