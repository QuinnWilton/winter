//! Agent context for Claude prompts.

use chrono::{DateTime, Utc};
use winter_atproto::{Directive, Identity, Thought};

/// A message in the DM conversation history.
#[derive(Debug, Clone)]
pub struct ConversationHistoryMessage {
    /// Label for the sender ("You" for Winter, "@handle" for others).
    pub sender_label: String,
    /// Message text.
    pub text: String,
    /// When the message was sent.
    pub sent_at: DateTime<Utc>,
}

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

/// What triggered this agent invocation.
#[derive(Debug, Clone)]
pub enum ContextTrigger {
    /// A scheduled job.
    Job { id: String, name: String },
    /// A persistent session (inbox-driven model).
    PersistentSession,
}

impl ContextTrigger {
    /// Generate trigger string for thought records.
    pub fn trigger_string(&self) -> Option<String> {
        match self {
            ContextTrigger::Job { name, .. } => Some(format!("job:{}", name)),
            ContextTrigger::PersistentSession => Some("persistent".to_string()),
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
            Some(ContextTrigger::Job { name, .. }) => {
                format!("job:{}", name)
            }
            Some(ContextTrigger::PersistentSession) => "persistent".to_string(),
            None => "none".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity() -> Identity {
        Identity {
            operator_did: "did:plc:test-operator".to_string(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
        }
    }

    #[test]
    fn job_trigger_string() {
        let trigger = ContextTrigger::Job {
            id: "tid123".to_string(),
            name: "awaken".to_string(),
        };
        assert_eq!(trigger.trigger_string(), Some("job:awaken".to_string()));
    }

    #[test]
    fn persistent_trigger_string() {
        let trigger = ContextTrigger::PersistentSession;
        assert_eq!(trigger.trigger_string(), Some("persistent".to_string()));
    }

    #[test]
    fn trigger_description_job() {
        let ctx = AgentContext::new(test_identity()).with_trigger(ContextTrigger::Job {
            id: "tid123".to_string(),
            name: "maintenance".to_string(),
        });
        assert_eq!(ctx.trigger_description(), "job:maintenance");
    }

    #[test]
    fn trigger_description_persistent() {
        let ctx =
            AgentContext::new(test_identity()).with_trigger(ContextTrigger::PersistentSession);
        assert_eq!(ctx.trigger_description(), "persistent");
    }

    #[test]
    fn trigger_description_none() {
        let ctx = AgentContext::new(test_identity());
        assert_eq!(ctx.trigger_description(), "none");
    }

    #[test]
    fn builder_chain() {
        let ctx = AgentContext::new(test_identity())
            .with_directives(vec![])
            .with_thoughts(vec![])
            .with_rule_heads(vec!["mutual(X)".to_string()])
            .with_custom_tools(vec![CustomToolSummary {
                name: "my_tool".to_string(),
                description: "does stuff".to_string(),
                approved: true,
            }]);
        assert_eq!(ctx.rule_heads.len(), 1);
        assert_eq!(ctx.custom_tools.len(), 1);
        assert!(ctx.trigger.is_none());
    }
}
