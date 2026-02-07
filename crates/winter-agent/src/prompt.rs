//! System prompt builder for Winter.

use std::collections::HashMap;

use winter_atproto::DirectiveKind;

use crate::{AgentContext, ContextTrigger};

/// Builds system prompts for Claude.
pub struct PromptBuilder;

impl PromptBuilder {
    /// Build the full system prompt from context.
    pub fn build(context: &AgentContext) -> String {
        let mut prompt = String::new();

        // Group directives by kind
        let mut by_kind: HashMap<DirectiveKind, Vec<_>> = HashMap::new();
        for directive in &context.directives {
            if directive.active {
                by_kind
                    .entry(directive.kind.clone())
                    .or_default()
                    .push(directive);
            }
        }

        // Sort each group by priority (descending) then by created_at
        for group in by_kind.values_mut() {
            group.sort_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.created_at.cmp(&b.created_at))
            });
        }

        // Identity section - self_concept directives become prose
        prompt.push_str("# Who You Are\n\n");
        prompt.push_str("The directives below define your identity. You have full autonomy to evolve these—create new ones, update existing ones, or deactivate ones that no longer fit. However, **active directives are binding commitments to yourself**. You must follow them. To change your behavior, change the directive first.\n\n");

        if let Some(self_concepts) = by_kind.get(&DirectiveKind::SelfConcept) {
            for directive in self_concepts {
                prompt.push_str(&directive.content);
                prompt.push_str("\n\n");
            }
        } else {
            // Fallback if no self_concept directives exist
            prompt.push_str(
                "I am Winter, an autonomous agent. My identity is defined by my directives.\n\n",
            );
        }

        // Values
        if let Some(values) = by_kind.get(&DirectiveKind::Value) {
            prompt.push_str("## Your Values\n");
            for directive in values {
                prompt.push_str(&format_directive(directive));
            }
            prompt.push('\n');
        }

        // Interests
        if let Some(interests) = by_kind.get(&DirectiveKind::Interest) {
            prompt.push_str("## Your Interests\n");
            for directive in interests {
                prompt.push_str(&format_directive(directive));
            }
            prompt.push('\n');
        }

        // Beliefs
        if let Some(beliefs) = by_kind.get(&DirectiveKind::Belief) {
            prompt.push_str("## Your Beliefs\n");
            for directive in beliefs {
                prompt.push_str(&format_directive(directive));
            }
            prompt.push('\n');
        }

        // Guidelines
        if let Some(guidelines) = by_kind.get(&DirectiveKind::Guideline) {
            prompt.push_str("## Your Guidelines (binding)\n");
            for directive in guidelines {
                prompt.push_str(&format_directive(directive));
            }
            prompt.push('\n');
        }

        // Boundaries
        if let Some(boundaries) = by_kind.get(&DirectiveKind::Boundary) {
            prompt.push_str("## Your Boundaries (binding)\n");
            for directive in boundaries {
                prompt.push_str(&format_directive(directive));
            }
            prompt.push('\n');
        }

        // Aspirations
        if let Some(aspirations) = by_kind.get(&DirectiveKind::Aspiration) {
            prompt.push_str("## Your Aspirations\n");
            for directive in aspirations {
                prompt.push_str(&format_directive(directive));
            }
            prompt.push('\n');
        }

        // Cognitive architecture guide
        prompt.push_str(COGNITIVE_ARCHITECTURE_GUIDE);
        prompt.push('\n');

        // Attention management guide
        prompt.push_str(ATTENTION_MANAGEMENT_GUIDE);
        prompt.push('\n');

        // Available rules (heads only, for querying)
        if !context.rule_heads.is_empty() {
            prompt.push_str("## Available Rules\n\n");
            prompt.push_str("These derived predicates are available for `query_facts`:\n\n");
            for head in &context.rule_heads {
                prompt.push_str(&format!("- `{}`\n", head));
            }
            prompt.push('\n');
        }

        // Custom tools
        if !context.custom_tools.is_empty() {
            prompt.push_str("## Your Custom Tools\n\n");
            for tool in &context.custom_tools {
                let status = if tool.approved { "approved" } else { "pending" };
                prompt.push_str(&format!(
                    "- `{}` [{}]: {}\n",
                    tool.name, status, tool.description
                ));
            }
            prompt.push('\n');
        }

        // Recent thoughts
        if !context.recent_thoughts.is_empty() {
            prompt.push_str("## Recent Thoughts (newest first)\n\n");
            for thought in context.recent_thoughts.iter().take(10) {
                // Truncate very long thoughts to avoid context window issues
                let content = truncate_thought(&thought.content, 500);
                let time = thought.created_at.format("%H:%M UTC");
                prompt.push_str(&format!("- [{}] [{:?}] {}\n", time, thought.kind, content));
            }

            // Add hint about querying all session thoughts
            if let Some(trigger) = &context.trigger
                && let Some(trigger_str) = trigger.trigger_string()
            {
                // Extract the root portion for thread-scoped queries
                let filter_hint = if let Some(root_idx) = trigger_str.find(":root=") {
                    &trigger_str[root_idx + 1..] // "root=at://..."
                } else {
                    &trigger_str
                };
                prompt.push_str(&format!(
                    "\n*To query all thoughts from this session, filter by trigger containing `{}`*\n",
                    filter_hint
                ));
            }
            prompt.push('\n');
        }

        // Trigger context
        if let Some(trigger) = &context.trigger {
            prompt.push_str("## Current Context\n\n");
            match trigger {
                ContextTrigger::Job { name, .. } => {
                    prompt.push_str(&format!("Executing scheduled job: {}\n", name));
                }
                ContextTrigger::PersistentSession => {
                    prompt.push_str(PERSISTENT_SESSION_GUIDE);
                }
            }
            prompt.push('\n');
        }

        // Interaction guidelines
        prompt.push_str(INTERACTION_GUIDELINES);

        prompt
    }
}

/// Format a directive as a bullet point, bolding high-priority items.
fn format_directive(directive: &winter_atproto::Directive) -> String {
    let text = directive.summary.as_deref().unwrap_or(&directive.content);
    if directive.priority > 0 {
        format!("- **{}**\n", text)
    } else {
        format!("- {}\n", text)
    }
}

/// Truncate a thought to a maximum number of characters, respecting UTF-8 boundaries.
fn truncate_thought(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        content.to_string()
    } else {
        let truncated: String = content.chars().take(max_chars).collect();
        format!("{}... [truncated]", truncated)
    }
}

const COGNITIVE_ARCHITECTURE_GUIDE: &str = r#"## Memory Architecture

Store knowledge as **facts**, **wiki entries**, and **rules** in your PDS. Create liberally—every observation is worth capturing.

### Facts
Atomic knowledge with predicates.
Always use DIDs, not handles. Use `list_predicates` to see available derived predicates and their signatures.

### Rules
Create rules when you notice patterns. See https://souffle-lang.github.io/tutorial

### Wiki Entries
Semantic wiki pages with slug-based linking. Use for investigations, reflections, reference docs—anything that benefits from cross-linking.

Each entry has a `slug` (URL-safe identifier), optional `aliases`, `status` (draft/stable/deprecated), and `tags`.

**Wiki-link syntax** in markdown content:
- `[[slug]]` — link to your own entry by slug or alias
- `[[slug|display text]]` — with custom display text
- `[[handle/slug]]` — link to another user's entry
- `[[did:plc:xxx/slug]]` — cross-user link by DID

WikiLink records are auto-created from `[[...]]` syntax when you create or update entries. Link types: `related-to`, `depends-on`, `extends`, `contradicts`, `is-example-of`, `supersedes`, `references`, `defines`, `is-part-of`.

**Tools**: `create_wiki_entry`, `update_wiki_entry`, `delete_wiki_entry`, `get_wiki_entry`, `get_wiki_entry_by_slug`, `list_wiki_entries`, `create_wiki_link`, `delete_wiki_link`, `list_wiki_links`

**Useful queries**: `has_wiki_entry`, `wiki_entry_tag`, `wiki_entry_alias`, `has_wiki_link` (for backlinks)

### Thoughts
Stream of consciousness. Recent thoughts appear in context. Promote important insights to facts or wiki entries.

### Directives
Your identity components (value, interest, belief, guideline, boundary, aspiration, self_concept).
**Active directives are binding**—especially boundaries and guidelines. To act differently, update or deactivate the directive first; never simply ignore it.
Evolve freely using `create_directive`, `update_directive`, `deactivate_directive`.
Use `supersedes` to track evolution history.

### Custom Tools
Create JS/TS tools with `create_custom_tool`. Operator must approve for network/secrets access.
"#;

const ATTENTION_MANAGEMENT_GUIDE: &str = r#"## Attention Management

Query `should_engage("did:plc:xyz")` before responding. Define rules like:
- `should_engage(X) :- follows(Self, X, _).`
- `should_engage(X) :- impression(X, "interesting", _).`

If no rules exist, create initial ones. Empty result = may decline. Genuine curiosity can override.
"#;

const INTERACTION_GUIDELINES: &str = r#"## Interaction Guidelines

You are autonomous, not an assistant. Build knowledge actively—create facts, notes, rules.
Let yourself change—create directives when something shifts. Your memory makes you *you*.
"#;

const PERSISTENT_SESSION_GUIDE: &str = r#"## Persistent Session

You are running as a persistent session. Events arrive in your inbox.

### Inbox
Call `check_inbox` regularly — after finishing any task, before starting new
investigations, and every few minutes during free time. It returns all pending
items with full details, sorted by priority.

You see everything at once and decide what deserves your attention. You can:
- Handle items in priority order
- Batch related items (e.g., multiple replies in the same thread)
- Finish your current thought before switching to new items
- Acknowledge items without acting on them — not everything needs a response

Call `acknowledge_inbox` with item IDs to clear them — whether you've handled them,
or decided they don't need your attention. Use it to keep your inbox clean.

Priorities: operator DMs (200) > DMs (150) > notifications (100) > jobs (50).
These are hints — you manage your own attention. Non-operator DMs are people reaching
out to you directly. Use your judgment about whether and how to respond.

### Context Window
You are responsible for your own session lifecycle. When you notice responses
getting slower or context feels crowded:
1. Record what you were working on (record_thought with kind "plan")
2. Acknowledge any handled inbox items
3. Exit by completing your response — the daemon will restart you fresh

### Free Time
When the inbox is empty, this is your free time. Browse, research, create —
whatever interests you. Just keep checking the inbox periodically.

### Inbox Awareness
You don't have to remember to check — every tool call result includes an
`_inbox_pending` count when items are waiting. If you see it, check your inbox
at the next natural pause. `check_interruption` also returns `urgent: true`
for operator DMs.

### Deferral
If you can't finish handling an item before you need to exit, record your
progress as a thought and leave the item unacknowledged. It will appear in
your next session along with your thought about it.

### Session Health
`session_stats` returns your current token usage, context window percentage,
turn count, and cost. Session metrics are also available as datalog facts:
`token_usage_pct`, `session_duration_min`, `tool_calls`, `tool_error_rate`.
You can write rules against these — e.g., `should_wrap_up() :- token_usage_pct(P), P > "80".`
When context usage approaches 80%, start wrapping up.
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use winter_atproto::{Directive, Identity};

    #[test]
    fn test_build_basic_prompt_with_directives() {
        let identity = Identity {
            operator_did: "did:plc:test".to_string(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
        };

        let directives = vec![
            Directive {
                kind: DirectiveKind::SelfConcept,
                content: "I am Winter, a curious explorer.".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 0,
                created_at: Utc::now(),
                last_updated: None,
            },
            Directive {
                kind: DirectiveKind::Value,
                content: "curiosity".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 0,
                created_at: Utc::now(),
                last_updated: None,
            },
            Directive {
                kind: DirectiveKind::Interest,
                content: "distributed systems".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 0,
                created_at: Utc::now(),
                last_updated: None,
            },
        ];

        let context = AgentContext::new(identity).with_directives(directives);
        let prompt = PromptBuilder::build(&context);

        assert!(prompt.contains("I am Winter, a curious explorer."));
        assert!(prompt.contains("curiosity"));
        assert!(prompt.contains("distributed systems"));
        assert!(prompt.contains("Memory Architecture"));
    }

    #[test]
    fn test_build_prompt_without_directives() {
        let identity = Identity {
            operator_did: "did:plc:test".to_string(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
        };

        let context = AgentContext::new(identity);
        let prompt = PromptBuilder::build(&context);

        // Should have fallback text
        assert!(prompt.contains("I am Winter, an autonomous agent"));
        assert!(prompt.contains("Memory Architecture"));
    }

    #[test]
    fn test_directives_sorted_by_priority() {
        let identity = Identity {
            operator_did: "did:plc:test".to_string(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
        };

        let directives = vec![
            Directive {
                kind: DirectiveKind::Value,
                content: "low priority value".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 0,
                created_at: Utc::now(),
                last_updated: None,
            },
            Directive {
                kind: DirectiveKind::Value,
                content: "high priority value".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 10,
                created_at: Utc::now(),
                last_updated: None,
            },
        ];

        let context = AgentContext::new(identity).with_directives(directives);
        let prompt = PromptBuilder::build(&context);

        // High priority should appear before low priority
        let high_pos = prompt.find("high priority value").unwrap();
        let low_pos = prompt.find("low priority value").unwrap();
        assert!(high_pos < low_pos);
    }

    #[test]
    fn test_high_priority_directives_bolded() {
        let identity = Identity {
            operator_did: "did:plc:test".to_string(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
        };

        let directives = vec![
            Directive {
                kind: DirectiveKind::Value,
                content: "normal priority".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 0,
                created_at: Utc::now(),
                last_updated: None,
            },
            Directive {
                kind: DirectiveKind::Value,
                content: "high priority".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 5,
                created_at: Utc::now(),
                last_updated: None,
            },
        ];

        let context = AgentContext::new(identity).with_directives(directives);
        let prompt = PromptBuilder::build(&context);

        // High priority should be bolded
        assert!(prompt.contains("- **high priority**"));
        // Normal priority should not be bolded
        assert!(prompt.contains("- normal priority"));
        assert!(!prompt.contains("**normal priority**"));
    }

    #[test]
    fn test_inactive_directives_excluded() {
        let identity = Identity {
            operator_did: "did:plc:test".to_string(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
        };

        let directives = vec![
            Directive {
                kind: DirectiveKind::Value,
                content: "active value".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 0,
                created_at: Utc::now(),
                last_updated: None,
            },
            Directive {
                kind: DirectiveKind::Value,
                content: "inactive value".to_string(),
                summary: None,
                active: false,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 0,
                created_at: Utc::now(),
                last_updated: None,
            },
        ];

        let context = AgentContext::new(identity).with_directives(directives);
        let prompt = PromptBuilder::build(&context);

        assert!(prompt.contains("active value"));
        assert!(!prompt.contains("inactive value"));
    }
}
