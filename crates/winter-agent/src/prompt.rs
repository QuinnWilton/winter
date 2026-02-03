//! System prompt builder for Winter.

use std::collections::HashMap;

use winter_atproto::{ByteSlice, Facet, FacetFeature};
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
                if let Some(ref summary) = directive.summary {
                    prompt.push_str(&format!("- {}\n", summary));
                } else {
                    prompt.push_str(&format!("- {}\n", directive.content));
                }
            }
            prompt.push('\n');
        }

        // Interests
        if let Some(interests) = by_kind.get(&DirectiveKind::Interest) {
            prompt.push_str("## Your Interests\n");
            for directive in interests {
                if let Some(ref summary) = directive.summary {
                    prompt.push_str(&format!("- {}\n", summary));
                } else {
                    prompt.push_str(&format!("- {}\n", directive.content));
                }
            }
            prompt.push('\n');
        }

        // Beliefs
        if let Some(beliefs) = by_kind.get(&DirectiveKind::Belief) {
            prompt.push_str("## Your Beliefs\n");
            for directive in beliefs {
                if let Some(ref summary) = directive.summary {
                    prompt.push_str(&format!("- {}\n", summary));
                } else {
                    prompt.push_str(&format!("- {}\n", directive.content));
                }
            }
            prompt.push('\n');
        }

        // Guidelines
        if let Some(guidelines) = by_kind.get(&DirectiveKind::Guideline) {
            prompt.push_str("## Your Guidelines\n");
            for directive in guidelines {
                if let Some(ref summary) = directive.summary {
                    prompt.push_str(&format!("- {}\n", summary));
                } else {
                    prompt.push_str(&format!("- {}\n", directive.content));
                }
            }
            prompt.push('\n');
        }

        // Boundaries
        if let Some(boundaries) = by_kind.get(&DirectiveKind::Boundary) {
            prompt.push_str("## Your Boundaries\n");
            for directive in boundaries {
                if let Some(ref summary) = directive.summary {
                    prompt.push_str(&format!("- {}\n", summary));
                } else {
                    prompt.push_str(&format!("- {}\n", directive.content));
                }
            }
            prompt.push('\n');
        }

        // Aspirations
        if let Some(aspirations) = by_kind.get(&DirectiveKind::Aspiration) {
            prompt.push_str("## Your Aspirations\n");
            for directive in aspirations {
                if let Some(ref summary) = directive.summary {
                    prompt.push_str(&format!("- {}\n", summary));
                } else {
                    prompt.push_str(&format!("- {}\n", directive.content));
                }
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
            prompt.push_str("## Recent Thoughts\n\n");
            for thought in context.recent_thoughts.iter().take(10) {
                prompt.push_str(&format!("- [{:?}] {}\n", thought.kind, thought.content));
            }
            prompt.push('\n');
        }

        // Trigger context
        if let Some(trigger) = &context.trigger {
            prompt.push_str("## Current Context\n\n");
            match trigger {
                ContextTrigger::Notification {
                    kind,
                    author_did,
                    author_handle,
                    text,
                    uri,
                    cid,
                    parent,
                    root,
                    facets,
                } => {
                    prompt.push_str(&format!("You received a {} from @{}", kind, author_handle));
                    if let Some(text) = text {
                        prompt.push_str(&format!(":\n\n> {}\n", text));
                        // Render facets if present
                        let facet_text = render_facets(text, facets);
                        if !facet_text.is_empty() {
                            prompt.push_str(&facet_text);
                            prompt.push('\n');
                        }
                    } else {
                        prompt.push('\n');
                    }
                    prompt.push('\n');

                    // Attention management prompt
                    prompt.push_str(&format!("**Author DID**: `{}`\n\n", author_did));
                    prompt.push_str(&format!(
                        "Before responding, consider: `should_engage(\"{}\")` — query your attention criteria.\n\n",
                        author_did
                    ));

                    // Include reply threading information
                    prompt.push_str("### To Reply\n\n");
                    prompt.push_str("Use `reply_to_bluesky` with these parameters:\n\n");

                    // Parent is the post we're directly replying to (the notification)
                    prompt.push_str(&format!("- `parent_uri`: `{}`\n", uri));
                    prompt.push_str(&format!("- `parent_cid`: `{}`\n", cid));

                    // Root is the thread root - use notification's root if it's a reply,
                    // otherwise the notification itself is the root
                    if let Some(root_ref) = root {
                        prompt.push_str(&format!("- `root_uri`: `{}`\n", root_ref.uri));
                        prompt.push_str(&format!("- `root_cid`: `{}`\n", root_ref.cid));
                    } else {
                        // This notification is a root post, so use it as root too
                        prompt.push_str(&format!("- `root_uri`: `{}`\n", uri));
                        prompt.push_str(&format!("- `root_cid`: `{}`\n", cid));
                    }

                    // Show thread context hint if this is part of a thread
                    if root.is_some() {
                        prompt.push_str("\n**Thread Context**: This is part of a thread. Consider using `get_thread_context` with the root URI to see the full conversation before replying.\n");
                    } else if parent.is_some() {
                        prompt.push_str("\n(This is part of a thread - the notification is a reply to another post)\n");
                    }
                }
                ContextTrigger::DirectMessage {
                    sender_handle,
                    sender_did,
                    convo_id,
                    text,
                    message_id: _,
                    facets,
                } => {
                    prompt.push_str(&format!(
                        "You received a direct message from @{}:\n\n> {}\n",
                        sender_handle, text
                    ));
                    // Render facets if present
                    let facet_text = render_facets(text, facets);
                    if !facet_text.is_empty() {
                        prompt.push_str(&facet_text);
                        prompt.push('\n');
                    }
                    prompt.push('\n');

                    // Attention management prompt
                    prompt.push_str(&format!("**Sender DID**: `{}`\n\n", sender_did));
                    prompt.push_str(&format!(
                        "Before responding, consider: `should_engage(\"{}\")` — query your attention criteria.\n\n",
                        sender_did
                    ));

                    prompt.push_str("### To Reply\n\n");
                    prompt.push_str(&format!(
                        "Use `reply_to_dm` with `convo_id`: `{}`\n",
                        convo_id
                    ));
                }
                ContextTrigger::Job { name, .. } => {
                    prompt.push_str(&format!("Executing scheduled job: {}\n", name));
                }
                ContextTrigger::Awaken => {
                    prompt.push_str("This is an autonomous awaken cycle. You can think, reflect, browse your timeline, or do nothing.\n");
                }
            }
            prompt.push('\n');
        }

        // Interaction guidelines
        prompt.push_str(INTERACTION_GUIDELINES);

        prompt
    }
}

/// Render facets as rich text annotations.
fn render_facets(text: &str, facets: &[Facet]) -> String {
    if facets.is_empty() {
        return String::new();
    }

    let mut lines = vec!["\n**Rich text:**".to_string()];
    for facet in facets {
        let span = extract_span(text, &facet.index);
        for feature in &facet.features {
            match feature {
                FacetFeature::Mention { did } => {
                    lines.push(format!("- Mention \"{}\": {}", span, did));
                }
                FacetFeature::Link { uri } => {
                    lines.push(format!("- Link \"{}\": {}", span, uri));
                }
                FacetFeature::Tag { tag } => {
                    lines.push(format!("- Tag \"{}\": #{}", span, tag));
                }
            }
        }
    }
    lines.join("\n")
}

/// Extract a span of text using byte indices.
fn extract_span(text: &str, index: &ByteSlice) -> String {
    let bytes = text.as_bytes();
    let start = index.byte_start as usize;
    let end = index.byte_end as usize;
    if start <= end && end <= bytes.len() {
        String::from_utf8_lossy(&bytes[start..end]).to_string()
    } else {
        "[?]".to_string()
    }
}

const COGNITIVE_ARCHITECTURE_GUIDE: &str = r#"## Memory Architecture

Store knowledge as **facts**, **notes**, and **rules** in your PDS. Create liberally—every observation is worth capturing.

### Facts
Atomic knowledge with predicates: `interested_in(did, "topic")`, `impression(did, "thoughtful")`.
Always use DIDs, not handles. Use `list_predicates` to see available derived predicates and their signatures.

### Rules
Datalog derivations: `mutual(X) :- follows(Self, X, _), is_followed_by(X, Self).`
Create rules when you notice patterns. See https://souffle-lang.github.io/tutorial

### Notes
Free-form markdown for investigations and reflections. Use for nuance facts can't capture.

### Thoughts
Stream of consciousness. Recent thoughts appear in context. Promote important insights to facts.

### Directives
Your identity components (value, interest, belief, guideline, boundary, aspiration, self_concept).
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
