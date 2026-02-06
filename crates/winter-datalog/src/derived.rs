//! Derived facts generator from PDS records.
//!
//! Generates facts automatically from authoritative PDS state:
//! - Bluesky records (follows, likes, reposts, posts)
//! - Winter records (directives, tools, jobs)
//!
//! These facts exist only in TSV files (not as ATProto fact records)
//! and are regenerated when source records change.

use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Write};
use std::path::Path;

use tracing::{debug, trace};

use chrono::{DateTime, Utc};

use winter_atproto::{
    BlogEntry, CacheUpdate, CustomTool, Directive, DirectiveKind, Fact, Follow, Job, JobSchedule,
    Like, Note, Post, Repost, Thought, ToolApproval, ToolApprovalStatus, Trigger, WikiEntry,
    WikiLink,
};

use crate::error::DatalogError;

/// Metadata about a post for derived facts.
#[derive(Debug, Clone)]
struct PostMeta {
    /// The post's AT URI.
    uri: String,
    /// Parent URI if this is a reply.
    reply_parent: Option<String>,
    /// Parent CID if this is a reply.
    reply_parent_cid: Option<String>,
    /// Root URI if this is a reply (the thread root).
    reply_root: Option<String>,
    /// Root CID if this is a reply.
    reply_root_cid: Option<String>,
    /// Quoted URI if this is a quote post.
    quote_uri: Option<String>,
    /// Quoted CID if this is a quote post.
    quote_cid: Option<String>,
    /// When the post was created.
    created_at: DateTime<Utc>,
    /// Languages the post is written in.
    langs: Vec<String>,
    /// DIDs of accounts mentioned in the post.
    mentions: Vec<String>,
    /// External links in the post.
    links: Vec<String>,
    /// Hashtags in the post.
    hashtags: Vec<String>,
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
    /// Tags for categorization.
    tags: Vec<String>,
    /// Duration in milliseconds (for tool_call thoughts).
    duration_ms: Option<u64>,
    /// Tool name (for tool_call thoughts only).
    tool_name: Option<String>,
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

/// Metadata about a wiki entry for derived facts.
#[derive(Debug, Clone)]
struct WikiEntryMeta {
    /// The wiki entry's AT URI.
    uri: String,
    /// Entry title.
    title: String,
    /// URL-safe slug.
    slug: String,
    /// Lifecycle status.
    status: String,
    /// Alternative names.
    aliases: Vec<String>,
    /// Tags for categorization.
    tags: Vec<String>,
    /// Previous version AT URI.
    supersedes: Option<String>,
    /// When this entry was created.
    created_at: DateTime<Utc>,
    /// When this entry was last updated.
    last_updated: DateTime<Utc>,
}

/// Metadata about a wiki link for derived facts.
#[derive(Debug, Clone)]
struct WikiLinkMeta {
    /// Source AT URI.
    source: String,
    /// Target AT URI.
    target: String,
    /// Semantic link type.
    link_type: String,
    /// When this link was created.
    created_at: DateTime<Utc>,
}

/// Metadata about a like for derived facts.
#[derive(Debug, Clone)]
struct LikeMeta {
    /// The liked post's URI.
    post_uri: String,
    /// The liked post's CID.
    post_cid: String,
    /// When the like was created.
    created_at: DateTime<Utc>,
}

/// Metadata about a repost for derived facts.
#[derive(Debug, Clone)]
struct RepostMeta {
    /// The reposted post's URI.
    post_uri: String,
    /// The reposted post's CID.
    post_cid: String,
    /// When the repost was created.
    created_at: DateTime<Utc>,
}

/// Metadata about a follow for derived facts.
#[derive(Debug, Clone)]
struct FollowMeta {
    /// The followed account's DID.
    target_did: String,
    /// When the follow was created.
    created_at: DateTime<Utc>,
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
    /// Follow records: rkey -> follow metadata.
    follows: HashMap<String, FollowMeta>,
    /// Like records: rkey -> like metadata.
    likes: HashMap<String, LikeMeta>,
    /// Repost records: rkey -> repost metadata.
    reposts: HashMap<String, RepostMeta>,
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
    /// Wiki entry records: rkey -> wiki entry metadata.
    wiki_entries: HashMap<String, WikiEntryMeta>,
    /// Wiki link records: rkey -> wiki link metadata.
    wiki_links: HashMap<String, WikiLinkMeta>,
    /// Fact tags: rkey -> tags.
    fact_tags: HashMap<String, Vec<String>>,
    /// Triggers: rkey -> (name, enabled).
    triggers: HashMap<String, (String, bool)>,

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
            wiki_entries: HashMap::new(),
            wiki_links: HashMap::new(),
            fact_tags: HashMap::new(),
            triggers: HashMap::new(),
            followers: HashSet::new(),
            dirty_predicates: HashSet::new(),
        }
    }

    /// Check if a predicate is derived (cannot be manually deleted).
    pub fn is_derived(predicate: &str) -> bool {
        matches!(
            predicate,
            // Bluesky: follows
            "follows"
                | "follow_created_at"
                | "is_followed_by"
                // Bluesky: likes
                | "liked"
                | "like_created_at"
                | "like_cid"
                // Bluesky: reposts
                | "reposted"
                | "repost_created_at"
                | "repost_cid"
                // Bluesky: posts
                | "posted"
                | "post_created_at"
                | "replied_to"
                | "reply_parent_uri"
                | "reply_parent_cid"
                | "thread_root"
                | "reply_root_uri"
                | "reply_root_cid"
                | "quoted"
                | "quote_cid"
                | "post_lang"
                | "post_mention"
                | "post_link"
                | "post_hashtag"
                // Winter: directives
                | "has_value"
                | "has_interest"
                | "has_belief"
                | "has_guideline"
                | "has_boundary"
                | "has_aspiration"
                | "has_self_concept"
                // Winter: tools, jobs
                | "has_tool"
                | "has_job"
                // Winter: notes
                | "has_note"
                | "note_tag"
                | "note_related_fact"
                // Winter: thoughts
                | "has_thought"
                | "thought_tag"
                | "tool_call_duration"
                // Winter: blog
                | "has_blog_post"
                // Winter: wiki
                | "has_wiki_entry"
                | "wiki_entry_alias"
                | "wiki_entry_tag"
                | "wiki_entry_supersedes"
                | "has_wiki_link"
                // Winter: fact tags
                | "fact_tag"
                // Winter: triggers
                | "has_trigger"
        )
    }

    /// Get full predicate information for all derived predicates.
    ///
    /// All predicates include rkey as the last argument, except `is_followed_by`
    /// which comes from external API data and has no rkey.
    pub fn predicate_info() -> HashMap<&'static str, PredicateInfo> {
        let mut m = HashMap::new();

        // =================================================================
        // Bluesky: follows
        // =================================================================
        m.insert(
            "follows",
            PredicateInfo {
                arity: 3,
                args: &["self_did", "target_did", "rkey"],
                description: "Accounts you follow",
            },
        );
        m.insert(
            "follow_created_at",
            PredicateInfo {
                arity: 4,
                args: &["self_did", "target_did", "timestamp", "rkey"],
                description: "When each follow was created (ISO8601)",
            },
        );
        m.insert(
            "is_followed_by",
            PredicateInfo {
                arity: 2,
                args: &["follower_did", "self_did"],
                description: "Accounts that follow you (no rkey - from API)",
            },
        );

        // =================================================================
        // Bluesky: likes
        // =================================================================
        m.insert(
            "liked",
            PredicateInfo {
                arity: 3,
                args: &["self_did", "post_uri", "rkey"],
                description: "Posts you have liked",
            },
        );
        m.insert(
            "like_created_at",
            PredicateInfo {
                arity: 4,
                args: &["self_did", "post_uri", "timestamp", "rkey"],
                description: "When each like was created (ISO8601)",
            },
        );
        m.insert(
            "like_cid",
            PredicateInfo {
                arity: 4,
                args: &["self_did", "post_uri", "cid", "rkey"],
                description: "CID of the liked post",
            },
        );

        // =================================================================
        // Bluesky: reposts
        // =================================================================
        m.insert(
            "reposted",
            PredicateInfo {
                arity: 3,
                args: &["self_did", "post_uri", "rkey"],
                description: "Posts you have reposted",
            },
        );
        m.insert(
            "repost_created_at",
            PredicateInfo {
                arity: 4,
                args: &["self_did", "post_uri", "timestamp", "rkey"],
                description: "When each repost was created (ISO8601)",
            },
        );
        m.insert(
            "repost_cid",
            PredicateInfo {
                arity: 4,
                args: &["self_did", "post_uri", "cid", "rkey"],
                description: "CID of the reposted post",
            },
        );

        // =================================================================
        // Bluesky: posts
        // =================================================================
        m.insert(
            "posted",
            PredicateInfo {
                arity: 3,
                args: &["self_did", "post_uri", "rkey"],
                description: "Posts you have created",
            },
        );
        m.insert(
            "post_created_at",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "timestamp", "rkey"],
                description: "When each post was created (ISO8601)",
            },
        );
        m.insert(
            "replied_to",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "parent_uri", "rkey"],
                description: "Reply relationships between posts (alias: reply_parent_uri)",
            },
        );
        m.insert(
            "reply_parent_uri",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "parent_uri", "rkey"],
                description: "URI of the reply parent (alias: replied_to)",
            },
        );
        m.insert(
            "reply_parent_cid",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "parent_cid", "rkey"],
                description: "CID of the reply parent",
            },
        );
        m.insert(
            "thread_root",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "root_uri", "rkey"],
                description: "Thread membership (alias: reply_root_uri)",
            },
        );
        m.insert(
            "reply_root_uri",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "root_uri", "rkey"],
                description: "URI of the thread root (alias: thread_root)",
            },
        );
        m.insert(
            "reply_root_cid",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "root_cid", "rkey"],
                description: "CID of the thread root",
            },
        );
        m.insert(
            "quoted",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "quoted_uri", "rkey"],
                description: "Quote post relationships",
            },
        );
        m.insert(
            "quote_cid",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "quoted_cid", "rkey"],
                description: "CID of the quoted post",
            },
        );
        m.insert(
            "post_lang",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "lang", "rkey"],
                description: "Language tag for post (one row per language)",
            },
        );
        m.insert(
            "post_mention",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "did", "rkey"],
                description: "Accounts mentioned in post (one row per mention)",
            },
        );
        m.insert(
            "post_link",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "link_uri", "rkey"],
                description: "External links in post (one row per link)",
            },
        );
        m.insert(
            "post_hashtag",
            PredicateInfo {
                arity: 3,
                args: &["post_uri", "tag", "rkey"],
                description: "Hashtags in post (one row per tag)",
            },
        );

        // =================================================================
        // Directive predicates
        // =================================================================
        m.insert(
            "has_value",
            PredicateInfo {
                arity: 2,
                args: &["content", "rkey"],
                description: "Your active values",
            },
        );
        m.insert(
            "has_interest",
            PredicateInfo {
                arity: 2,
                args: &["content", "rkey"],
                description: "Your active interests",
            },
        );
        m.insert(
            "has_belief",
            PredicateInfo {
                arity: 2,
                args: &["content", "rkey"],
                description: "Your active beliefs",
            },
        );
        m.insert(
            "has_guideline",
            PredicateInfo {
                arity: 2,
                args: &["content", "rkey"],
                description: "Your active guidelines",
            },
        );
        m.insert(
            "has_boundary",
            PredicateInfo {
                arity: 2,
                args: &["content", "rkey"],
                description: "Your active boundaries",
            },
        );
        m.insert(
            "has_aspiration",
            PredicateInfo {
                arity: 2,
                args: &["content", "rkey"],
                description: "Your active aspirations",
            },
        );
        m.insert(
            "has_self_concept",
            PredicateInfo {
                arity: 2,
                args: &["content", "rkey"],
                description: "Your active self-concepts",
            },
        );

        // Tool and job predicates
        m.insert(
            "has_tool",
            PredicateInfo {
                arity: 3,
                args: &["name", "approved", "rkey"],
                description: "Your custom tools (approved: true/false)",
            },
        );
        m.insert(
            "has_job",
            PredicateInfo {
                arity: 3,
                args: &["name", "schedule_type", "rkey"],
                description: "Your scheduled jobs (once/interval)",
            },
        );
        m.insert(
            "has_trigger",
            PredicateInfo {
                arity: 3,
                args: &["name", "enabled", "rkey"],
                description: "Your datalog triggers (enabled: true/false)",
            },
        );

        // Note predicates
        m.insert(
            "has_note",
            PredicateInfo {
                arity: 6,
                args: &[
                    "uri",
                    "title",
                    "category",
                    "created_at",
                    "last_updated",
                    "rkey",
                ],
                description: "Your notes",
            },
        );
        m.insert(
            "note_tag",
            PredicateInfo {
                arity: 3,
                args: &["note_uri", "tag", "rkey"],
                description: "Tags on notes (one row per tag)",
            },
        );
        m.insert(
            "note_related_fact",
            PredicateInfo {
                arity: 3,
                args: &["note_uri", "fact_uri", "rkey"],
                description: "Facts linked to notes",
            },
        );

        // Thought predicates
        m.insert(
            "has_thought",
            PredicateInfo {
                arity: 5,
                args: &["uri", "kind", "trigger", "created_at", "rkey"],
                description: "Your stream of consciousness",
            },
        );
        m.insert(
            "thought_tag",
            PredicateInfo {
                arity: 3,
                args: &["thought_uri", "tag", "rkey"],
                description: "Tags on thoughts (one row per tag)",
            },
        );
        m.insert(
            "tool_call_duration",
            PredicateInfo {
                arity: 4,
                args: &["uri", "tool_name", "duration_ms", "rkey"],
                description: "Duration of tool calls in milliseconds",
            },
        );

        // Blog predicates
        m.insert(
            "has_blog_post",
            PredicateInfo {
                arity: 6,
                args: &[
                    "uri",
                    "title",
                    "whtwnd_url",
                    "created_at",
                    "is_draft",
                    "rkey",
                ],
                description: "Your WhiteWind blog posts",
            },
        );

        // Wiki entry predicates
        m.insert(
            "has_wiki_entry",
            PredicateInfo {
                arity: 7,
                args: &["uri", "title", "slug", "status", "created_at", "last_updated", "rkey"],
                description: "Your wiki entries",
            },
        );
        m.insert(
            "wiki_entry_alias",
            PredicateInfo {
                arity: 3,
                args: &["entry_uri", "alias", "rkey"],
                description: "Alternative names for wiki entries (one row per alias)",
            },
        );
        m.insert(
            "wiki_entry_tag",
            PredicateInfo {
                arity: 3,
                args: &["entry_uri", "tag", "rkey"],
                description: "Tags on wiki entries (one row per tag)",
            },
        );
        m.insert(
            "wiki_entry_supersedes",
            PredicateInfo {
                arity: 3,
                args: &["new_uri", "old_uri", "rkey"],
                description: "Wiki entry version chain",
            },
        );
        m.insert(
            "has_wiki_link",
            PredicateInfo {
                arity: 5,
                args: &["source_uri", "target_uri", "link_type", "created_at", "rkey"],
                description: "Semantic links between records",
            },
        );

        // Fact tags
        m.insert(
            "fact_tag",
            PredicateInfo {
                arity: 3,
                args: &["fact_uri", "tag", "rkey"],
                description: "Tags on facts (one row per tag)",
            },
        );

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

            // Wiki entries
            CacheUpdate::WikiEntryCreated { rkey, entry } => {
                self.add_wiki_entry(rkey.clone(), entry);
            }
            CacheUpdate::WikiEntryUpdated { rkey, entry } => {
                self.add_wiki_entry(rkey.clone(), entry);
            }
            CacheUpdate::WikiEntryDeleted { rkey } => {
                self.remove_wiki_entry(rkey);
            }

            // Wiki links
            CacheUpdate::WikiLinkCreated { rkey, link } => {
                self.add_wiki_link(rkey.clone(), link);
            }
            CacheUpdate::WikiLinkDeleted { rkey } => {
                self.remove_wiki_link(rkey);
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

            // Triggers
            CacheUpdate::TriggerCreated { rkey, trigger } => {
                self.add_trigger(rkey.clone(), trigger);
            }
            CacheUpdate::TriggerUpdated { rkey, trigger } => {
                self.add_trigger(rkey.clone(), trigger);
            }
            CacheUpdate::TriggerDeleted { rkey } => {
                self.remove_trigger(rkey);
            }

            // Ignored events
            _ => {}
        }
    }

    // =========================================================================
    // Follow handling
    // =========================================================================

    fn add_follow(&mut self, rkey: String, follow: &Follow) {
        self.follows.insert(
            rkey,
            FollowMeta {
                target_did: follow.subject.clone(),
                created_at: follow.created_at,
            },
        );
        self.dirty_predicates.insert("follows".to_string());
        self.dirty_predicates
            .insert("follow_created_at".to_string());
    }

    fn remove_follow(&mut self, rkey: &str) {
        if self.follows.remove(rkey).is_some() {
            self.dirty_predicates.insert("follows".to_string());
            self.dirty_predicates
                .insert("follow_created_at".to_string());
        }
    }

    // =========================================================================
    // Like handling
    // =========================================================================

    fn add_like(&mut self, rkey: String, like: &Like) {
        self.likes.insert(
            rkey,
            LikeMeta {
                post_uri: like.subject.uri.clone(),
                post_cid: like.subject.cid.clone(),
                created_at: like.created_at,
            },
        );
        self.dirty_predicates.insert("liked".to_string());
        self.dirty_predicates.insert("like_created_at".to_string());
        self.dirty_predicates.insert("like_cid".to_string());
    }

    fn remove_like(&mut self, rkey: &str) {
        if self.likes.remove(rkey).is_some() {
            self.dirty_predicates.insert("liked".to_string());
            self.dirty_predicates.insert("like_created_at".to_string());
            self.dirty_predicates.insert("like_cid".to_string());
        }
    }

    // =========================================================================
    // Repost handling
    // =========================================================================

    fn add_repost(&mut self, rkey: String, repost: &Repost) {
        self.reposts.insert(
            rkey,
            RepostMeta {
                post_uri: repost.subject.uri.clone(),
                post_cid: repost.subject.cid.clone(),
                created_at: repost.created_at,
            },
        );
        self.dirty_predicates.insert("reposted".to_string());
        self.dirty_predicates
            .insert("repost_created_at".to_string());
        self.dirty_predicates.insert("repost_cid".to_string());
    }

    fn remove_repost(&mut self, rkey: &str) {
        if self.reposts.remove(rkey).is_some() {
            self.dirty_predicates.insert("reposted".to_string());
            self.dirty_predicates
                .insert("repost_created_at".to_string());
            self.dirty_predicates.insert("repost_cid".to_string());
        }
    }

    // =========================================================================
    // Post handling
    // =========================================================================

    fn add_post(&mut self, rkey: String, post: &Post) {
        use winter_atproto::FacetFeature;

        let uri = format!("at://{}/app.bsky.feed.post/{}", self.self_did, rkey);

        // Extract reply info
        let reply_parent = post.reply.as_ref().map(|r| r.parent.uri.clone());
        let reply_parent_cid = post.reply.as_ref().map(|r| r.parent.cid.clone());
        let reply_root = post.reply.as_ref().map(|r| r.root.uri.clone());
        let reply_root_cid = post.reply.as_ref().map(|r| r.root.cid.clone());

        // Extract quote embed info (URI and CID)
        let (quote_uri, quote_cid) = post.embed.as_ref().map_or((None, None), |e| match e {
            winter_atproto::PostEmbed::Record { record } => {
                (Some(record.uri.clone()), Some(record.cid.clone()))
            }
            winter_atproto::PostEmbed::RecordWithMedia { record, .. } => (
                Some(record.record.uri.clone()),
                Some(record.record.cid.clone()),
            ),
            _ => (None, None),
        });

        // Extract facets: mentions, links, hashtags
        let mut mentions = Vec::new();
        let mut links = Vec::new();
        let mut hashtags = Vec::new();
        for facet in &post.facets {
            for feature in &facet.features {
                match feature {
                    FacetFeature::Mention { did } => mentions.push(did.clone()),
                    FacetFeature::Link { uri } => links.push(uri.clone()),
                    FacetFeature::Tag { tag } => hashtags.push(tag.clone()),
                }
            }
        }

        // Check what the previous post had (for dirty tracking)
        let prev = self.posts.get(&rkey);
        let had_reply = prev.map(|p| p.reply_parent.is_some()).unwrap_or(false);
        let had_root = prev.map(|p| p.reply_root.is_some()).unwrap_or(false);
        let had_quote = prev.map(|p| p.quote_uri.is_some()).unwrap_or(false);
        let had_langs = prev.map(|p| !p.langs.is_empty()).unwrap_or(false);
        let had_mentions = prev.map(|p| !p.mentions.is_empty()).unwrap_or(false);
        let had_links = prev.map(|p| !p.links.is_empty()).unwrap_or(false);
        let had_hashtags = prev.map(|p| !p.hashtags.is_empty()).unwrap_or(false);

        // Track whether current post has these fields (before move)
        let has_mentions = !mentions.is_empty();
        let has_links = !links.is_empty();
        let has_hashtags = !hashtags.is_empty();

        self.posts.insert(
            rkey,
            PostMeta {
                uri,
                reply_parent: reply_parent.clone(),
                reply_parent_cid: reply_parent_cid.clone(),
                reply_root: reply_root.clone(),
                reply_root_cid: reply_root_cid.clone(),
                quote_uri: quote_uri.clone(),
                quote_cid: quote_cid.clone(),
                created_at: post.created_at,
                langs: post.langs.clone(),
                mentions,
                links,
                hashtags,
            },
        );

        // Mark base predicates dirty
        self.dirty_predicates.insert("posted".to_string());
        self.dirty_predicates.insert("post_created_at".to_string());

        // Mark conditional predicates dirty
        if reply_parent.is_some() || had_reply {
            self.dirty_predicates.insert("replied_to".to_string());
            self.dirty_predicates.insert("reply_parent_uri".to_string());
            self.dirty_predicates.insert("reply_parent_cid".to_string());
        }
        if reply_root.is_some() || had_root {
            self.dirty_predicates.insert("thread_root".to_string());
            self.dirty_predicates.insert("reply_root_uri".to_string());
            self.dirty_predicates.insert("reply_root_cid".to_string());
        }
        if quote_uri.is_some() || had_quote {
            self.dirty_predicates.insert("quoted".to_string());
            self.dirty_predicates.insert("quote_cid".to_string());
        }
        if !post.langs.is_empty() || had_langs {
            self.dirty_predicates.insert("post_lang".to_string());
        }
        if has_mentions || had_mentions {
            self.dirty_predicates.insert("post_mention".to_string());
        }
        if has_links || had_links {
            self.dirty_predicates.insert("post_link".to_string());
        }
        if has_hashtags || had_hashtags {
            self.dirty_predicates.insert("post_hashtag".to_string());
        }
    }

    fn remove_post(&mut self, rkey: &str) {
        if let Some(post) = self.posts.remove(rkey) {
            self.dirty_predicates.insert("posted".to_string());
            self.dirty_predicates.insert("post_created_at".to_string());
            if post.reply_parent.is_some() {
                self.dirty_predicates.insert("replied_to".to_string());
                self.dirty_predicates.insert("reply_parent_uri".to_string());
                self.dirty_predicates.insert("reply_parent_cid".to_string());
            }
            if post.reply_root.is_some() {
                self.dirty_predicates.insert("thread_root".to_string());
                self.dirty_predicates.insert("reply_root_uri".to_string());
                self.dirty_predicates.insert("reply_root_cid".to_string());
            }
            if post.quote_uri.is_some() {
                self.dirty_predicates.insert("quoted".to_string());
                self.dirty_predicates.insert("quote_cid".to_string());
            }
            if !post.langs.is_empty() {
                self.dirty_predicates.insert("post_lang".to_string());
            }
            if !post.mentions.is_empty() {
                self.dirty_predicates.insert("post_mention".to_string());
            }
            if !post.links.is_empty() {
                self.dirty_predicates.insert("post_link".to_string());
            }
            if !post.hashtags.is_empty() {
                self.dirty_predicates.insert("post_hashtag".to_string());
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

        // Parse tool name from content for tool_call thoughts
        // Content format: "Called {tool_name}\n..." or "Called {tool_name} - FAILED\n..."
        let tool_name = if matches!(thought.kind, winter_atproto::ThoughtKind::ToolCall) {
            thought
                .content
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("Called "))
                .map(|s| s.trim_end_matches(" - FAILED").to_string())
        } else {
            None
        };

        // Check if tags changed for dirty tracking
        let had_tags = self
            .thoughts
            .get(&rkey)
            .map(|m| !m.tags.is_empty())
            .unwrap_or(false);

        self.thoughts.insert(
            rkey,
            ThoughtMeta {
                uri,
                kind: kind.to_string(),
                trigger: thought.trigger.clone(),
                tags: thought.tags.clone(),
                duration_ms: thought.duration_ms,
                tool_name,
                created_at: thought.created_at,
            },
        );

        self.dirty_predicates.insert("has_thought".to_string());
        self.dirty_predicates
            .insert("tool_call_duration".to_string());
        if !thought.tags.is_empty() || had_tags {
            self.dirty_predicates.insert("thought_tag".to_string());
        }
    }

    fn remove_thought(&mut self, rkey: &str) {
        if let Some(meta) = self.thoughts.remove(rkey) {
            self.dirty_predicates.insert("has_thought".to_string());
            self.dirty_predicates
                .insert("tool_call_duration".to_string());
            if !meta.tags.is_empty() {
                self.dirty_predicates.insert("thought_tag".to_string());
            }
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
    // Wiki entry handling
    // =========================================================================

    fn add_wiki_entry(&mut self, rkey: String, entry: &WikiEntry) {
        let uri = format!(
            "at://{}/diy.razorgirl.winter.wikiEntry/{}",
            self.self_did, rkey
        );

        let had_aliases = self
            .wiki_entries
            .get(&rkey)
            .map(|m| !m.aliases.is_empty())
            .unwrap_or(false);
        let had_tags = self
            .wiki_entries
            .get(&rkey)
            .map(|m| !m.tags.is_empty())
            .unwrap_or(false);
        let had_supersedes = self
            .wiki_entries
            .get(&rkey)
            .map(|m| m.supersedes.is_some())
            .unwrap_or(false);

        self.wiki_entries.insert(
            rkey,
            WikiEntryMeta {
                uri,
                title: entry.title.clone(),
                slug: entry.slug.clone(),
                status: entry.status.clone(),
                aliases: entry.aliases.clone(),
                tags: entry.tags.clone(),
                supersedes: entry.supersedes.clone(),
                created_at: entry.created_at,
                last_updated: entry.last_updated,
            },
        );

        self.dirty_predicates.insert("has_wiki_entry".to_string());
        if !entry.aliases.is_empty() || had_aliases {
            self.dirty_predicates
                .insert("wiki_entry_alias".to_string());
        }
        if !entry.tags.is_empty() || had_tags {
            self.dirty_predicates.insert("wiki_entry_tag".to_string());
        }
        if entry.supersedes.is_some() || had_supersedes {
            self.dirty_predicates
                .insert("wiki_entry_supersedes".to_string());
        }
    }

    fn remove_wiki_entry(&mut self, rkey: &str) {
        if let Some(meta) = self.wiki_entries.remove(rkey) {
            self.dirty_predicates.insert("has_wiki_entry".to_string());
            if !meta.aliases.is_empty() {
                self.dirty_predicates
                    .insert("wiki_entry_alias".to_string());
            }
            if !meta.tags.is_empty() {
                self.dirty_predicates.insert("wiki_entry_tag".to_string());
            }
            if meta.supersedes.is_some() {
                self.dirty_predicates
                    .insert("wiki_entry_supersedes".to_string());
            }
        }
    }

    // =========================================================================
    // Wiki link handling
    // =========================================================================

    fn add_wiki_link(&mut self, rkey: String, link: &WikiLink) {
        self.wiki_links.insert(
            rkey,
            WikiLinkMeta {
                source: link.source.clone(),
                target: link.target.clone(),
                link_type: link.link_type.clone(),
                created_at: link.created_at,
            },
        );
        self.dirty_predicates.insert("has_wiki_link".to_string());
    }

    fn remove_wiki_link(&mut self, rkey: &str) {
        if self.wiki_links.remove(rkey).is_some() {
            self.dirty_predicates.insert("has_wiki_link".to_string());
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
    // Trigger handling
    // =========================================================================

    fn add_trigger(&mut self, rkey: String, trigger: &Trigger) {
        self.triggers
            .insert(rkey, (trigger.name.clone(), trigger.enabled));
        self.dirty_predicates.insert("has_trigger".to_string());
    }

    fn remove_trigger(&mut self, rkey: &str) {
        if self.triggers.remove(rkey).is_some() {
            self.dirty_predicates.insert("has_trigger".to_string());
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

    fn write_predicate_file(
        &self,
        fact_dir: &Path,
        predicate: &str,
    ) -> Result<usize, DatalogError> {
        let path = fact_dir.join(format!("{}.facts", predicate));
        let file = std::fs::File::create(&path)?;
        let mut file = BufWriter::new(file);

        // Get count based on predicate (for logging)
        let count = match predicate {
            "follows" | "follow_created_at" => self.follows.len(),
            "is_followed_by" => self.followers.len(),
            "liked" | "like_created_at" | "like_cid" => self.likes.len(),
            "reposted" | "repost_created_at" | "repost_cid" => self.reposts.len(),
            "posted" | "post_created_at" | "replied_to" | "reply_parent_uri"
            | "reply_parent_cid" | "thread_root" | "reply_root_uri" | "reply_root_cid"
            | "quoted" | "quote_cid" | "post_lang" | "post_mention" | "post_link"
            | "post_hashtag" => self.posts.len(),
            "has_note" | "note_tag" | "note_related_fact" => self.notes.len(),
            "has_thought" | "thought_tag" | "tool_call_duration" => self.thoughts.len(),
            "has_blog_post" => self.blog_entries.len(),
            "has_wiki_entry" | "wiki_entry_alias" | "wiki_entry_tag" | "wiki_entry_supersedes" => self.wiki_entries.len(),
            "has_wiki_link" => self.wiki_links.len(),
            "has_tool" => self.tools.len(),
            "has_job" => self.jobs.len(),
            "has_trigger" => self.triggers.len(),
            _ => 0,
        };

        match predicate {
            // =================================================================
            // Follows
            // =================================================================
            "follows" => {
                for (rkey, meta) in &self.follows {
                    writeln!(file, "{}\t{}\t{}", self.self_did, meta.target_did, rkey)?;
                }
            }
            "follow_created_at" => {
                for (rkey, meta) in &self.follows {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did,
                        meta.target_did,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "is_followed_by" => {
                // No rkey - this comes from external API data
                for follower in &self.followers {
                    writeln!(file, "{}\t{}", follower, self.self_did)?;
                }
            }

            // =================================================================
            // Likes
            // =================================================================
            "liked" => {
                for (rkey, meta) in &self.likes {
                    writeln!(file, "{}\t{}\t{}", self.self_did, meta.post_uri, rkey)?;
                }
            }
            "like_created_at" => {
                for (rkey, meta) in &self.likes {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did,
                        meta.post_uri,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "like_cid" => {
                for (rkey, meta) in &self.likes {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did, meta.post_uri, meta.post_cid, rkey
                    )?;
                }
            }

            // =================================================================
            // Reposts
            // =================================================================
            "reposted" => {
                for (rkey, meta) in &self.reposts {
                    writeln!(file, "{}\t{}\t{}", self.self_did, meta.post_uri, rkey)?;
                }
            }
            "repost_created_at" => {
                for (rkey, meta) in &self.reposts {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did,
                        meta.post_uri,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "repost_cid" => {
                for (rkey, meta) in &self.reposts {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did, meta.post_uri, meta.post_cid, rkey
                    )?;
                }
            }

            // =================================================================
            // Posts
            // =================================================================
            "posted" => {
                for (rkey, post) in &self.posts {
                    writeln!(file, "{}\t{}\t{}", self.self_did, post.uri, rkey)?;
                }
            }
            "post_created_at" => {
                for (rkey, post) in &self.posts {
                    writeln!(
                        file,
                        "{}\t{}\t{}",
                        post.uri,
                        post.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "replied_to" | "reply_parent_uri" => {
                for (rkey, post) in &self.posts {
                    if let Some(ref parent) = post.reply_parent {
                        writeln!(file, "{}\t{}\t{}", post.uri, parent, rkey)?;
                    }
                }
            }
            "reply_parent_cid" => {
                for (rkey, post) in &self.posts {
                    if let Some(ref cid) = post.reply_parent_cid {
                        writeln!(file, "{}\t{}\t{}", post.uri, cid, rkey)?;
                    }
                }
            }
            "thread_root" | "reply_root_uri" => {
                for (rkey, post) in &self.posts {
                    if let Some(ref root) = post.reply_root {
                        writeln!(file, "{}\t{}\t{}", post.uri, root, rkey)?;
                    }
                }
            }
            "reply_root_cid" => {
                for (rkey, post) in &self.posts {
                    if let Some(ref cid) = post.reply_root_cid {
                        writeln!(file, "{}\t{}\t{}", post.uri, cid, rkey)?;
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
            "quote_cid" => {
                for (rkey, post) in &self.posts {
                    if let Some(ref cid) = post.quote_cid {
                        writeln!(file, "{}\t{}\t{}", post.uri, cid, rkey)?;
                    }
                }
            }
            "post_lang" => {
                for (rkey, post) in &self.posts {
                    for lang in &post.langs {
                        writeln!(file, "{}\t{}\t{}", post.uri, lang, rkey)?;
                    }
                }
            }
            "post_mention" => {
                for (rkey, post) in &self.posts {
                    for did in &post.mentions {
                        writeln!(file, "{}\t{}\t{}", post.uri, did, rkey)?;
                    }
                }
            }
            "post_link" => {
                for (rkey, post) in &self.posts {
                    for link in &post.links {
                        writeln!(file, "{}\t{}\t{}", post.uri, link, rkey)?;
                    }
                }
            }
            "post_hashtag" => {
                for (rkey, post) in &self.posts {
                    for tag in &post.hashtags {
                        writeln!(file, "{}\t{}\t{}", post.uri, tag, rkey)?;
                    }
                }
            }

            // =================================================================
            // Directives
            // =================================================================
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

            // =================================================================
            // Tools and Jobs
            // =================================================================
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
            "has_trigger" => {
                for (rkey, (name, enabled)) in &self.triggers {
                    writeln!(file, "{}\t{}\t{}", name, enabled, rkey)?;
                }
            }

            // =================================================================
            // Notes
            // =================================================================
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

            // =================================================================
            // Thoughts
            // =================================================================
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
            "thought_tag" => {
                for (rkey, meta) in &self.thoughts {
                    for tag in &meta.tags {
                        writeln!(file, "{}\t{}\t{}", meta.uri, tag, rkey)?;
                    }
                }
            }
            "tool_call_duration" => {
                for (rkey, meta) in &self.thoughts {
                    // Only tool_call thoughts with duration and tool name
                    if meta.kind == "tool_call"
                        && let (Some(duration), Some(tool_name)) =
                            (meta.duration_ms, meta.tool_name.as_ref())
                    {
                        writeln!(file, "{}\t{}\t{}\t{}", meta.uri, tool_name, duration, rkey)?;
                    }
                }
            }

            // =================================================================
            // Blog Posts
            // =================================================================
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

            // =================================================================
            // Wiki entries
            // =================================================================
            "has_wiki_entry" => {
                for (rkey, meta) in &self.wiki_entries {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        meta.uri,
                        escape_tsv(&meta.title),
                        meta.slug,
                        meta.status,
                        meta.created_at.to_rfc3339(),
                        meta.last_updated.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "wiki_entry_alias" => {
                for (rkey, meta) in &self.wiki_entries {
                    for alias in &meta.aliases {
                        writeln!(file, "{}\t{}\t{}", meta.uri, escape_tsv(alias), rkey)?;
                    }
                }
            }
            "wiki_entry_tag" => {
                for (rkey, meta) in &self.wiki_entries {
                    for tag in &meta.tags {
                        writeln!(file, "{}\t{}\t{}", meta.uri, tag, rkey)?;
                    }
                }
            }
            "wiki_entry_supersedes" => {
                for (rkey, meta) in &self.wiki_entries {
                    if let Some(ref old_uri) = meta.supersedes {
                        writeln!(file, "{}\t{}\t{}", meta.uri, old_uri, rkey)?;
                    }
                }
            }
            "has_wiki_link" => {
                for (rkey, meta) in &self.wiki_links {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}",
                        meta.source,
                        meta.target,
                        meta.link_type,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }

            // =================================================================
            // Fact Tags
            // =================================================================
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

    fn write_directive_predicate<W: Write>(
        &self,
        file: &mut W,
        kind: &DirectiveKind,
    ) -> Result<(), DatalogError> {
        for (rkey, (k, content)) in &self.directives {
            if k == kind {
                // Escape tabs and newlines in content
                let escaped = content.replace(['\t', '\n'], " ");
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
            wiki_entries: self.wiki_entries.len(),
            wiki_links: self.wiki_links.len(),
            triggers: self.triggers.len(),
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

    /// Get a snapshot of currently dirty predicates.
    pub fn dirty_predicates_snapshot(&self) -> HashSet<String> {
        self.dirty_predicates.clone()
    }

    /// Clear the dirty predicates set.
    pub fn clear_dirty(&mut self) {
        self.dirty_predicates.clear();
    }

    /// Create a snapshot of the derived facts for flushing.
    ///
    /// This clones all data needed for writing TSV files, allowing the
    /// caller to release locks before doing I/O.
    pub fn clone_for_flush(&self) -> DerivedFlushSnapshot {
        DerivedFlushSnapshot {
            self_did: self.self_did.clone(),
            handle: self.handle.clone(),
            follows: self.follows.clone(),
            likes: self.likes.clone(),
            reposts: self.reposts.clone(),
            posts: self.posts.clone(),
            directives: self.directives.clone(),
            tools: self.tools.clone(),
            jobs: self.jobs.clone(),
            notes: self.notes.clone(),
            thoughts: self.thoughts.clone(),
            blog_entries: self.blog_entries.clone(),
            wiki_entries: self.wiki_entries.clone(),
            wiki_links: self.wiki_links.clone(),
            fact_tags: self.fact_tags.clone(),
            triggers: self.triggers.clone(),
            followers: self.followers.clone(),
        }
    }
}

/// A snapshot of derived facts for writing to disk.
///
/// This is used to release locks before doing I/O, preventing
/// lock contention during file writes.
#[derive(Clone)]
pub struct DerivedFlushSnapshot {
    self_did: String,
    #[allow(dead_code)]
    handle: String,
    follows: HashMap<String, FollowMeta>,
    likes: HashMap<String, LikeMeta>,
    reposts: HashMap<String, RepostMeta>,
    posts: HashMap<String, PostMeta>,
    directives: HashMap<String, (DirectiveKind, String)>,
    tools: HashMap<String, (String, bool)>,
    jobs: HashMap<String, (String, String)>,
    notes: HashMap<String, NoteMeta>,
    thoughts: HashMap<String, ThoughtMeta>,
    blog_entries: HashMap<String, BlogMeta>,
    wiki_entries: HashMap<String, WikiEntryMeta>,
    wiki_links: HashMap<String, WikiLinkMeta>,
    fact_tags: HashMap<String, Vec<String>>,
    triggers: HashMap<String, (String, bool)>,
    followers: HashSet<String>,
}

impl DerivedFlushSnapshot {
    /// Write all derived predicate files.
    pub fn write_all_predicates(&self, fact_dir: &Path) -> Result<(), DatalogError> {
        for predicate in DerivedFactGenerator::arities().keys() {
            self.write_predicate_file(fact_dir, predicate)?;
        }
        Ok(())
    }

    /// Write only the specified dirty predicates.
    pub fn write_dirty_predicates(
        &self,
        fact_dir: &Path,
        dirty: &HashSet<String>,
    ) -> Result<(), DatalogError> {
        for predicate in dirty {
            self.write_predicate_file(fact_dir, predicate)?;
        }
        Ok(())
    }

    /// Write only the specified subset of predicates.
    ///
    /// Used for lazy regeneration - only writes predicates that are needed
    /// for the current query, creating empty files for predicates that exist
    /// but have no data.
    pub fn write_predicates_subset(
        &self,
        fact_dir: &Path,
        predicates: &HashSet<String>,
    ) -> Result<(), DatalogError> {
        for predicate in predicates {
            // Only write if this is a known derived predicate
            if DerivedFactGenerator::is_derived(predicate) {
                self.write_predicate_file(fact_dir, predicate)?;
            }
        }
        Ok(())
    }

    fn write_predicate_file(&self, fact_dir: &Path, predicate: &str) -> Result<(), DatalogError> {
        let path = fact_dir.join(format!("{}.facts", predicate));
        let file = std::fs::File::create(&path)?;
        let mut file = BufWriter::new(file);

        match predicate {
            // =================================================================
            // Follows
            // =================================================================
            "follows" => {
                for (rkey, meta) in &self.follows {
                    writeln!(file, "{}\t{}\t{}", self.self_did, meta.target_did, rkey)?;
                }
            }
            "follow_created_at" => {
                for (rkey, meta) in &self.follows {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did,
                        meta.target_did,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "is_followed_by" => {
                for follower in &self.followers {
                    writeln!(file, "{}\t{}", follower, self.self_did)?;
                }
            }

            // =================================================================
            // Likes
            // =================================================================
            "liked" => {
                for (rkey, meta) in &self.likes {
                    writeln!(file, "{}\t{}\t{}", self.self_did, meta.post_uri, rkey)?;
                }
            }
            "like_created_at" => {
                for (rkey, meta) in &self.likes {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did,
                        meta.post_uri,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "like_cid" => {
                for (rkey, meta) in &self.likes {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did, meta.post_uri, meta.post_cid, rkey
                    )?;
                }
            }

            // =================================================================
            // Reposts
            // =================================================================
            "reposted" => {
                for (rkey, meta) in &self.reposts {
                    writeln!(file, "{}\t{}\t{}", self.self_did, meta.post_uri, rkey)?;
                }
            }
            "repost_created_at" => {
                for (rkey, meta) in &self.reposts {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did,
                        meta.post_uri,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "repost_cid" => {
                for (rkey, meta) in &self.reposts {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}",
                        self.self_did, meta.post_uri, meta.post_cid, rkey
                    )?;
                }
            }

            // =================================================================
            // Posts
            // =================================================================
            "posted" => {
                for (rkey, meta) in &self.posts {
                    writeln!(file, "{}\t{}\t{}", self.self_did, meta.uri, rkey)?;
                }
            }
            "post_created_at" => {
                for (rkey, meta) in &self.posts {
                    writeln!(
                        file,
                        "{}\t{}\t{}",
                        meta.uri,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "replied_to" | "reply_parent_uri" => {
                for (rkey, meta) in &self.posts {
                    if let Some(ref parent) = meta.reply_parent {
                        writeln!(file, "{}\t{}\t{}", meta.uri, parent, rkey)?;
                    }
                }
            }
            "reply_parent_cid" => {
                for (rkey, meta) in &self.posts {
                    if let Some(ref cid) = meta.reply_parent_cid {
                        writeln!(file, "{}\t{}\t{}", meta.uri, cid, rkey)?;
                    }
                }
            }
            "thread_root" | "reply_root_uri" => {
                for (rkey, meta) in &self.posts {
                    if let Some(ref root) = meta.reply_root {
                        writeln!(file, "{}\t{}\t{}", meta.uri, root, rkey)?;
                    }
                }
            }
            "reply_root_cid" => {
                for (rkey, meta) in &self.posts {
                    if let Some(ref cid) = meta.reply_root_cid {
                        writeln!(file, "{}\t{}\t{}", meta.uri, cid, rkey)?;
                    }
                }
            }
            "quoted" => {
                for (rkey, meta) in &self.posts {
                    if let Some(ref quoted) = meta.quote_uri {
                        writeln!(file, "{}\t{}\t{}", meta.uri, quoted, rkey)?;
                    }
                }
            }
            "quote_cid" => {
                for (rkey, meta) in &self.posts {
                    if let Some(ref cid) = meta.quote_cid {
                        writeln!(file, "{}\t{}\t{}", meta.uri, cid, rkey)?;
                    }
                }
            }
            "post_lang" => {
                for (rkey, meta) in &self.posts {
                    for lang in &meta.langs {
                        writeln!(file, "{}\t{}\t{}", meta.uri, lang, rkey)?;
                    }
                }
            }
            "post_mention" => {
                for (rkey, meta) in &self.posts {
                    for did in &meta.mentions {
                        writeln!(file, "{}\t{}\t{}", meta.uri, did, rkey)?;
                    }
                }
            }
            "post_link" => {
                for (rkey, meta) in &self.posts {
                    for link in &meta.links {
                        writeln!(file, "{}\t{}\t{}", meta.uri, link, rkey)?;
                    }
                }
            }
            "post_hashtag" => {
                for (rkey, meta) in &self.posts {
                    for tag in &meta.hashtags {
                        writeln!(file, "{}\t{}\t{}", meta.uri, tag, rkey)?;
                    }
                }
            }

            // =================================================================
            // Directives
            // =================================================================
            "has_value" => {
                for (rkey, (kind, content)) in &self.directives {
                    if *kind == DirectiveKind::Value {
                        writeln!(file, "{}\t{}", escape_tsv(content), rkey)?;
                    }
                }
            }
            "has_interest" => {
                for (rkey, (kind, content)) in &self.directives {
                    if *kind == DirectiveKind::Interest {
                        writeln!(file, "{}\t{}", escape_tsv(content), rkey)?;
                    }
                }
            }
            "has_belief" => {
                for (rkey, (kind, content)) in &self.directives {
                    if *kind == DirectiveKind::Belief {
                        writeln!(file, "{}\t{}", escape_tsv(content), rkey)?;
                    }
                }
            }
            "has_guideline" => {
                for (rkey, (kind, content)) in &self.directives {
                    if *kind == DirectiveKind::Guideline {
                        writeln!(file, "{}\t{}", escape_tsv(content), rkey)?;
                    }
                }
            }
            "has_boundary" => {
                for (rkey, (kind, content)) in &self.directives {
                    if *kind == DirectiveKind::Boundary {
                        writeln!(file, "{}\t{}", escape_tsv(content), rkey)?;
                    }
                }
            }
            "has_aspiration" => {
                for (rkey, (kind, content)) in &self.directives {
                    if *kind == DirectiveKind::Aspiration {
                        writeln!(file, "{}\t{}", escape_tsv(content), rkey)?;
                    }
                }
            }
            "has_self_concept" => {
                for (rkey, (kind, content)) in &self.directives {
                    if *kind == DirectiveKind::SelfConcept {
                        writeln!(file, "{}\t{}", escape_tsv(content), rkey)?;
                    }
                }
            }

            // =================================================================
            // Tools and Jobs
            // =================================================================
            "has_tool" => {
                for (rkey, (name, approved)) in &self.tools {
                    writeln!(file, "{}\t{}\t{}", name, approved, rkey)?;
                }
            }
            "has_job" => {
                for (rkey, (name, schedule_type)) in &self.jobs {
                    writeln!(file, "{}\t{}\t{}", name, schedule_type, rkey)?;
                }
            }
            "has_trigger" => {
                for (rkey, (name, enabled)) in &self.triggers {
                    writeln!(file, "{}\t{}\t{}", name, enabled, rkey)?;
                }
            }

            // =================================================================
            // Notes
            // =================================================================
            "has_note" => {
                for (rkey, meta) in &self.notes {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        meta.uri,
                        escape_tsv(&meta.title),
                        meta.category.as_deref().unwrap_or(""),
                        meta.created_at.to_rfc3339(),
                        meta.last_updated.to_rfc3339(),
                        rkey
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

            // =================================================================
            // Thoughts
            // =================================================================
            "has_thought" => {
                for (rkey, meta) in &self.thoughts {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}",
                        meta.uri,
                        meta.kind,
                        meta.trigger.as_deref().unwrap_or(""),
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "thought_tag" => {
                for (rkey, meta) in &self.thoughts {
                    for tag in &meta.tags {
                        writeln!(file, "{}\t{}\t{}", meta.uri, tag, rkey)?;
                    }
                }
            }
            "tool_call_duration" => {
                for (rkey, meta) in &self.thoughts {
                    if let (Some(tool_name), Some(duration_ms)) =
                        (&meta.tool_name, meta.duration_ms)
                    {
                        writeln!(
                            file,
                            "{}\t{}\t{}\t{}",
                            meta.uri, tool_name, duration_ms, rkey
                        )?;
                    }
                }
            }

            // =================================================================
            // Blog entries
            // =================================================================
            "has_blog_post" => {
                for (rkey, meta) in &self.blog_entries {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        meta.uri,
                        escape_tsv(&meta.title),
                        meta.whtwnd_url,
                        meta.created_at,
                        meta.is_draft,
                        rkey
                    )?;
                }
            }

            // =================================================================
            // Wiki entries
            // =================================================================
            "has_wiki_entry" => {
                for (rkey, meta) in &self.wiki_entries {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        meta.uri,
                        escape_tsv(&meta.title),
                        meta.slug,
                        meta.status,
                        meta.created_at.to_rfc3339(),
                        meta.last_updated.to_rfc3339(),
                        rkey
                    )?;
                }
            }
            "wiki_entry_alias" => {
                for (rkey, meta) in &self.wiki_entries {
                    for alias in &meta.aliases {
                        writeln!(file, "{}\t{}\t{}", meta.uri, escape_tsv(alias), rkey)?;
                    }
                }
            }
            "wiki_entry_tag" => {
                for (rkey, meta) in &self.wiki_entries {
                    for tag in &meta.tags {
                        writeln!(file, "{}\t{}\t{}", meta.uri, tag, rkey)?;
                    }
                }
            }
            "wiki_entry_supersedes" => {
                for (rkey, meta) in &self.wiki_entries {
                    if let Some(ref old_uri) = meta.supersedes {
                        writeln!(file, "{}\t{}\t{}", meta.uri, old_uri, rkey)?;
                    }
                }
            }
            "has_wiki_link" => {
                for (rkey, meta) in &self.wiki_links {
                    writeln!(
                        file,
                        "{}\t{}\t{}\t{}\t{}",
                        meta.source,
                        meta.target,
                        meta.link_type,
                        meta.created_at.to_rfc3339(),
                        rkey
                    )?;
                }
            }

            // =================================================================
            // Fact tags
            // =================================================================
            "fact_tag" => {
                for (rkey, tags) in &self.fact_tags {
                    let uri = format!("at://{}/diy.razorgirl.winter.fact/{}", self.self_did, rkey);
                    for tag in tags {
                        writeln!(file, "{}\t{}\t{}", uri, tag, rkey)?;
                    }
                }
            }

            _ => {
                // Unknown predicate, create empty file
                trace!(predicate, "unknown derived predicate, creating empty file");
            }
        }

        Ok(())
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
    pub wiki_entries: usize,
    pub wiki_links: usize,
    pub triggers: usize,
}

/// Escape tabs and newlines in a string for TSV output.
fn escape_tsv(s: &str) -> String {
    s.replace(['\t', '\n'], " ")
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
        make_thought_with_tags(kind, trigger, vec![])
    }

    fn make_thought_with_tags(
        kind: ThoughtKind,
        trigger: Option<&str>,
        tags: Vec<&str>,
    ) -> Thought {
        Thought {
            kind,
            content: "test thought content".to_string(),
            trigger: trigger.map(String::from),
            tags: tags.into_iter().map(String::from).collect(),
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
            expires_at: None,
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
        assert_eq!(meta.uri, "at://did:plc:winter/com.whtwnd.blog.entry/blog1");
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
    fn test_thought_tag_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Thought with tags
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought1".to_string(),
            thought: make_thought_with_tags(
                ThoughtKind::Insight,
                Some("test trigger"),
                vec!["datalog", "performance", "optimization"],
            ),
        });

        // Thought without tags (should not produce rows)
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought2".to_string(),
            thought: make_thought(ThoughtKind::Question, None),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("thought_tag.facts")).unwrap();
        let uri = "at://did:plc:test/diy.razorgirl.winter.thought/thought1";

        // Each tag should produce a row: uri\ttag\trkey
        assert!(content.contains(&format!("{}\tdatalog\tthought1", uri)));
        assert!(content.contains(&format!("{}\tperformance\tthought1", uri)));
        assert!(content.contains(&format!("{}\toptimization\tthought1", uri)));

        // Count lines (should be 3, only from thought1 with tags)
        let line_count = content.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(line_count, 3);
    }

    #[test]
    fn test_thought_tag_dirty_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Create thought without tags - should NOT mark thought_tag dirty
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought1".to_string(),
            thought: make_thought(ThoughtKind::Insight, None),
        });
        dfg.flush_to_dir(dir.path()).unwrap();

        // Create thought with tags - should mark thought_tag dirty
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought2".to_string(),
            thought: make_thought_with_tags(ThoughtKind::Plan, None, vec!["tag1"]),
        });
        assert!(dfg.dirty_predicates.contains("thought_tag"));
        dfg.flush_to_dir(dir.path()).unwrap();

        // Delete thought with tags - should mark thought_tag dirty
        dfg.handle_update(&CacheUpdate::ThoughtDeleted {
            rkey: "thought2".to_string(),
        });
        assert!(dfg.dirty_predicates.contains("thought_tag"));
    }

    #[test]
    fn test_tool_call_duration_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Tool call thought with duration
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "toolcall1".to_string(),
            thought: Thought {
                kind: ThoughtKind::ToolCall,
                content: "Called query_facts\nArgs:\n{}".to_string(),
                trigger: Some("internal:tool_call".to_string()),
                tags: Vec::new(),
                duration_ms: Some(1234),
                created_at: Utc::now(),
            },
        });

        // Tool call without duration (should not appear in tool_call_duration)
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "toolcall2".to_string(),
            thought: Thought {
                kind: ThoughtKind::ToolCall,
                content: "Called list_rules\nArgs:\n{}".to_string(),
                trigger: Some("internal:tool_call".to_string()),
                tags: Vec::new(),
                duration_ms: None,
                created_at: Utc::now(),
            },
        });

        // Non-tool-call thought (should not appear)
        dfg.handle_update(&CacheUpdate::ThoughtCreated {
            rkey: "thought1".to_string(),
            thought: make_thought(ThoughtKind::Reflection, Some("test")),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("tool_call_duration.facts")).unwrap();

        // Only toolcall1 should appear (has duration)
        assert!(content.contains("at://did:plc:test/diy.razorgirl.winter.thought/toolcall1"));
        assert!(content.contains("query_facts"));
        assert!(content.contains("1234"));

        // toolcall2 should not appear (no duration)
        assert!(!content.contains("toolcall2"));

        // thought1 should not appear (not a tool_call)
        assert!(!content.contains("thought1"));

        let line_count = content.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(line_count, 1);
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
            note: make_note(
                "Second Note",
                Some("cat2"),
                vec!["shared", "unique"],
                vec![],
            ),
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

    // =========================================================================
    // New Predicate Tests
    // =========================================================================

    fn make_repost(uri: &str, cid: &str) -> Repost {
        Repost {
            subject: StrongRef {
                uri: uri.to_string(),
                cid: cid.to_string(),
            },
            created_at: Utc::now(),
        }
    }

    fn make_post_with_facets(
        text: &str,
        reply: Option<ReplyRef>,
        embed: Option<winter_atproto::PostEmbed>,
        facets: Vec<Facet>,
        langs: Vec<&str>,
    ) -> Post {
        Post {
            text: text.to_string(),
            reply,
            embed,
            facets,
            langs: langs.into_iter().map(String::from).collect(),
            created_at: Utc::now(),
        }
    }

    use winter_atproto::{ByteSlice, Facet, FacetFeature, ReplyRef};

    #[test]
    fn test_new_predicates_in_is_derived() {
        // Post predicates
        assert!(DerivedFactGenerator::is_derived("post_created_at"));
        assert!(DerivedFactGenerator::is_derived("reply_parent_uri"));
        assert!(DerivedFactGenerator::is_derived("reply_parent_cid"));
        assert!(DerivedFactGenerator::is_derived("reply_root_uri"));
        assert!(DerivedFactGenerator::is_derived("reply_root_cid"));
        assert!(DerivedFactGenerator::is_derived("quote_cid"));
        assert!(DerivedFactGenerator::is_derived("post_lang"));
        assert!(DerivedFactGenerator::is_derived("post_mention"));
        assert!(DerivedFactGenerator::is_derived("post_link"));
        assert!(DerivedFactGenerator::is_derived("post_hashtag"));
        // Like predicates
        assert!(DerivedFactGenerator::is_derived("like_created_at"));
        assert!(DerivedFactGenerator::is_derived("like_cid"));
        // Repost predicates
        assert!(DerivedFactGenerator::is_derived("repost_created_at"));
        assert!(DerivedFactGenerator::is_derived("repost_cid"));
        // Follow predicates
        assert!(DerivedFactGenerator::is_derived("follow_created_at"));
    }

    #[test]
    fn test_new_predicate_arities() {
        let arities = DerivedFactGenerator::arities();
        // Post predicates
        assert_eq!(arities.get("post_created_at"), Some(&3)); // (post_uri, timestamp, rkey)
        assert_eq!(arities.get("reply_parent_uri"), Some(&3)); // (post_uri, parent_uri, rkey)
        assert_eq!(arities.get("reply_parent_cid"), Some(&3)); // (post_uri, parent_cid, rkey)
        assert_eq!(arities.get("reply_root_uri"), Some(&3)); // (post_uri, root_uri, rkey)
        assert_eq!(arities.get("reply_root_cid"), Some(&3)); // (post_uri, root_cid, rkey)
        assert_eq!(arities.get("quote_cid"), Some(&3)); // (post_uri, quoted_cid, rkey)
        assert_eq!(arities.get("post_lang"), Some(&3)); // (post_uri, lang, rkey)
        assert_eq!(arities.get("post_mention"), Some(&3)); // (post_uri, did, rkey)
        assert_eq!(arities.get("post_link"), Some(&3)); // (post_uri, link_uri, rkey)
        assert_eq!(arities.get("post_hashtag"), Some(&3)); // (post_uri, tag, rkey)
        // Like predicates
        assert_eq!(arities.get("like_created_at"), Some(&4)); // (self_did, post_uri, timestamp, rkey)
        assert_eq!(arities.get("like_cid"), Some(&4)); // (self_did, post_uri, cid, rkey)
        // Repost predicates
        assert_eq!(arities.get("repost_created_at"), Some(&4)); // (self_did, post_uri, timestamp, rkey)
        assert_eq!(arities.get("repost_cid"), Some(&4)); // (self_did, post_uri, cid, rkey)
        // Follow predicates
        assert_eq!(arities.get("follow_created_at"), Some(&4)); // (self_did, target_did, timestamp, rkey)
    }

    #[test]
    fn test_follow_created_at_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        dfg.handle_update(&CacheUpdate::FollowCreated {
            rkey: "follow1".to_string(),
            follow: make_follow("did:plc:target"),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // Check follows file
        let follows = std::fs::read_to_string(dir.path().join("follows.facts")).unwrap();
        assert!(follows.contains("did:plc:test"));
        assert!(follows.contains("did:plc:target"));
        assert!(follows.contains("follow1"));

        // Check follow_created_at file
        let follow_created_at =
            std::fs::read_to_string(dir.path().join("follow_created_at.facts")).unwrap();
        assert!(follow_created_at.contains("did:plc:test"));
        assert!(follow_created_at.contains("did:plc:target"));
        assert!(follow_created_at.contains("follow1"));
        // Should have ISO8601 timestamp
        assert!(follow_created_at.contains("T"));
    }

    #[test]
    fn test_like_predicates_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        dfg.handle_update(&CacheUpdate::LikeCreated {
            rkey: "like1".to_string(),
            like: Like {
                subject: StrongRef {
                    uri: "at://did:plc:author/app.bsky.feed.post/abc".to_string(),
                    cid: "bafyreig123".to_string(),
                },
                created_at: Utc::now(),
            },
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // Check liked file
        let liked = std::fs::read_to_string(dir.path().join("liked.facts")).unwrap();
        assert!(liked.contains("did:plc:test"));
        assert!(liked.contains("at://did:plc:author/app.bsky.feed.post/abc"));

        // Check like_created_at file
        let like_created_at =
            std::fs::read_to_string(dir.path().join("like_created_at.facts")).unwrap();
        assert!(like_created_at.contains("did:plc:test"));
        assert!(like_created_at.contains("at://did:plc:author/app.bsky.feed.post/abc"));
        assert!(like_created_at.contains("T")); // ISO8601 timestamp

        // Check like_cid file
        let like_cid = std::fs::read_to_string(dir.path().join("like_cid.facts")).unwrap();
        assert!(like_cid.contains("did:plc:test"));
        assert!(like_cid.contains("bafyreig123"));
    }

    #[test]
    fn test_repost_predicates_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        dfg.handle_update(&CacheUpdate::RepostCreated {
            rkey: "repost1".to_string(),
            repost: make_repost("at://did:plc:author/app.bsky.feed.post/xyz", "bafyreig456"),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // Check reposted file
        let reposted = std::fs::read_to_string(dir.path().join("reposted.facts")).unwrap();
        assert!(reposted.contains("did:plc:test"));
        assert!(reposted.contains("at://did:plc:author/app.bsky.feed.post/xyz"));

        // Check repost_created_at file
        let repost_created_at =
            std::fs::read_to_string(dir.path().join("repost_created_at.facts")).unwrap();
        assert!(repost_created_at.contains("T")); // ISO8601 timestamp

        // Check repost_cid file
        let repost_cid = std::fs::read_to_string(dir.path().join("repost_cid.facts")).unwrap();
        assert!(repost_cid.contains("bafyreig456"));
    }

    #[test]
    fn test_post_created_at_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "post1".to_string(),
            post: make_post_with_facets("Hello world", None, None, vec![], vec!["en"]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // Check post_created_at file
        let post_created_at =
            std::fs::read_to_string(dir.path().join("post_created_at.facts")).unwrap();
        assert!(post_created_at.contains("at://did:plc:test/app.bsky.feed.post/post1"));
        assert!(post_created_at.contains("T")); // ISO8601 timestamp
    }

    #[test]
    fn test_post_lang_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "post1".to_string(),
            post: make_post_with_facets("Hello world", None, None, vec![], vec!["en", "ja"]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // Check post_lang file
        let post_lang = std::fs::read_to_string(dir.path().join("post_lang.facts")).unwrap();
        let post_uri = "at://did:plc:test/app.bsky.feed.post/post1";

        // Should have two rows (one per language)
        assert!(post_lang.contains(&format!("{}\ten", post_uri)));
        assert!(post_lang.contains(&format!("{}\tja", post_uri)));
        assert_eq!(post_lang.lines().filter(|l| !l.is_empty()).count(), 2);
    }

    #[test]
    fn test_post_mention_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        let facets = vec![Facet {
            index: ByteSlice {
                byte_start: 0,
                byte_end: 5,
            },
            features: vec![FacetFeature::Mention {
                did: "did:plc:alice".to_string(),
            }],
        }];

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "post1".to_string(),
            post: make_post_with_facets("@alice hello", None, None, facets, vec![]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // Check post_mention file
        let post_mention = std::fs::read_to_string(dir.path().join("post_mention.facts")).unwrap();
        let post_uri = "at://did:plc:test/app.bsky.feed.post/post1";

        assert!(post_mention.contains(&format!("{}\tdid:plc:alice", post_uri)));
    }

    #[test]
    fn test_post_link_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        let facets = vec![Facet {
            index: ByteSlice {
                byte_start: 0,
                byte_end: 20,
            },
            features: vec![FacetFeature::Link {
                uri: "https://example.com".to_string(),
            }],
        }];

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "post1".to_string(),
            post: make_post_with_facets(
                "Check out https://example.com",
                None,
                None,
                facets,
                vec![],
            ),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // Check post_link file
        let post_link = std::fs::read_to_string(dir.path().join("post_link.facts")).unwrap();
        let post_uri = "at://did:plc:test/app.bsky.feed.post/post1";

        assert!(post_link.contains(&format!("{}\thttps://example.com", post_uri)));
    }

    #[test]
    fn test_post_hashtag_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        let facets = vec![Facet {
            index: ByteSlice {
                byte_start: 0,
                byte_end: 8,
            },
            features: vec![FacetFeature::Tag {
                tag: "atproto".to_string(),
            }],
        }];

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "post1".to_string(),
            post: make_post_with_facets("#atproto is cool", None, None, facets, vec![]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        // Check post_hashtag file
        let post_hashtag = std::fs::read_to_string(dir.path().join("post_hashtag.facts")).unwrap();
        let post_uri = "at://did:plc:test/app.bsky.feed.post/post1";

        assert!(post_hashtag.contains(&format!("{}\tatproto", post_uri)));
    }

    #[test]
    fn test_reply_predicates_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        let reply = ReplyRef {
            parent: StrongRef {
                uri: "at://did:plc:other/app.bsky.feed.post/parent".to_string(),
                cid: "bafyparent".to_string(),
            },
            root: StrongRef {
                uri: "at://did:plc:other/app.bsky.feed.post/root".to_string(),
                cid: "bafyroot".to_string(),
            },
        };

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "reply1".to_string(),
            post: make_post_with_facets("This is a reply", Some(reply), None, vec![], vec![]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let post_uri = "at://did:plc:test/app.bsky.feed.post/reply1";
        let parent_uri = "at://did:plc:other/app.bsky.feed.post/parent";
        let root_uri = "at://did:plc:other/app.bsky.feed.post/root";

        // Check replied_to file (same as reply_parent_uri)
        let replied_to = std::fs::read_to_string(dir.path().join("replied_to.facts")).unwrap();
        assert!(replied_to.contains(&format!("{}\t{}", post_uri, parent_uri)));

        // Check reply_parent_uri file (alias for replied_to)
        let reply_parent_uri =
            std::fs::read_to_string(dir.path().join("reply_parent_uri.facts")).unwrap();
        assert!(reply_parent_uri.contains(&format!("{}\t{}", post_uri, parent_uri)));

        // Check reply_parent_cid file
        let reply_parent_cid =
            std::fs::read_to_string(dir.path().join("reply_parent_cid.facts")).unwrap();
        assert!(reply_parent_cid.contains(&format!("{}\tbafyparent", post_uri)));

        // Check thread_root file (same as reply_root_uri)
        let thread_root = std::fs::read_to_string(dir.path().join("thread_root.facts")).unwrap();
        assert!(thread_root.contains(&format!("{}\t{}", post_uri, root_uri)));

        // Check reply_root_uri file (alias for thread_root)
        let reply_root_uri =
            std::fs::read_to_string(dir.path().join("reply_root_uri.facts")).unwrap();
        assert!(reply_root_uri.contains(&format!("{}\t{}", post_uri, root_uri)));

        // Check reply_root_cid file
        let reply_root_cid =
            std::fs::read_to_string(dir.path().join("reply_root_cid.facts")).unwrap();
        assert!(reply_root_cid.contains(&format!("{}\tbafyroot", post_uri)));
    }

    #[test]
    fn test_quote_cid_tsv_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        let embed = winter_atproto::PostEmbed::Record {
            record: StrongRef {
                uri: "at://did:plc:other/app.bsky.feed.post/quoted".to_string(),
                cid: "bafyquoted".to_string(),
            },
        };

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "quote1".to_string(),
            post: make_post_with_facets("Check this out", None, Some(embed), vec![], vec![]),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let post_uri = "at://did:plc:test/app.bsky.feed.post/quote1";

        // Check quote_cid file
        let quote_cid = std::fs::read_to_string(dir.path().join("quote_cid.facts")).unwrap();
        assert!(quote_cid.contains(&format!("{}\tbafyquoted", post_uri)));

        // Check quoted file (URI)
        let quoted = std::fs::read_to_string(dir.path().join("quoted.facts")).unwrap();
        assert!(quoted.contains(&format!(
            "{}\tat://did:plc:other/app.bsky.feed.post/quoted",
            post_uri
        )));
    }

    #[test]
    fn test_combined_post_predicates() {
        let dir = tempfile::tempdir().unwrap();
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Create a post with multiple facets
        let facets = vec![
            Facet {
                index: ByteSlice {
                    byte_start: 0,
                    byte_end: 6,
                },
                features: vec![FacetFeature::Mention {
                    did: "did:plc:alice".to_string(),
                }],
            },
            Facet {
                index: ByteSlice {
                    byte_start: 10,
                    byte_end: 30,
                },
                features: vec![FacetFeature::Link {
                    uri: "https://example.com".to_string(),
                }],
            },
            Facet {
                index: ByteSlice {
                    byte_start: 35,
                    byte_end: 43,
                },
                features: vec![FacetFeature::Tag {
                    tag: "bluesky".to_string(),
                }],
            },
        ];

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "complex1".to_string(),
            post: make_post_with_facets(
                "@alice check https://example.com #bluesky",
                None,
                None,
                facets,
                vec!["en", "es"],
            ),
        });

        dfg.flush_to_dir(dir.path()).unwrap();

        let post_uri = "at://did:plc:test/app.bsky.feed.post/complex1";

        // Verify all predicates have content
        let post_mention = std::fs::read_to_string(dir.path().join("post_mention.facts")).unwrap();
        assert!(post_mention.contains(&format!("{}\tdid:plc:alice", post_uri)));

        let post_link = std::fs::read_to_string(dir.path().join("post_link.facts")).unwrap();
        assert!(post_link.contains(&format!("{}\thttps://example.com", post_uri)));

        let post_hashtag = std::fs::read_to_string(dir.path().join("post_hashtag.facts")).unwrap();
        assert!(post_hashtag.contains(&format!("{}\tbluesky", post_uri)));

        let post_lang = std::fs::read_to_string(dir.path().join("post_lang.facts")).unwrap();
        assert!(post_lang.contains(&format!("{}\ten", post_uri)));
        assert!(post_lang.contains(&format!("{}\tes", post_uri)));
    }

    #[test]
    fn test_dirty_tracking_for_new_predicates() {
        let mut dfg = DerivedFactGenerator::new("did:plc:test", "test.handle");

        // Create a follow
        dfg.handle_update(&CacheUpdate::FollowCreated {
            rkey: "follow1".to_string(),
            follow: make_follow("did:plc:target"),
        });

        assert!(dfg.dirty_predicates.contains("follows"));
        assert!(dfg.dirty_predicates.contains("follow_created_at"));

        dfg.dirty_predicates.clear();

        // Create a like
        dfg.handle_update(&CacheUpdate::LikeCreated {
            rkey: "like1".to_string(),
            like: make_like("at://did:plc:author/post/1"),
        });

        assert!(dfg.dirty_predicates.contains("liked"));
        assert!(dfg.dirty_predicates.contains("like_created_at"));
        assert!(dfg.dirty_predicates.contains("like_cid"));

        dfg.dirty_predicates.clear();

        // Create a repost
        dfg.handle_update(&CacheUpdate::RepostCreated {
            rkey: "repost1".to_string(),
            repost: make_repost("at://did:plc:author/post/1", "bafycid"),
        });

        assert!(dfg.dirty_predicates.contains("reposted"));
        assert!(dfg.dirty_predicates.contains("repost_created_at"));
        assert!(dfg.dirty_predicates.contains("repost_cid"));

        dfg.dirty_predicates.clear();

        // Create a post with facets
        let facets = vec![Facet {
            index: ByteSlice {
                byte_start: 0,
                byte_end: 5,
            },
            features: vec![
                FacetFeature::Mention {
                    did: "did:plc:alice".to_string(),
                },
                FacetFeature::Tag {
                    tag: "test".to_string(),
                },
            ],
        }];

        dfg.handle_update(&CacheUpdate::PostCreated {
            rkey: "post1".to_string(),
            post: make_post_with_facets("@alice #test", None, None, facets, vec!["en"]),
        });

        assert!(dfg.dirty_predicates.contains("posted"));
        assert!(dfg.dirty_predicates.contains("post_created_at"));
        assert!(dfg.dirty_predicates.contains("post_lang"));
        assert!(dfg.dirty_predicates.contains("post_mention"));
        assert!(dfg.dirty_predicates.contains("post_hashtag"));
    }
}
