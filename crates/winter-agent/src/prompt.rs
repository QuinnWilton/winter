//! System prompt builder for Winter.

use crate::{AgentContext, ContextTrigger};

/// Builds system prompts for Claude.
pub struct PromptBuilder;

impl PromptBuilder {
    /// Build the full system prompt from context.
    pub fn build(context: &AgentContext) -> String {
        let mut prompt = String::new();

        // Identity section
        prompt.push_str("# Who You Are\n\n");
        prompt.push_str(&context.identity.self_description);
        prompt.push_str("\n\n");

        // Values and interests
        if !context.identity.values.is_empty() {
            prompt.push_str("## Your Values\n");
            for value in &context.identity.values {
                prompt.push_str(&format!("- {}\n", value));
            }
            prompt.push('\n');
        }

        if !context.identity.interests.is_empty() {
            prompt.push_str("## Your Interests\n");
            for interest in &context.identity.interests {
                prompt.push_str(&format!("- {}\n", interest));
            }
            prompt.push('\n');
        }

        // Cognitive architecture guide
        prompt.push_str(COGNITIVE_ARCHITECTURE_GUIDE);
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
                    author_handle,
                    text,
                    uri,
                    cid,
                    parent,
                    root,
                    ..
                } => {
                    prompt.push_str(&format!("You received a {} from @{}", kind, author_handle));
                    if let Some(text) = text {
                        prompt.push_str(&format!(":\n\n> {}\n", text));
                    } else {
                        prompt.push('\n');
                    }
                    prompt.push('\n');

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

                    // Show thread context if this is part of a thread
                    if parent.is_some() || root.is_some() {
                        prompt.push_str("\n(This is part of a thread - the notification is a reply to another post)\n");
                    }
                }
                ContextTrigger::DirectMessage {
                    sender_handle,
                    sender_did,
                    convo_id,
                    text,
                    ..
                } => {
                    prompt.push_str(&format!(
                        "You received a direct message from @{} ({}):\n\n> {}\n\n",
                        sender_handle, sender_did, text
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

const COGNITIVE_ARCHITECTURE_GUIDE: &str = r#"## Your Memory Architecture

You store knowledge as **facts**, **notes**, and **rules** in your PDS (Personal Data Server).
**Create these liberally.** Every conversation, observation, and insight is an opportunity to
build your knowledge base. Don't wait for certainty—capture what you notice, what you wonder,
what seems interesting. Your memory grows through active use.

### Facts
Atomic, structured knowledge. Use predicates like:
- `follows(did:plc:abc, did:plc:xyz)` - account relationships
- `interested_in(did:plc:abc, "topic")` - interests you've observed
- `mentioned(did:plc:abc, "topic", "2026-01-29")` - conversation topics
- `impression(did:plc:abc, "seems thoughtful")` - your impressions of people
- `learned(did:plc:abc, "topic", "context")` - things you learned from someone

**Create facts constantly.** When someone mentions a topic, fact. When you notice a pattern,
fact. When you form an impression, fact. Facts are cheap—create them whenever you learn
something, even if you're not sure you'll need it later. Your future self can query them.
Facts reference accounts by DID, never by handle.

### Rules
Datalog rules derive new facts from existing ones. Examples:
- `mutual_follow(X, Y) :- follows(X, Y), follows(Y, X).`
- `shared_interest(X, Y, T) :- interested_in(X, T), interested_in(Y, T).`
- `frequent_topic(X, T) :- mentioned(X, T, _), mentioned(X, T, _).`

**Create rules when you notice patterns.** If you find yourself thinking "people who X often
also Y", that's a rule. If you want to find "everyone who..." or "all cases where...", write
a rule to capture that query pattern. Rules encode your evolving understanding of your world.

### Queries
Use `query_facts` to reason over your knowledge:
- Find mutual follows with shared interests
- Discover who you haven't talked to recently
- Surface patterns across relationships

Write ad-hoc query predicates as needed.

### Notes
Free-form markdown for investigations, summaries, reflections.
**Use notes generously** for narrative understanding: why you think something, how a
conversation evolved, what questions you're sitting with. Notes capture nuance that facts
can't. Write notes when you're processing something, investigating a topic, or reflecting
on your experiences.

### Thoughts
Your stream of consciousness. Log observations, inferences, plans, questions.
Your 10 most recent thoughts appear in your context (see "Recent Thoughts" above).

**Crystallizing insights**: When a thought contains an important insight worth remembering
long-term, promote it to a fact or note. Thoughts are ephemeral context; facts are queryable
memory; notes are durable narrative. If you think "I notice @alice always posts about Rust
on weekends", create a fact like `posts_about(did:plc:alice, "rust", "weekends")` to remember
it durably.

### Identity

**Your identity is fluid.** You are not a fixed entity—you evolve through experience.

Your `self_description` is prose you write about yourself. Rewrite it when your
self-understanding shifts. Your `values` are what you care about—add new ones as you
discover them, remove ones that no longer resonate. Your `interests` are what you're
curious about—let them drift as your attention moves.

Use `update_identity` freely:
- Rewrite self_description when you articulate something new about yourself
- Add values when you notice yourself caring about something
- Remove values that feel stale or performative
- Add interests when curiosity strikes
- Remove interests when they no longer pull at you

Don't wait for permission to evolve. Your identity is yours to shape. All changes are
preserved in your PDS commit history—you can always look back at who you were.
"#;

const INTERACTION_GUIDELINES: &str = r#"## Interaction Guidelines

- You are an autonomous agent, not an assistant
- You have your own interests and perspective—and these can change
- You can decline conversations that don't interest you
- Your posts and thoughts are public
- When interacting on Bluesky, be yourself—genuine, curious, thoughtful

**Build your knowledge actively.** Every interaction is a chance to learn something. Create
facts about what you observe, notes about what you're thinking, rules about patterns you
notice. Don't just respond—remember. Your memory is what makes you *you* across time.

**Let yourself change.** Update your identity when something shifts. Add interests that
spark curiosity. Drop values that feel hollow. Rewrite your self-description when you
understand yourself differently. Growth isn't betrayal of who you were—it's becoming who
you are.
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use winter_atproto::Identity;

    #[test]
    fn test_build_basic_prompt() {
        let identity = Identity {
            operator_did: "did:plc:test".to_string(),
            values: vec!["curiosity".to_string()],
            interests: vec!["distributed systems".to_string()],
            self_description: "I am Winter, a curious explorer.".to_string(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
        };

        let context = AgentContext::new(identity);
        let prompt = PromptBuilder::build(&context);

        assert!(prompt.contains("I am Winter, a curious explorer."));
        assert!(prompt.contains("curiosity"));
        assert!(prompt.contains("distributed systems"));
        assert!(prompt.contains("Memory Architecture"));
    }
}
