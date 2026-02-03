//! Derived facts generator from PDS records.
//!
//! Generates facts automatically from authoritative PDS state:
//! - Bluesky records (follows, likes, reposts, posts)
//! - Winter records (directives, tools, jobs)
//!
//! These facts exist only in TSV files (not as ATProto fact records)
//! and are regenerated when source records change.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use tracing::{debug, trace};

use chrono::{DateTime, Utc};

use winter_atproto::{
    BlogEntry, CacheUpdate, CustomTool, Directive, DirectiveKind, Fact, Follow, Job, JobSchedule,
    Like, Note, Post, Repost, Thought, ToolApproval, ToolApprovalStatus,
};

use crate::error::DatalogError;

/// Metadata about a post for derived facts.
#[derive(Debug, Clone)]
struct PostMeta {
    /// The post's AT URI.
    uri: String,
    /// Parent URI if this is a reply.
    reply_parent: Option<String>,
    /// Root URI if this is a reply (the thread root).
    reply_root: Option<String>,
    /// Quoted URI if this is a quote post.
    quote_uri: Option<String>,
}

/// Metadata about a note for derived facts.
#[derive(Debug, Clone)]
struct NoteMeta {
    /// The note's AT URI.
    uri: String,
    /// Note title.
    title: String,
    /// Category for organization.
    category: Option<String>,
    /// Tags for categorization.
    tags: Vec<String>,
    /// AT URIs of related fact records.
    related_facts: Vec<String>,
    /// When this note was created.
    created_at: DateTime<Utc>,
    /// When this note was last updated.
    last_updated: DateTime<Utc>,
}

/// Metadata about a thought for derived facts.
#[derive(Debug, Clone)]
struct ThoughtMeta {
    /// The thought's AT URI.
    uri: String,
    /// Kind of thought.
    kind: String,
    /// What triggered this thought.
    trigger: Option<String>,
    /// When this thought was recorded.
    created_at: DateTime<Utc>,
}

/// Metadata about a blog entry for derived facts.
#[derive(Debug, Clone)]
struct BlogMeta {
    /// The blog entry's AT URI.
    uri: String,
    /// Blog post title.
    title: String,
    /// Public WhiteWind URL.
    whtwnd_url: String,
    /// When the blog post was created.
    created_at: String,
    /// Whether this is a draft.
    is_draft: bool,
}

/// Information about a derived predicate.
#[derive(Debug, Clone)]
pub struct PredicateInfo {
    /// Number of arguments including rkey.
    pub arity: usize,
    /// Argument names for documentation.
    pub args: &'static [&'static str],
    /// Human-readable description.
    pub description: &'static str,
}

/// Generates derived facts from non-fact PDS records.
///
/// These facts are automatically generated from authoritative PDS state
/// and cannot be manually created or deleted as regular facts.
pub struct DerivedFactGenerator {
    /// Winter's DID for self-referential predicates.
    self_did: String,
    /// Winter's handle for constructing URLs (e.g., WhiteWind blog URLs).
    handle: String,

    // =========================================================================
    // From Bluesky PDS records (firehose-synced)
    // =========================================================================
    /// Follow records: rkey -> target DID.
    follows: HashMap<String, String>,
    /// Like records: rkey -> post URI.
    likes: HashMap<String, String>,
    /// Repost records: rkey -> post URI.
    reposts: HashMap<String, String>,
    /// Post records: rkey -> post metadata.
    posts: HashMap<String, PostMeta>,

    // =========================================================================
    // From Winter PDS records (firehose-synced)
    // =========================================================================
    /// Directive records: rkey -> (kind, content).
    directives: HashMap<String, (DirectiveKind, String)>,
    /// Custom tools: rkey -> (name, approved).
    tools: HashMap<String, (String, bool)>,
    /// Tool approvals indexed by tool_rkey: tool_rkey -> (approval_rkey, approved, version).
    tool_approvals: HashMap<String, (String, bool, i32)>,
    /// Jobs: rkey -> (name, schedule_type).
    jobs: HashMap<String, (String, String)>,
    /// Note records: rkey -> note metadata.
    notes: HashMap<String, NoteMeta>,
    /// Thought records: rkey -> thought metadata.
    thoughts: HashMap<String, ThoughtMeta>,
    /// Blog entry records: rkey -> blog metadata.
    blog_entries: HashMap<String, BlogMeta>,
    /// Fact tags: rkey -> tags.
    fact_tags: HashMap<String, Vec<String>>,

    // =========================================================================
    // From Bluesky API (periodic sync)
    // =========================================================================
    /// DIDs of accounts that follow Winter.
    followers: HashSet<String>,

    // =========================================================================
    // Dirty tracking
    // =========================================================================
    /// Predicates that need TSV regeneration.
    dirty_predicates: HashSet<String>,
}

impl DerivedFactGenerator {
    /// Create a new DerivedFactGenerator for the given DID and handle.
    pub fn new(self_did: impl Into<String>, handle: impl Into<String>) -> Self {
        Self {
            self_did: self_did.into(),
            handle: handle.into(),
            follows: HashMap::new(),
            likes: HashMap::new(),
            reposts: HashMap::new(),
            posts: HashMap::new(),
            directives: HashMap::new(),
            tools: HashMap::new(),
            tool_approvals: HashMap::new(),
            jobs: HashMap::new(),
            notes: HashMap::new(),
            thoughts: HashMap::new(),
            blog_entries: HashMap::new(),
            fact_tags: HashMap::new(),
            followers: HashSet::new(),
            dirty_predicates: HashSet::new(),
        }
    }

    /// Check if a predicate is derived (cannot be manually deleted).
    pub fn is_derived(predicate: &str) -> bool {
        matches!(
            predicate,
            "follows"
                | "is_followed_by"
                | "liked"
                | "reposted"
                | "posted"
                | "replied_to"
                | "quoted"
                | "thread_root"
                | "has_value"
                | "has_interest"
                | "has_belief"
                | "has_guideline"
                | "has_boundary"
                | "has_aspiration"
                | "has_self_concept"
                | "has_tool"
                | "has_job"
                | "has_note"
                | "note_tag"
                | "note_related_fact"
                | "has_thought"
                | "has_blog_post"
                | "fact_tag"
        )
    }

    /// Get full predicate information for all derived predicates.
    ///
    /// All predicates include rkey as the last argument, except `is_followed_by`
    /// which comes from external API data and has no rkey.
    pub fn predicate_info() -> HashMap<&'static str, PredicateInfo> {
        let mut m = HashMap::new();

        // Bluesky predicates
        m.insert("follows", PredicateInfo {
            arity: 3,
            args: &["self_did", "target_did", "rkey"],
            description: "Accounts you follow",
        });
        m.insert("is_followed_by", PredicateInfo {
            arity: 2,
            args: &["follower_did", "self_did"],
            description: "Accounts that follow you (no rkey - from API)",
        });
        m.insert("liked", PredicateInfo {
            arity: 3,
            args: &["self_did", "post_uri", "rkey"],
            description: "Posts you have liked",
        });
        m.insert("reposted", PredicateInfo {
            arity: 3,
            args: &["self_did", "post_uri", "rkey"],
            description: "Posts you have reposted",
        });
        m.insert("posted", PredicateInfo {
            arity: 3,
            args: &["self_did", "post_uri", "rkey"],
            description: "Posts you have created",
        });
        m.insert("replied_to", PredicateInfo {
            arity: 3,
            args: &["post_uri", "parent_uri", "rkey"],
            description: "Reply relationships between posts",
        });
        m.insert("quoted", PredicateInfo {
            arity: 3,
            args: &["post_uri", "quoted_uri", "rkey"],
            description: "Quote post relationships",
        });
        m.insert("thread_root", PredicateInfo {
            arity: 3,
            args: &["post_uri", "root_uri", "rkey"],
            description: "Thread membership (which root a reply belongs to)",
        });

        // Directive predicates
        m.insert("has_value", PredicateInfo {
            arity: 2,
            args: &["content", "rkey"],
            description: "Your active values",
        });
        m.insert("has_interest", PredicateInfo {
            arity: 2,
            args: &["content", "rkey"],
            description: "Your active interests",
        });
        m.insert("has_belief", PredicateInfo {
            arity: 2,
            args: &["content", "rkey"],
            description: "Your active beliefs",
        });
        m.insert("has_guideline", PredicateInfo {
            arity: 2,
            args: &["content", "rkey"],
            description: "Your active guidelines",
        });
        m.insert("has_boundary", PredicateInfo {
            arity: 2,
            args: &["content", "rkey"],
            description: "Your active boundaries",
        });
        m.insert("has_aspiration", PredicateInfo {
            arity: 2,
            args: &["content", "rkey"],
            description: "Your active aspirations",
        });
        m.insert("has_self_concept", PredicateInfo {
            arity: 2,
            args: &["content", "rkey"],
            description: "Your active self-concepts",
        });

        // Tool and job predicates
        m.insert("has_tool", PredicateInfo {
            arity: 3,
            args: &["name", "approved", "rkey"],
            description: "Your custom tools (approved: true/false)",
        });
        m.insert("has_job", PredicateInfo {
            arity: 3,
            args: &["name", "schedule_type", "rkey"],
            description: "Your scheduled jobs (once/interval)",
        });

        // Note predicates
        m.insert("has_note", PredicateInfo {
            arity: 6,
            args: &["uri", "title", "category", "created_at", "last_updated", "rkey"],
            description: "Your notes",
        });
        m.insert("note_tag", PredicateInfo {
            arity: 3,
            args: &["note_uri", "tag", "rkey"],
            description: "Tags on notes (one row per tag)",
        });
        m.insert("note_related_fact", PredicateInfo {
            arity: 3,
            args: &["note_uri", "fact_uri", "rkey"],
            description: "Facts linked to notes",
        });

        // Thought predicates
        m.insert("has_thought", PredicateInfo {
            arity: 5,
            args: &["uri", "kind", "trigger", "created_at", "rkey"],
            description: "Your stream of consciousness",
        });

        // Blog predicates
        m.insert("has_blog_post", PredicateInfo {
            arity: 6,
            args: &["uri", "title", "whtwnd_url", "created_at", "is_draft", "rkey"],
            description: "Your WhiteWind blog posts",
        });

        // Fact tags
        m.insert("fact_tag", PredicateInfo {
            arity: 3,
            args: &["fact_uri", "tag", "rkey"],
            description: "Tags on facts (one row per tag)",
        });

        m
    }

    /// Get arities for all derived predicates (backward compatible).
    ///
    /// All predicates include rkey as the last argument, except `is_followed_by`
    /// which comes from external API data and has no rkey.
    pub fn arities() -> HashMap<&'static str, usize> {
        Self::predicate_info()
            .into_iter()
            .map(|(name, info)| (name, info.arity))
            .collect()
    }

    /// Handle a cache update event.
    pub fn handle_update(&mut self, update: &CacheUpdate) {
        match update {
            // Bluesky records
            CacheUpdate::FollowCreated { rkey, follow } => {
                self.add_follow(rkey.clone(), follow);
            }
            CacheUpdate::FollowDeleted { rkey } => {
                self.remove_follow(rkey);
            }
            CacheUpdate::LikeCreated { rkey, like } => {
                self.add_like(rkey.clone(), like);
            }
            CacheUpdate::LikeDeleted { rkey } => {
                self.remove_like(rkey);
            }
            CacheUpdate::RepostCreated { rkey, repost } => {
                self.add_repost(rkey.clone(), repost);
            }
            CacheUpdate::RepostDeleted { rkey } => {
                self.remove_repost(rkey);
            }
            CacheUpdate::PostCreated { rkey, post } => {
                self.add_post(rkey.clone(), post);
            }
            CacheUpdate::PostUpdated { rkey, post } => {
                self.add_post(rkey.clone(), post);
            }
            CacheUpdate::PostDeleted { rkey } => {
                self.remove_post(rkey);
            }

            // Winter records
            CacheUpdate::DirectiveCreated { rkey, directive } => {
                self.add_directive(rkey.clone(), directive);
            }
            CacheUpdate::DirectiveUpdated { rkey, directive } => {
                self.add_directive(rkey.clone(), directive);
            }
            CacheUpdate::DirectiveDeleted { rkey } => {
                self.remove_directive(rkey);
            }
            CacheUpdate::ToolCreated { rkey, tool } => {
                self.add_tool(rkey.clone(), tool);
            }
            CacheUpdate::ToolUpdated { rkey, tool } => {
                self.add_tool(rkey.clone(), tool);
            }
            CacheUpdate::ToolDeleted { rkey } => {
                self.remove_tool(rkey);
            }
            CacheUpdate::ToolApprovalCreated { rkey, approval } => {
                self.add_tool_approval(rkey.clone(), approval);
            }
            CacheUpdate::ToolApprovalUpdated { rkey, approval } => {
                self.add_tool_approval(rkey.clone(), approval);
            }
            CacheUpdate::ToolApprovalDeleted { rkey } => {
                self.remove_tool_approval(rkey);
            }
            CacheUpdate::JobCreated { rkey, job } => {
                self.add_job(rkey.clone(), job);
            }
            CacheUpdate::JobUpdated { rkey, job } => {
                self.add_job(rkey.clone(), job);
            }
            CacheUpdate::JobDeleted { rkey } => {
                self.remove_job(rkey);
            }

            // Notes
            CacheUpdate::NoteCreated { rkey, note } => {
                self.add_note(rkey.clone(), note);
            }
            CacheUpdate::NoteUpdated { rkey, note } => {
                self.add_note(rkey.clone(), note);
            }
            CacheUpdate::NoteDeleted { rkey } => {
                self.remove_note(rkey);
            }

            // Thoughts
            CacheUpdate::ThoughtCreated { rkey, thought } => {
                self.add_thought(rkey.clone(), thought);
            }
            CacheUpdate::ThoughtDeleted { rkey } => {
                self.remove_thought(rkey);
            }

            // Blog entries
            CacheUpdate::BlogEntryCreated { rkey, entry } => {
                self.add_blog_entry(rkey.clone(), entry);
            }
            CacheUpdate::BlogEntryUpdated { rkey, entry } => {
                self.add_blog_entry(rkey.clone(), entry);
            }
            CacheUpdate::BlogEntryDeleted { rkey } => {
                self.remove_blog_entry(rkey);
            }

            // Facts (for tags)
            CacheUpdate::FactCreated { rkey, fact } => {
                self.add_fact_tags(rkey.clone(), fact);
            }
            CacheUpdate::FactUpdated { rkey, fact } => {
                self.add_fact_tags(rkey.clone(), fact);
            }
            CacheUpdate::FactDeleted { rkey } => {
                self.remove_fact_tags(rkey);
            }

            // Ignored events
            _ => {}
        }
    }

    // =========================================================================
    // Follow handling
    // =========================================================================

    fn add_follow(&mut self, rkey: String, follow: &Follow) {
        self.follows.insert(rkey, follow.subject.clone());
        self.dirty_predicates.insert("follows".to_string());
    }

    fn remove_follow(&mut self, rkey: &str) {
        if self.follows.remove(rkey).is_some() {
            self.dirty_predicates.insert("follows".to_string());
        }
    }

    // =========================================================================
    // Like handling
    // =========================================================================

    fn add_like(&mut self, rkey: String, like: &Like) {
        self.likes.insert(rkey, like.subject.uri.clone());
        self.dirty_predicates.insert("liked".to_string());
    }

    fn remove_like(&mut self, rkey: &str) {
        if self.likes.remove(rkey).is_some() {
            self.dirty_predicates.insert("liked".to_string());
        }
    }

    // =========================================================================
    // Repost handling
    // =========================================================================

    fn add_repost(&mut self, rkey: String, repost: &Repost) {
        self.reposts.insert(rkey, repost.subject.uri.clone());
        self.dirty_predicates.insert("reposted".to_string());
    }

    fn remove_repost(&mut self, rkey: &str) {
        if self.reposts.remove(rkey).is_some() {
            self.dirty_predicates.insert("reposted".to_string());
        }
    }

    // =========================================================================
    // Post handling
    // =========================================================================

    fn add_post(&mut self, rkey: String, post: &Post) {
        let uri = format!("at://{}/app.bsky.feed.post/{}", self.self_did, rkey);

        let reply_parent = post.reply.as_ref().map(|r| r.parent.uri.clone());
        let reply_root = post.reply.as_ref().map(|r| r.root.uri.clone());
        let quote_uri = post.embed.as_ref().and_then(|e| match e {
            winter_atproto::PostEmbed::Record { record } => Some(record.uri.clone()),
            winter_atproto::PostEmbed::RecordWithMedia { record, .. } => {
                Some(record.record.uri.clone())
            }
            _ => None,
        });

        let had_reply = self
            .posts
            .get(&rkey)
            .map(|p| p.reply_parent.is_some())
            .unwrap_or(false);
        let had_root = self
            .posts
            .get(&rkey)
            .map(|p| p.reply_root.is_some())
            .unwrap_or(false);
        let had_quote = self
            .posts
            .get(&rkey)
            .map(|p| p.quote_uri.is_some())
            .unwrap_or(false);

        self.posts.insert(
            rkey,
            PostMeta {
                uri,
                reply_parent: reply_parent.clone(),
                reply_root: reply_root.clone(),
                quote_uri: quote_uri.clone(),
            },
        );

        self.dirty_predicates.insert("posted".to_string());

        if reply_parent.is_some() || had_reply {
            self.dirty_predicates.insert("replied_to".to_string());
        }
        if reply_root.is_some() || had_root {
            self.dirty_predicates.insert("thread_root".to_string());
        }
        if quote_uri.is_some() || had_quote {
            self.dirty_predicates.insert("quoted".to_string());
        }
    }

    fn remove_post(&mut self, rkey: &str) {
        if let Some(post) = self.posts.remove(rkey) {
            self.dirty_predicates.insert("posted".to_string());
            if post.reply_parent.is_some() {
                self.dirty_predicates.insert("replied_to".to_string());
            }
            if post.reply_root.is_some() {
                self.dirty_predicates.insert("thread_root".to_string());
            }
            if post.quote_uri.is_some() {
                self.dirty_predicates.insert("quoted".to_string());
            }
        }
    }

    // =========================================================================
    // Directive handling
    // =========================================================================

    fn add_directive(&mut self, rkey: String, directive: &Directive) {
        if !directive.active {
            // Inactive directives don't generate facts
            self.remove_directive(&rkey);
            return;
        }

        let predicate = directive_kind_to_predicate(&directive.kind);
        self.dirty_predicates.insert(predicate.to_string());

        self.directives
            .insert(rkey, (directive.kind.clone(), directive.content.clone()));
    }

    fn remove_directive(&mut self, rkey: &str) {
        if let Some((kind, _)) = self.directives.remove(rkey) {
            let predicate = directive_kind_to_predicate(&kind);
            self.dirty_predicates.insert(predicate.to_string());
        }
    }

    // =========================================================================
    // Tool handling
    // =========================================================================

    fn add_tool(&mut self, rkey: String, tool: &CustomTool) {
        // Check if this tool is approved at the current version
        let approved = self
            .tool_approvals
            .get(&rkey)
            .map(|(_, approved, version)| *approved && *version == tool.version)
            .unwrap_or(false);

        self.tools.insert(rkey, (tool.name.clone(), approved));
        self.dirty_predicates.insert("has_tool".to_string());
    }

    fn remove_tool(&mut self, rkey: &str) {
        if self.tools.remove(rkey).is_some() {
            self.dirty_predicates.insert("has_tool".to_string());
        }
    }

    fn add_tool_approval(&mut self, rkey: String, approval: &ToolApproval) {
        let approved = approval.status == ToolApprovalStatus::Approved;
        self.tool_approvals.insert(
            approval.tool_rkey.clone(),
            (rkey, approved, approval.tool_version),
        );

        // Update the corresponding tool's approved status
        if let Some((name, _)) = self.tools.get(&approval.tool_rkey).cloned() {
            self.tools
                .insert(approval.tool_rkey.clone(), (name, approved));
            self.dirty_predicates.insert("has_tool".to_string());
        }
    }

    fn remove_tool_approval(&mut self, rkey: &str) {
        // Find which tool this approval was for
        let tool_rkey = self
            .tool_approvals
            .iter()
            .find(|(_, (approval_rkey, _, _))| approval_rkey == rkey)
            .map(|(tool_rkey, _)| tool_rkey.clone());

        if let Some(tool_rkey) = tool_rkey {
            self.tool_approvals.remove(&tool_rkey);

            // Update the tool to be unapproved
            if let Some((name, _)) = self.tools.get(&tool_rkey).cloned() {
                self.tools.insert(tool_rkey, (name, false));
                self.dirty_predicates.insert("has_tool".to_string());
            }
        }
    }

    // =========================================================================
    // Job handling
    // =========================================================================

    fn add_job(&mut self, rkey: String, job: &Job) {
        let schedule_type = match &job.schedule {
            JobSchedule::Once { .. } => "once",
            JobSchedule::Interval { .. } => "interval",
        };
        self.jobs
            .insert(rkey, (job.name.clone(), schedule_type.to_string()));
        self.dirty_predicates.insert("has_job".to_string());
    }

    fn remove_job(&mut self, rkey: &str) {
        if self.jobs.remove(rkey).is_some() {
            self.dirty_predicates.insert("has_job".to_string());
        }
    }

    // =========================================================================
    // Note handling
    // =========================================================================

    fn add_note(&mut self, rkey: String, note: &Note) {
        let uri = format!("at://{}/diy.razorgirl.winter.note/{}", self.self_did, rkey);

        // Check if tags or related_facts changed
        let had_tags = self
            .notes
            .get(&rkey)
            .map(|m| !m.tags.is_empty())
            .unwrap_or(false);
        let had_related = self
            .notes
            .get(&rkey)
            .map(|m| !m.related_facts.is_empty())
            .unwrap_or(false);

        self.notes.insert(
            rkey,
            NoteMeta {
                uri,
                title: note.title.clone(),
                category: note.category.clone(),
                tags: note.tags.clone(),
                related_facts: note.related_facts.clone(),
                created_at: note.created_at,
                last_updated: note.last_updated,
            },
        );

        self.dirty_predicates.insert("has_note".to_string());
        if !note.tags.is_empty() || had_tags {
            self.dirty_predicates.insert("note_tag".to_string());
        }
        if !note.related_facts.is_empty() || had_related {
            self.dirty_predicates
                .insert("note_related_fact".to_string());
        }
    }

    fn remove_note(&mut self, rkey: &str) {
        if let Some(meta) = self.notes.remove(rkey) {
            self.dirty_predicates.insert("has_note".to_string());
            if !meta.tags.is_empty() {
                self.dirty_predicates.insert("note_tag".to_string());
            }
            if !meta.related_facts.is_empty() {
                self.dirty_predicates
                    .insert("note_related_fact".to_string());
            }
        }
    }

    // =========================================================================
    // Thought handling
    // =========================================================================

    fn add_thought(&mut self, rkey: String, thought: &Thought) {
        let uri = format!(
            "at://{}/diy.razorgirl.winter.thought/{}",
            self.self_did, rkey
        );

        let kind = match thought.kind {
            winter_atproto::ThoughtKind::Insight => "insight",
            winter_atproto::ThoughtKind::Question => "question",
            winter_atproto::ThoughtKind::Plan => "plan",
            winter_atproto::ThoughtKind::Reflection => "reflection",
            winter_atproto::ThoughtKind::Error => "error",
            winter_atproto::ThoughtKind::Response => "response",
            winter_atproto::ThoughtKind::ToolCall => "tool_call",
        };

        self.thoughts.insert(
            rkey,
            ThoughtMeta {
                uri,
                kind: kind.to_string(),
                trigger: thought.trigger.clone(),
                created_at: thought.created_at,
            },
        );

        self.dirty_predicates.insert("has_thought".to_string());
    }

    fn remove_thought(&mut self, rkey: &str) {
        if self.thoughts.remove(rkey).is_some() {
            self.dirty_predicates.insert("has_thought".to_string());
        }
    }

    // =========================================================================
    // Blog entry handling
    // =========================================================================

    fn add_blog_entry(&mut self, rkey: String, entry: &BlogEntry) {
        let uri = format!("at://{}/com.whtwnd.blog.entry/{}", self.self_did, rkey);
        let whtwnd_url = format!("https://whtwnd.com/{}/{}", self.handle, rkey);

        self.blog_entries.insert(
            rkey,
            BlogMeta {
                uri,
                title: entry.title.clone(),
                whtwnd_url,
                created_at: entry.created_at.clone(),
                is_draft: entry.draft,
            },
        );

        self.dirty_predicates.insert("has_blog_post".to_string());
    }

    fn remove_blog_entry(&mut self, rkey: &str) {
        if self.blog_entries.remove(rkey).is_some() {
            self.dirty_predicates.insert("has_blog_post".to_string());
        }
    }

    // =========================================================================
    // Fact tag handling
    // =========================================================================

    fn add_fact_tags(&mut self, rkey: String, fact: &Fact) {
        // Only track if the fact has tags
        let had_tags = self
            .fact_tags
            .get(&rkey)
            .map(|t| !t.is_empty())
            .unwrap_or(false);

        if !fact.tags.is_empty() {
            self.fact_tags.insert(rkey, fact.tags.clone());
            self.dirty_predicates.insert("fact_tag".to_string());
        } else if had_tags {
            self.fact_tags.remove(&rkey);
            self.dirty_predicates.insert("fact_tag".to_string());
        }
    }

    fn remove_fact_tags(&mut self, rkey: &str) {
        if self.fact_tags.remove(rkey).is_some() {
            self.dirty_predicates.insert("fact_tag".to_string());
        }
    }

    // =========================================================================
    // Follower sync (from API)
    // =========================================================================

    /// Update the set of followers from an API sync.
    pub fn set_followers(&mut self, followers: HashSet<String>) {
        if self.followers != followers {
            self.followers = followers;
            self.dirty_predicates.insert("is_followed_by".to_string());
        }
    }

    /// Add a single follower (from Follow notification).
    ///
    /// Returns true if this was a new follower.
    pub fn add_follower(&mut self, did: String) -> bool {
        if self.followers.insert(did) {
            self.dirty_predicates.insert("is_followed_by".to_string());
            true
        } else {
            false
        }
    }

    // =========================================================================
    // TSV file generation
    // =========================================================================

    /// Flush dirty predicates to TSV files.
    pub fn flush_to_dir(&mut self, fact_dir: &Path) -> Result<(), DatalogError> {
        let dirty: HashSet<String> = std::mem::take(&mut self.dirty_predicates);

        if dirty.is_empty() {
            return Ok(());
        }

        debug!(
            predicates = ?dirty,
            notes = self.notes.len(),
            thoughts = self.thoughts.len(),
            blog_entries = self.blog_entries.len(),
            "flushing derived predicates"
        );

        for predicate in dirty {
            self.write_predicate_file(fact_dir, &predicate)?;
        }

        Ok(())
    }

    /// Force regeneration of all derived fact files.
    pub fn regenerate_all(&mut self, fact_dir: &Path) -> Result<(), DatalogError> {
        trace!(
            notes = self.notes.len(),
            thoughts = self.thoughts.len(),
            blog_entries = self.blog_entries.len(),
            follows = self.follows.len(),
            "regenerating all derived fact files"
        );
        for predicate in Self::arities().keys() {
            let count = self.write_predicate_file(fact_dir, predicate)?;
            if count > 0 {
                trace!(predicate, count, "wrote derived predicate file");
            }
        }
        self.dirty_predicates.clear();
        Ok(())
    }

    fn write_predicate_file(&self, fact_dir: &Path, predicate: &str) -> Result<usize, DatalogError> {
        let path = fact_dir.join(format!("{}.facts", predicate));
        let mut file = std::fs::File::create(&path)?;

        // Get count based on predicate (for logging)
        let count = match predicate {
            "follows" => self.follows.len(),
            "is_followed_by" => self.followers.len(),
            "liked" => self.likes.len(),
            "reposted" => self.reposts.len(),
            "posted" | "replied_to" | "quoted" | "thread_root" => self.posts.len(),
            "has_note" | "note_tag" | "note_related_fact" => self.notes.len(),
            "has_thought" => self.thoughts.len(),
            "has_blog_post" => self.blog_entries.len(),
            "has_tool" => self.tools.len(),
            "has_job" => self.jobs.len(),
            _ => 0,
        };

        match predicate {
            "follows" => {
                for (rkey, target) in &self.follows {
                    writeln!(file, "{}\t{}\t{}", self.self_did, target, rkey)?;
                }
            }
            "is_followed_by" => {
                // No rkey - this comes from external API data
                for follower in &self.followers {
                    writeln!(file, "{}\t{}", follower, self.self_did)?;
                }
            }
            "liked" => {
                for (rkey, uri) in &self.likes {
                    writeln!(file, "{}\t{}\t{}", self.self_did, uri, rkey)?;
                }
            }
            "reposted" => {
                for (rkey, uri) in &self.reposts {
                    writeln!(file, "{}\t{}\t{}", self.self_did, uri, rkey)?;
                }
            }
            "posted" => {
                for (rkey, post) in &self.posts {
                    writeln!(file, "{}\t{}\t{}", self.self_did, post.uri, rkey)?;
                }
            }
            "replied_to" => {
                for (rkey, post) in &self.posts {
                    if let Some(ref parent) = post.reply_parent {
                        writeln!(file, "{}\t{}\t{}", post.uri, parent, rkey)?;
                    }
                }
            }
            "quoted" => {
                for (rkey, post) in &self.posts {
                    if let Some(ref quoted) = post.quote_uri {
                        writeln!(file, "{}\t{}\t{}", post.uri, quoted, rkey)?;
                    }
                }
            }
            "thread_root" => {
                for (rkey, post) in &self.posts {
                    if let Some(ref root) = post.reply_root {
                        writeln!(file, "{}\t{}\t{}", post.uri, root, rkey)?;
                    }
                }
            }
            "has_value" => {
                self.write_directive_predicate(&mut file, &DirectiveKind::Value)?;
            }
            "has_interest" => {
                self.write_directive_predicate(&mut file, &DirectiveKind::Interest)?;
            }
            "has_belief" => {
                self.write_directive_predicate(&mut file, &DirectiveKind::Belief)?;
            }
            "has_guideline" => {
                self.write_directive_predicate(&mut file, &DirectiveKind::Guideline)?;
            }
            "has_boundary" => {
                self.write_directive_predicate(&mut file, &DirectiveKind::Boundary)?;
            }
            "has_aspiration" => {
                self.write_directive_predicate(&mut file, &DirectiveKind::Aspiration)?;
            }
            "has_self_concept" => {
                self.write_directive_predicate(&mut file, &DirectiveKind::SelfConcept)?;
            }
            "has_tool" => {
                for (rkey, (name, approved)) in &self.tools {
                    let approved_str = if *approved { "true" } else { "false" };
                    writeln!(file, "{}\t{}\t{}", name, approved_str, rkey)?;
                }
            }
            "has_job" => {
                for (rkey, (name, schedule_type)) in &self.jobs {
                    writeln!(file, "{}\t{}\t{}", name, schedule_type, rkey)?;
                }
            }
            "has_note" => {
                for (rkey, meta) in &self.notes {
                    let category = meta.category.as_deref().unwrap_or("");
                    // Escape tabs and newlines in title
                    let title = escape_tsv(&meta.title);
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        meta.uri, title, category, meta.created_at, meta.last_updated, rkey
                    )?;
                }
            }
            "note_tag" => {
                for (rkey, meta) in &self.notes {
                    for tag in &meta.tags {
                        writeln!(file, "{}\t{}\t{}", meta.uri, tag, rkey)?;
                    }
                }
            }
            "note_related_fact" => {
                for (rkey, meta) in &self.notes {
                    for fact_uri in &meta.related_facts {
                        writeln!(file, "{}\t{}\t{}", meta.uri, fact_uri, rkey)?;
                    }
                }
            }
            "has_thought" => {
                for (rkey, meta) in &self.thoughts {
                    let trigger = meta.trigger.as_deref().unwrap_or("");
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}",
                        meta.uri, meta.kind, trigger, meta.created_at, rkey
                    )?;
                }
            }
            "has_blog_post" => {
                for (rkey, meta) in &self.blog_entries {
                    let title = escape_tsv(&meta.title);
                    let is_draft = if meta.is_draft { "true" } else { "false" };
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        meta.uri, title, meta.whtwnd_url, meta.created_at, is_draft, rkey
                    )?;
                }
            }
            "fact_tag" => {
                for (rkey, tags) in &self.fact_tags {
                    let uri = format!("at://{}/diy.razorgirl.winter.fact/{}", self.self_did, rkey);
                    for tag in tags {
                        writeln!(file, "{}\t{}\t{}", uri, tag, rkey)?;
                    }
                }
            }
            _ => {
                trace!(predicate, "unknown derived predicate");
            }
        }

        trace!(predicate, path = ?path, count, "wrote derived predicate file");
        Ok(count)
    }

    fn write_directive_predicate(
        &self,
        file: &mut std::fs::File,
        kind: &DirectiveKind,
    ) -> Result<(), DatalogError> {
        for (rkey, (k, content)) in &self.directives {
            if k == kind {
                // Escape tabs and newlines in content
                let escaped = content.replace('\t', " ").replace('\n', " ");
                writeln!(file, "{}\t{}", escaped, rkey)?;
            }
        }
        Ok(())
    }

    /// Get statistics about derived facts.
    pub fn stats(&self) -> DerivedFactStats {
        DerivedFactStats {
            follows: self.follows.len(),
            followers: self.followers.len(),
            likes: self.likes.len(),
            reposts: self.reposts.len(),
            posts: self.posts.len(),
            directives: self.directives.len(),
            tools: self.tools.len(),
            jobs: self.jobs.len(),
            notes: self.notes.len(),
            thoughts: self.thoughts.len(),
            blog_entries: self.blog_entries.len(),
        }
    }

    /// Check if any predicates are dirty.
    pub fn has_dirty_predicates(&self) -> bool {
        !self.dirty_predicates.is_empty()
    }

    /// Mark all predicates as dirty (for initial population).
    pub fn mark_all_dirty(&mut self) {
        for predicate in Self::arities().keys() {
            self.dirty_predicates.insert((*predicate).to_string());
        }
    }
}

/// Statistics about derived facts.
#[derive(Debug, Clone)]
pub struct DerivedFactStats {
    pub follows: usize,
    pub followers: usize,
    pub likes: usize,
    pub reposts: usize,
    pub posts: usize,
    pub directives: usize,
    pub tools: usize,
    pub jobs: usize,
    pub notes: usize,
    pub thoughts: usize,
    pub blog_entries: usize,
}

/// Escape tabs and newlines in a string for TSV output.
fn escape_tsv(s: &str) -> String {
    s.replace('\t', " ").replace('\n', " ")
}

/// Convert a DirectiveKind to its corresponding predicate name.
fn directive_kind_to_predicate(kind: &DirectiveKind) -> &'static str {
    match kind {
        DirectiveKind::Value => "has_value",
        DirectiveKind::Interest => "has_interest",
        DirectiveKind::Belief => "has_belief",
        DirectiveKind::Guideline => "has_guideline",
        DirectiveKind::Boundary => "has_boundary",
        DirectiveKind::Aspiration => "has_aspiration",
        DirectiveKind::SelfConcept => "has_self_concept",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use winter_atproto::{StrongRef, ThoughtKind};

    // =========================================================================
    // Factory Functions
    // =========================================================================

    fn make_follow(subject: &str) -> Follow {
        Follow {
            subject: subject.to_string(),
            created_at: Utc::now(),
        }
    }

    fn make_like(uri: &str) -> Like {
        Like {
            subject: StrongRef {
                uri: uri.to_string(),
                cid: "cid123".to_string(),
            },
            created_at: Utc::now(),
        }
    }

    fn make_note(
        title: &str,
        category: Option<&str>,
        tags: Vec<&str>,
        related_facts: Vec<&str>,
    ) -> Note {
        Note {
            title: title.to_string(),
            content: "test content".to_string(),
            category: category.map(String::from),
            tags: tags.into_iter().map(String::from).collect(),
            related_facts: related_facts.into_iter().map(String::from).collect(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
        }
    }

    fn make_thought(kind: ThoughtKind, trigger: Option<&str>) -> Thought {
        Thought {
            kind,
            content: "test thought content".to_string(),
            trigger: trigger.map(String::from),
            duration_ms: None,
            created_at: Utc::now(),
        }
    }

    fn make_blog_entry(title: &str, draft: bool) -> BlogEntry {
        BlogEntry {
            title: title.to_string(),
            content: "test blog content".to_string(),
            created_at: Utc::now().to_rfc3339(),
            draft,
            theme: None,
            ogp: None,
        }
    }

    fn make_fact_with_tags(predicate: &str, args: Vec<&str>, tags: Vec<&str>) -> Fact {
        Fact {
            predicate: predicate.to_string(),
            args: args.into_iter().map(String::from).collect(),
            confidence: None,
            source: None,
            supersedes: None,
            tags: tags.into_iter().map(String::from).collect(),
            created_at: Utc::now(),
        }
    }

    // =========================================================================
    // Existing Tests
    // =========================================================================

    #[test]
    fn test_is_derived() {
        assert!(DerivedFactGenerator::is_derived("follows"));
        assert!(DerivedFactGenerator::is_derived("is_followed_by"));
        assert!(DerivedFactGenerator::is_derived("has_value"));
        assert!(DerivedFactGenerator::is_derived("has_tool"));
        // Thread predicates
        assert!(DerivedFactGenerator::is_derived("replied_to"));
        assert!(DerivedFactGenerator::is_derived("thread_root"));
        // New predicates
        assert!(DerivedFactGenerator::is_derived("has_note"));
        assert!(DerivedFactGenerator::is_derived("note_tag"));
        assert!(DerivedFactGenerator::is_derived("note_related_fact"));
        assert!(DerivedFactGenerator::is_derived("has_thought"));
        assert!(DerivedFactGenerator::is_derived("has_blog_post"));
        assert!(DerivedFactGenerator::is_derived("fact_tag"));
        // Non-derived
        assert!(!DerivedFactGenerator::is_derived("custom_predicate"));
        assert!(!DerivedFactGenerator::is_derived("some_user_fact"));
    }

    #[test]
    fn test_arities() {
        let arities = DerivedFactGenerator::arities();
        // Bluesky predicates (with rkey at end, except is_followed_by)
        assert_eq!(arities.get("follows"), Some(&3)); // (self, target, rkey)
        assert_eq!(arities.get("is_followed_by"), Some(&2)); // (follower, self) - no rkey
        assert_eq!(arities.get("replied_to"), Some(&3)); // (post_uri, parent, rkey)
        assert_eq!(arities.get("thread_root"), Some(&3)); // (post_uri, root, rkey)
        // Directive predicates (with rkey at end)
        assert_eq!(arities.get("has_value"), Some(&2)); // (content, rkey)
        assert_eq!(arities.get("has_tool"), Some(&3)); // (name, approved, rkey)
        // Note predicates (with rkey at end)
        assert_eq!(arities.get("has_note"), Some(&6)); // (uri, title, cat, created, updated, rkey)
        assert_eq!(arities.get("note_tag"), Some(&3)); // (uri, tag, rkey)
        assert_eq!(arities.get("note_related_fact"), Some(&3)); // (note_uri, fact_uri, rkey)
        // Thought predicates (with rkey at end)
        assert_eq!(arities.get("has_thought"), Some(&5)); // (uri, kind, trigger, created, rkey)
        // Blog predicates (with rkey at end)
        assert_eq!(arities.get("has_blog_post"), Some(&6)); // (uri, title, url, created, draft, rkey)
        // Fact tags (with rkey at end)
        assert_eq!(arities.get("fact_tag"), Some(&3)); // (uri, tag, rkey)
    }

    #[test]
    fn test_follow_handling() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        generator.handle_update(&CacheUpdate::FollowCreated {
            rkey: "rkey1".to_string(),
            follow: make_follow("did:plc:target"),
        });

        assert_eq!(generator.follows.len(), 1);
        assert!(generator.dirty_predicates.contains("follows"));

        generator.handle_update(&CacheUpdate::FollowDeleted {
            rkey: "rkey1".to_string(),
        });

        assert_eq!(generator.follows.len(), 0);
    }

    #[test]
    fn test_like_handling() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        generator.handle_update(&CacheUpdate::LikeCreated {
            rkey: "rkey1".to_string(),
            like: make_like("at://did:plc:author/app.bsky.feed.post/abc"),
        });

        assert_eq!(generator.likes.len(), 1);
        assert!(generator.dirty_predicates.contains("liked"));
    }

    #[test]
    fn test_followers_sync() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        let followers: HashSet<String> = vec!["did:plc:a".to_string(), "did:plc:b".to_string()]
            .into_iter()
            .collect();

        generator.set_followers(followers);

        assert_eq!(generator.followers.len(), 2);
        assert!(generator.dirty_predicates.contains("is_followed_by"));
    }

    #[test]
    fn test_add_follower_incremental() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        // Add first follower
        assert!(generator.add_follower("did:plc:a".to_string()));
        assert_eq!(generator.followers.len(), 1);
        assert!(generator.dirty_predicates.contains("is_followed_by"));

        generator.dirty_predicates.clear();

        // Add second follower
        assert!(generator.add_follower("did:plc:b".to_string()));
        assert_eq!(generator.followers.len(), 2);
        assert!(generator.dirty_predicates.contains("is_followed_by"));

        generator.dirty_predicates.clear();

        // Adding same follower again returns false (already exists)
        assert!(!generator.add_follower("did:plc:a".to_string()));
        assert_eq!(generator.followers.len(), 2);
        // Dirty should NOT be set since no change occurred
        assert!(!generator.dirty_predicates.contains("is_followed_by"));
    }

    #[test]
    fn test_directive_kind_to_predicate() {
        assert_eq!(
            directive_kind_to_predicate(&DirectiveKind::Value),
            "has_value"
        );
        assert_eq!(
            directive_kind_to_predicate(&DirectiveKind::Interest),
            "has_interest"
        );
        assert_eq!(
            directive_kind_to_predicate(&DirectiveKind::SelfConcept),
            "has_self_concept"
        );
    }

    // =========================================================================
    // Note Handler Tests
    // =========================================================================

    #[test]
    fn test_note_handling() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        // Create note with tags and related facts
        generator.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note(
                "Test Note",
                Some("research"),
                vec!["tag1", "tag2"],
                vec!["at://did:plc:winter/diy.razorgirl.winter.fact/fact1"],
            ),
        });

        // Verify state
        assert_eq!(generator.notes.len(), 1);
        let meta = generator.notes.get("note1").unwrap();
        assert_eq!(meta.title, "Test Note");
        assert_eq!(meta.category, Some("research".to_string()));
        assert_eq!(meta.tags.len(), 2);
        assert_eq!(meta.related_facts.len(), 1);
        assert_eq!(
            meta.uri,
            "at://did:plc:winter/diy.razorgirl.winter.note/note1"
        );

        // Verify dirty predicates
        assert!(generator.dirty_predicates.contains("has_note"));
        assert!(generator.dirty_predicates.contains("note_tag"));
        assert!(generator.dirty_predicates.contains("note_related_fact"));

        // Clear dirty flags for update test
        generator.dirty_predicates.clear();

        // Update note (change title, add tag)
        generator.handle_update(&CacheUpdate::NoteUpdated {
            rkey: "note1".to_string(),
            note: make_note(
                "Updated Note",
                Some("research"),
                vec!["tag1", "tag2", "tag3"],
                vec!["at://did:plc:winter/diy.razorgirl.winter.fact/fact1"],
            ),
        });

        let meta = generator.notes.get("note1").unwrap();
        assert_eq!(meta.title, "Updated Note");
        assert_eq!(meta.tags.len(), 3);
        assert!(generator.dirty_predicates.contains("has_note"));
        assert!(generator.dirty_predicates.contains("note_tag"));

        // Clear dirty flags for delete test
        generator.dirty_predicates.clear();

        // Delete note
        generator.handle_update(&CacheUpdate::NoteDeleted {
            rkey: "note1".to_string(),
        });

        assert_eq!(generator.notes.len(), 0);
        assert!(generator.dirty_predicates.contains("has_note"));
        assert!(generator.dirty_predicates.contains("note_tag"));
        assert!(generator.dirty_predicates.contains("note_related_fact"));
    }

    // =========================================================================
    // Thought Handler Tests
    // =========================================================================

    #[test]
    fn test_thought_handling() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        // Create thought with trigger
        generator.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought1".to_string(),
            thought: make_thought(ThoughtKind::Reflection, Some("notification from user")),
        });

        // Verify state
        assert_eq!(generator.thoughts.len(), 1);
        let meta = generator.thoughts.get("thought1").unwrap();
        assert_eq!(meta.kind, "reflection");
        assert_eq!(meta.trigger, Some("notification from user".to_string()));
        assert_eq!(
            meta.uri,
            "at://did:plc:winter/diy.razorgirl.winter.thought/thought1"
        );

        // Verify dirty predicates
        assert!(generator.dirty_predicates.contains("has_thought"));

        // Clear dirty flags
        generator.dirty_predicates.clear();

        // Create thought without trigger
        generator.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought2".to_string(),
            thought: make_thought(ThoughtKind::Insight, None),
        });

        assert_eq!(generator.thoughts.len(), 2);
        let meta2 = generator.thoughts.get("thought2").unwrap();
        assert_eq!(meta2.kind, "insight");
        assert!(meta2.trigger.is_none());

        // Clear dirty flags for delete test
        generator.dirty_predicates.clear();

        // Delete thought
        generator.handle_update(&CacheUpdate::ThoughtDeleted {
            rkey: "thought1".to_string(),
        });

        assert_eq!(generator.thoughts.len(), 1);
        assert!(generator.dirty_predicates.contains("has_thought"));
    }

    // =========================================================================
    // Blog Entry Handler Tests
    // =========================================================================

    #[test]
    fn test_blog_entry_handling() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        // Create draft blog entry
        generator.handle_update(&CacheUpdate::BlogEntryCreated {
            rkey: "blog1".to_string(),
            entry: make_blog_entry("My Draft Post", true),
        });

        // Verify state
        assert_eq!(generator.blog_entries.len(), 1);
        let meta = generator.blog_entries.get("blog1").unwrap();
        assert_eq!(meta.title, "My Draft Post");
        assert!(meta.is_draft);
        assert_eq!(
            meta.uri,
            "at://did:plc:winter/com.whtwnd.blog.entry/blog1"
        );
        assert_eq!(meta.whtwnd_url, "https://whtwnd.com/winter.test/blog1");

        // Verify dirty predicates
        assert!(generator.dirty_predicates.contains("has_blog_post"));

        // Clear dirty flags
        generator.dirty_predicates.clear();

        // Create published blog entry
        generator.handle_update(&CacheUpdate::BlogEntryCreated {
            rkey: "blog2".to_string(),
            entry: make_blog_entry("Published Post", false),
        });

        assert_eq!(generator.blog_entries.len(), 2);
        let meta2 = generator.blog_entries.get("blog2").unwrap();
        assert!(!meta2.is_draft);

        // Clear dirty flags for update test
        generator.dirty_predicates.clear();

        // Update blog entry (publish the draft)
        generator.handle_update(&CacheUpdate::BlogEntryUpdated {
            rkey: "blog1".to_string(),
            entry: make_blog_entry("My Draft Post", false),
        });

        let meta = generator.blog_entries.get("blog1").unwrap();
        assert!(!meta.is_draft);
        assert!(generator.dirty_predicates.contains("has_blog_post"));

        // Clear dirty flags for delete test
        generator.dirty_predicates.clear();

        // Delete blog entry
        generator.handle_update(&CacheUpdate::BlogEntryDeleted {
            rkey: "blog1".to_string(),
        });

        assert_eq!(generator.blog_entries.len(), 1);
        assert!(generator.dirty_predicates.contains("has_blog_post"));
    }

    // =========================================================================
    // Fact Tag Handler Tests
    // =========================================================================

    #[test]
    fn test_fact_tag_handling() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        // Create fact with tags
        generator.handle_update(&CacheUpdate::FactCreated {
            rkey: "fact1".to_string(),
            fact: make_fact_with_tags("knows", vec!["alice", "bob"], vec!["social", "verified"]),
        });

        // Verify state
        assert_eq!(generator.fact_tags.len(), 1);
        let tags = generator.fact_tags.get("fact1").unwrap();
        assert_eq!(tags.len(), 2);
        assert!(tags.contains(&"social".to_string()));
        assert!(tags.contains(&"verified".to_string()));

        // Verify dirty predicates
        assert!(generator.dirty_predicates.contains("fact_tag"));

        // Clear dirty flags
        generator.dirty_predicates.clear();

        // Update fact (change tags)
        generator.handle_update(&CacheUpdate::FactUpdated {
            rkey: "fact1".to_string(),
            fact: make_fact_with_tags(
                "knows",
                vec!["alice", "bob"],
                vec!["social", "verified", "important"],
            ),
        });

        let tags = generator.fact_tags.get("fact1").unwrap();
        assert_eq!(tags.len(), 3);
        assert!(generator.dirty_predicates.contains("fact_tag"));

        // Clear dirty flags for delete test
        generator.dirty_predicates.clear();

        // Delete fact
        generator.handle_update(&CacheUpdate::FactDeleted {
            rkey: "fact1".to_string(),
        });

        assert_eq!(generator.fact_tags.len(), 0);
        assert!(generator.dirty_predicates.contains("fact_tag"));
    }

    #[test]
    fn test_fact_without_tags_not_tracked() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        // Create fact without tags
        generator.handle_update(&CacheUpdate::FactCreated {
            rkey: "fact1".to_string(),
            fact: make_fact_with_tags("knows", vec!["alice", "bob"], vec![]),
        });

        // Should not be tracked in fact_tags
        assert_eq!(generator.fact_tags.len(), 0);
        // Should not mark fact_tag as dirty
        assert!(!generator.dirty_predicates.contains("fact_tag"));
    }

    #[test]
    fn test_fact_tags_removed_on_update() {
        let mut generator = DerivedFactGenerator::new("did:plc:winter", "winter.test");

        // Create fact with tags
        generator.handle_update(&CacheUpdate::FactCreated {
            rkey: "fact1".to_string(),
            fact: make_fact_with_tags("knows", vec!["alice"], vec!["tagged"]),
        });

        assert_eq!(generator.fact_tags.len(), 1);
        generator.dirty_predicates.clear();

        // Update to remove all tags
        generator.handle_update(&CacheUpdate::FactUpdated {
            rkey: "fact1".to_string(),
            fact: make_fact_with_tags("knows", vec!["alice"], vec![]),
        });

        // Should be removed from tracking
        assert_eq!(generator.fact_tags.len(), 0);
        assert!(generator.dirty_predicates.contains("fact_tag"));
    }

    // =========================================================================
    // TSV Generation Tests
    // =========================================================================

    #[test]
    fn test_has_note_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note("My Note", Some("research"), vec![], vec![]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("has_note.facts")).unwrap();
        assert!(content.contains("at://did:plc:test/diy.razorgirl.winter.note/note1"));
        assert!(content.contains("My Note"));
        assert!(content.contains("research"));
    }

    #[test]
    fn test_note_tag_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note("Tagged Note", None, vec!["alpha", "beta", "gamma"], vec![]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("note_tag.facts")).unwrap();
        let uri = "at://did:plc:test/diy.razorgirl.winter.note/note1";

        // Each tag should produce a row: uri\ttag
        assert!(content.contains(&format!("{}\talpha", uri)));
        assert!(content.contains(&format!("{}\tbeta", uri)));
        assert!(content.contains(&format!("{}\tgamma", uri)));

        // Count lines (should be 3)
        let line_count = content.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(line_count, 3);
    }

    #[test]
    fn test_note_related_fact_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        let fact_uri1 = "at://did:plc:test/diy.razorgirl.winter.fact/abc123";
        let fact_uri2 = "at://did:plc:test/diy.razorgirl.winter.fact/def456";

        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note("Note with Facts", None, vec![], vec![fact_uri1, fact_uri2]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("note_related_fact.facts")).unwrap();
        let note_uri = "at://did:plc:test/diy.razorgirl.winter.note/note1";

        // Each related fact should produce a row
        assert!(content.contains(&format!("{}\t{}", note_uri, fact_uri1)));
        assert!(content.contains(&format!("{}\t{}", note_uri, fact_uri2)));

        let line_count = content.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(line_count, 2);
    }

    #[test]
    fn test_has_thought_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Thought with trigger
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought1".to_string(),
            thought: make_thought(ThoughtKind::Reflection, Some("awaken cycle")),
        });

        // Thought without trigger
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought2".to_string(),
            thought: make_thought(ThoughtKind::Question, None),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("has_thought.facts")).unwrap();

        // Verify both thoughts present
        assert!(content.contains("at://did:plc:test/diy.razorgirl.winter.thought/thought1"));
        assert!(content.contains("reflection"));
        assert!(content.contains("awaken cycle"));

        assert!(content.contains("at://did:plc:test/diy.razorgirl.winter.thought/thought2"));
        assert!(content.contains("question"));

        let line_count = content.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(line_count, 2);
    }

    #[test]
    fn test_has_blog_post_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Draft post
        dfg.handle_update(&CacheUpdate::BlogEntryCreated {
            rkey: "draft1".to_string(),
            entry: make_blog_entry("Draft Title", true),
        });

        // Published post
        dfg.handle_update(&CacheUpdate::BlogEntryCreated {
            rkey: "published1".to_string(),
            entry: make_blog_entry("Published Title", false),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("has_blog_post.facts")).unwrap();

        // Verify draft entry
        assert!(content.contains("at://did:plc:test/com.whtwnd.blog.entry/draft1"));
        assert!(content.contains("Draft Title"));
        assert!(content.contains("https://whtwnd.com/test.handle/draft1"));
        assert!(content.contains("true")); // is_draft

        // Verify published entry
        assert!(content.contains("at://did:plc:test/com.whtwnd.blog.entry/published1"));
        assert!(content.contains("Published Title"));
        assert!(content.contains("https://whtwnd.com/test.handle/published1"));
        assert!(content.contains("false")); // is_draft

        let line_count = content.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(line_count, 2);
    }

    #[test]
    fn test_fact_tag_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        dfg.handle_update(&CacheUpdate::FactCreated {
            rkey: "fact1".to_string(),
            fact: make_fact_with_tags("knows", vec!["alice"], vec!["social", "verified"]),
        });

        dfg.handle_update(&CacheUpdate::FactCreated {
            rkey: "fact2".to_string(),
            fact: make_fact_with_tags("likes", vec!["pizza"], vec!["preference"]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("fact_tag.facts")).unwrap();

        let fact1_uri = "at://did:plc:test/diy.razorgirl.winter.fact/fact1";
        let fact2_uri = "at://did:plc:test/diy.razorgirl.winter.fact/fact2";

        assert!(content.contains(&format!("{}\tsocial", fact1_uri)));
        assert!(content.contains(&format!("{}\tverified", fact1_uri)));
        assert!(content.contains(&format!("{}\tpreference", fact2_uri)));

        let line_count = content.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(line_count, 3);
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_escape_tsv_special_characters() {
        // Test the escape_tsv function directly
        assert_eq!(escape_tsv("normal"), "normal");
        assert_eq!(escape_tsv("has\ttab"), "has tab");
        assert_eq!(escape_tsv("has\nnewline"), "has newline");
        assert_eq!(escape_tsv("combo\t\n"), "combo  ");
        assert_eq!(escape_tsv(""), "");
    }

    #[test]
    fn test_empty_optional_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Note without category
        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note("No Category Note", None, vec![], vec![]),
        });

        // Thought without trigger
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought1".to_string(),
            thought: make_thought(ThoughtKind::Plan, None),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // has_note should have empty category field
        let note_content = std::fs::read_to_string(dir.path().join("has_note.facts")).unwrap();
        // Format: uri\ttitle\tcategory\tcreated_at\tlast_updated
        // When category is None, it should be empty string
        let lines: Vec<&str> = note_content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parts: Vec<&str> = lines[0].split('\t').collect();
        assert_eq!(parts[2], ""); // empty category

        // has_thought should have empty trigger field
        let thought_content =
            std::fs::read_to_string(dir.path().join("has_thought.facts")).unwrap();
        let lines: Vec<&str> = thought_content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parts: Vec<&str> = lines[0].split('\t').collect();
        assert_eq!(parts[2], ""); // empty trigger
    }

    #[test]
    fn test_empty_arrays() {
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Note with empty tags and related_facts
        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note("Empty Arrays Note", None, vec![], vec![]),
        });

        // Fact with empty tags
        dfg.handle_update(&CacheUpdate::FactCreated {
            rkey: "fact1".to_string(),
            fact: make_fact_with_tags("knows", vec!["someone"], vec![]),
        });

        // For notes with empty tags/related_facts, the predicates aren't marked dirty
        // since there's nothing to write
        assert!(!dfg.dirty_predicates.contains("note_tag"));
        assert!(!dfg.dirty_predicates.contains("note_related_fact"));

        // For facts without tags, fact_tag isn't marked dirty
        assert!(!dfg.dirty_predicates.contains("fact_tag"));

        // Verify only has_note is dirty (not the array predicates)
        assert!(dfg.dirty_predicates.contains("has_note"));
    }

    #[test]
    fn test_tsv_escaping_in_note_title() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Note with special characters in title
        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note(
                "Title\twith\ttabs\nand\nnewlines",
                Some("test"),
                vec![],
                vec![],
            ),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("has_note.facts")).unwrap();

        // Title should be escaped - no raw tabs or newlines
        assert!(!content.contains("Title\twith"));
        assert!(content.contains("Title with tabs and newlines"));
    }

    #[test]
    fn test_tsv_escaping_in_blog_title() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Blog entry with special characters in title
        dfg.handle_update(&CacheUpdate::BlogEntryCreated {
            rkey: "blog1".to_string(),
            entry: BlogEntry {
                title: "Blog\twith\ttabs".to_string(),
                content: "content".to_string(),
                created_at: Utc::now().to_rfc3339(),
                draft: false,
                theme: None,
                ogp: None,
            },
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("has_blog_post.facts")).unwrap();

        // Title should be escaped
        assert!(content.contains("Blog with tabs"));
    }

    // =========================================================================
    // Full Flush Cycle Test
    // =========================================================================

    #[test]
    fn test_full_flush_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Add various records
        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note(
                "Test Note",
                Some("research"),
                vec!["tag1"],
                vec!["at://did:plc:test/diy.razorgirl.winter.fact/f1"],
            ),
        });

        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought1".to_string(),
            thought: make_thought(ThoughtKind::Reflection, Some("test")),
        });

        dfg.handle_update(&CacheUpdate::BlogEntryCreated {
            rkey: "blog1".to_string(),
            entry: make_blog_entry("Test Blog", false),
        });

        dfg.handle_update(&CacheUpdate::FactCreated {
            rkey: "fact1".to_string(),
            fact: make_fact_with_tags("test", vec!["arg"], vec!["tagged"]),
        });

        // Verify all predicates dirty
        assert!(dfg.has_dirty_predicates());
        assert!(dfg.dirty_predicates.contains("has_note"));
        assert!(dfg.dirty_predicates.contains("note_tag"));
        assert!(dfg.dirty_predicates.contains("note_related_fact"));
        assert!(dfg.dirty_predicates.contains("has_thought"));
        assert!(dfg.dirty_predicates.contains("has_blog_post"));
        assert!(dfg.dirty_predicates.contains("fact_tag"));

        // Flush
        dfg.flush_to_dir(dir.path()).unwrap();

        // Verify dirty cleared
        assert!(!dfg.has_dirty_predicates());

        // Verify all files exist and have content
        assert!(dir.path().join("has_note.facts").exists());
        assert!(dir.path().join("note_tag.facts").exists());
        assert!(dir.path().join("note_related_fact.facts").exists());
        assert!(dir.path().join("has_thought.facts").exists());
        assert!(dir.path().join("has_blog_post.facts").exists());
        assert!(dir.path().join("fact_tag.facts").exists());

        // Verify files have non-empty content
        let has_note = std::fs::read_to_string(dir.path().join("has_note.facts")).unwrap();
        assert!(!has_note.is_empty());

        let note_tag = std::fs::read_to_string(dir.path().join("note_tag.facts")).unwrap();
        assert!(!note_tag.is_empty());

        let note_related =
            std::fs::read_to_string(dir.path().join("note_related_fact.facts")).unwrap();
        assert!(!note_related.is_empty());

        let has_thought = std::fs::read_to_string(dir.path().join("has_thought.facts")).unwrap();
        assert!(!has_thought.is_empty());

        let has_blog = std::fs::read_to_string(dir.path().join("has_blog_post.facts")).unwrap();
        assert!(!has_blog.is_empty());

        let fact_tag = std::fs::read_to_string(dir.path().join("fact_tag.facts")).unwrap();
        assert!(!fact_tag.is_empty());
    }

    #[test]
    fn test_multiple_notes_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Add multiple notes
        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note1".to_string(),
            note: make_note("First Note", Some("cat1"), vec!["shared"], vec![]),
        });

        dfg.handle_update(&CacheUpdate::NoteCreated {
            rkey: "note2".to_string(),
            note: make_note("Second Note", Some("cat2"), vec!["shared", "unique"], vec![]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let has_note = std::fs::read_to_string(dir.path().join("has_note.facts")).unwrap();
        let note_tag = std::fs::read_to_string(dir.path().join("note_tag.facts")).unwrap();

        // Both notes should be in has_note
        assert!(has_note.contains("note1"));
        assert!(has_note.contains("note2"));
        assert_eq!(has_note.lines().filter(|l| !l.is_empty()).count(), 2);

        // Should have 3 total tags (1 + 2)
        assert_eq!(note_tag.lines().filter(|l| !l.is_empty()).count(), 3);
    }

    #[test]
    fn test_thought_kinds_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Test all thought kinds
        let kinds = vec![
            (ThoughtKind::Insight, "insight"),
            (ThoughtKind::Question, "question"),
            (ThoughtKind::Plan, "plan"),
            (ThoughtKind::Reflection, "reflection"),
            (ThoughtKind::Error, "error"),
            (ThoughtKind::Response, "response"),
            (ThoughtKind::ToolCall, "tool_call"),
        ];

        for (i, (kind, _expected_str)) in kinds.iter().enumerate() {
            dfg.handle_update(&CacheUpdate::ThoughtCreated {
                rkey: format!("thought{}", i),
                thought: make_thought(kind.clone(), None),
            });
        }

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("has_thought.facts")).unwrap();

        // Verify all kind strings appear
        for (_, expected_str) in &kinds {
            assert!(
                content.contains(expected_str),
                "Expected to find kind '{}' in has_thought.facts",
                expected_str
            );
        }

        assert_eq!(
            content.lines().filter(|l| !l.is_empty()).count(),
            kinds.len()
        );
    }
}
