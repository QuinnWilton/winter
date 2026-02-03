//! In-memory cache for ATProto repository records.
//!
//! Provides thread-safe caching of facts and rules with support for
//! real-time updates via firehose subscription.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use dashmap::DashMap;
use tokio::sync::{Mutex, RwLock, broadcast};
use tracing::{debug, trace};

use crate::{
    BlogEntry, CustomTool, DaemonState, Directive, Fact, FactDeclaration, Follow, Identity, Job,
    Like, Note, Post, Repost, Rule, Thought, ToolApproval,
};

/// Synchronization state of the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SyncState {
    /// Not yet started synchronization.
    Disconnected = 0,
    /// CAR fetch in progress, firehose events being queued.
    Syncing = 1,
    /// Fully synchronized with real-time updates.
    Live = 2,
}

impl From<u8> for SyncState {
    fn from(v: u8) -> Self {
        match v {
            0 => SyncState::Disconnected,
            1 => SyncState::Syncing,
            2 => SyncState::Live,
            _ => SyncState::Disconnected,
        }
    }
}

/// A cached record with its CID.
#[derive(Debug, Clone)]
pub struct CachedRecord<T> {
    pub value: T,
    pub cid: String,
}

/// Filter for scoping thought retrieval by conversation context.
///
/// Used to prevent cross-contamination when multiple workers process
/// notifications concurrently.
#[derive(Debug, Clone)]
pub enum ScopeFilter {
    /// Filter by thread (matches thoughts with root URI in trigger).
    Thread { root_uri: String },
    /// Filter by direct message conversation.
    DirectMessage { convo_id: String },
    /// Filter by job name.
    Job { name: String },
    /// Only include global thoughts (trigger: None).
    Global,
}

/// Check if a thought matches the given scope filter.
///
/// Global thoughts (trigger: None) always match non-Global scopes.
fn thought_matches_scope(thought: &Thought, scope: &ScopeFilter) -> bool {
    match &thought.trigger {
        None => true, // Global thoughts always match (included in all contexts)
        Some(trigger) => match scope {
            ScopeFilter::Thread { root_uri } => {
                // Match notification thoughts from same thread via root= suffix
                // Format: notification:{post_uri}:root={root_uri}
                trigger.starts_with("notification:")
                    && trigger.ends_with(&format!(":root={}", root_uri))
            }
            ScopeFilter::DirectMessage { convo_id } => {
                // Format: dm:{convo_id}:{message_id}
                trigger.starts_with(&format!("dm:{}:", convo_id))
            }
            ScopeFilter::Job { name } => {
                // Format: job:{name}
                trigger == &format!("job:{}", name)
            }
            ScopeFilter::Global => false, // Only global thoughts (handled above)
        },
    }
}

/// Update event for cache subscribers.
#[derive(Debug, Clone)]
pub enum CacheUpdate {
    /// A fact was created.
    FactCreated { rkey: String, fact: Fact },
    /// A fact was updated.
    FactUpdated { rkey: String, fact: Fact },
    /// A fact was deleted.
    FactDeleted { rkey: String },
    /// A rule was created.
    RuleCreated { rkey: String, rule: Rule },
    /// A rule was updated.
    RuleUpdated { rkey: String, rule: Rule },
    /// A rule was deleted.
    RuleDeleted { rkey: String },
    /// A thought was created.
    ThoughtCreated { rkey: String, thought: Thought },
    /// A thought was deleted.
    ThoughtDeleted { rkey: String },
    /// A note was created.
    NoteCreated { rkey: String, note: Note },
    /// A note was updated.
    NoteUpdated { rkey: String, note: Note },
    /// A note was deleted.
    NoteDeleted { rkey: String },
    /// A job was created.
    JobCreated { rkey: String, job: Job },
    /// A job was updated.
    JobUpdated { rkey: String, job: Job },
    /// A job was deleted.
    JobDeleted { rkey: String },
    /// Identity was updated.
    IdentityUpdated { identity: Identity },
    /// Cache is now fully synchronized.
    Synchronized,
    // =========================================================================
    // Bluesky record updates
    // =========================================================================
    /// A follow was created.
    FollowCreated { rkey: String, follow: Follow },
    /// A follow was deleted.
    FollowDeleted { rkey: String },
    /// A like was created.
    LikeCreated { rkey: String, like: Like },
    /// A like was deleted.
    LikeDeleted { rkey: String },
    /// A repost was created.
    RepostCreated { rkey: String, repost: Repost },
    /// A repost was deleted.
    RepostDeleted { rkey: String },
    /// A post was created.
    PostCreated { rkey: String, post: Post },
    /// A post was updated.
    PostUpdated { rkey: String, post: Post },
    /// A post was deleted.
    PostDeleted { rkey: String },
    // =========================================================================
    // Winter record updates (beyond facts/rules/thoughts/notes/jobs)
    // =========================================================================
    /// A directive was created.
    DirectiveCreated { rkey: String, directive: Directive },
    /// A directive was updated.
    DirectiveUpdated { rkey: String, directive: Directive },
    /// A directive was deleted.
    DirectiveDeleted { rkey: String },
    /// A custom tool was created.
    ToolCreated { rkey: String, tool: CustomTool },
    /// A custom tool was updated.
    ToolUpdated { rkey: String, tool: CustomTool },
    /// A custom tool was deleted.
    ToolDeleted { rkey: String },
    /// A tool approval was created.
    ToolApprovalCreated {
        rkey: String,
        approval: ToolApproval,
    },
    /// A tool approval was updated.
    ToolApprovalUpdated {
        rkey: String,
        approval: ToolApproval,
    },
    /// A tool approval was deleted.
    ToolApprovalDeleted { rkey: String },
    /// A blog entry was created.
    BlogEntryCreated { rkey: String, entry: BlogEntry },
    /// A blog entry was updated.
    BlogEntryUpdated { rkey: String, entry: BlogEntry },
    /// A blog entry was deleted.
    BlogEntryDeleted { rkey: String },
    /// A fact declaration was created.
    DeclarationCreated {
        rkey: String,
        declaration: FactDeclaration,
    },
    /// A fact declaration was updated.
    DeclarationUpdated {
        rkey: String,
        declaration: FactDeclaration,
    },
    /// A fact declaration was deleted.
    DeclarationDeleted { rkey: String },
    /// Daemon state was updated.
    StateUpdated { state: DaemonState },
}

/// A commit event from the firehose, queued during sync.
#[derive(Debug, Clone)]
pub struct FirehoseCommit {
    /// Repository revision.
    pub rev: String,
    /// Operations in this commit.
    pub ops: Vec<FirehoseOp>,
}

/// An operation from a firehose commit.
#[derive(Debug, Clone)]
pub enum FirehoseOp {
    /// Record created or updated.
    CreateOrUpdate {
        collection: String,
        rkey: String,
        cid: String,
        record: Vec<u8>,
    },
    /// Record deleted.
    Delete { collection: String, rkey: String },
}

/// In-memory cache for repository records.
///
/// Thread-safe and designed for concurrent access from multiple tasks.
pub struct RepoCache {
    /// Cached facts by rkey.
    facts: DashMap<String, CachedRecord<Fact>>,
    /// Cached rules by rkey.
    rules: DashMap<String, CachedRecord<Rule>>,
    /// Cached thoughts by rkey.
    thoughts: DashMap<String, CachedRecord<Thought>>,
    /// Cached notes by rkey.
    notes: DashMap<String, CachedRecord<Note>>,
    /// Cached jobs by rkey.
    jobs: DashMap<String, CachedRecord<Job>>,
    /// Cached identity (singleton).
    identity: RwLock<Option<CachedRecord<Identity>>>,
    /// Cached daemon state (singleton).
    daemon_state: RwLock<Option<CachedRecord<DaemonState>>>,
    // =========================================================================
    // Bluesky records (for derived facts)
    // =========================================================================
    /// Cached follows by rkey.
    follows: DashMap<String, CachedRecord<Follow>>,
    /// Cached likes by rkey.
    likes: DashMap<String, CachedRecord<Like>>,
    /// Cached reposts by rkey.
    reposts: DashMap<String, CachedRecord<Repost>>,
    /// Cached posts by rkey.
    posts: DashMap<String, CachedRecord<Post>>,
    // =========================================================================
    // Winter records (for derived facts)
    // =========================================================================
    /// Cached directives by rkey.
    directives: DashMap<String, CachedRecord<Directive>>,
    /// Cached custom tools by rkey.
    tools: DashMap<String, CachedRecord<CustomTool>>,
    /// Cached tool approvals by rkey.
    tool_approvals: DashMap<String, CachedRecord<ToolApproval>>,
    /// Cached blog entries by rkey.
    blog_entries: DashMap<String, CachedRecord<BlogEntry>>,
    /// Cached fact declarations by rkey.
    declarations: DashMap<String, CachedRecord<FactDeclaration>>,
    // =========================================================================
    // Sync state
    // =========================================================================
    /// Current sync state.
    state: AtomicU8,
    /// Current repository revision.
    repo_rev: RwLock<Option<String>>,
    /// Pending firehose events during CAR fetch.
    pending_events: Mutex<VecDeque<FirehoseCommit>>,
    /// Broadcast channel for cache updates.
    updates_tx: broadcast::Sender<CacheUpdate>,
}

impl RepoCache {
    /// Create a new empty cache.
    pub fn new() -> Arc<Self> {
        let (updates_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            facts: DashMap::new(),
            rules: DashMap::new(),
            thoughts: DashMap::new(),
            notes: DashMap::new(),
            jobs: DashMap::new(),
            identity: RwLock::new(None),
            daemon_state: RwLock::new(None),
            follows: DashMap::new(),
            likes: DashMap::new(),
            reposts: DashMap::new(),
            posts: DashMap::new(),
            directives: DashMap::new(),
            tools: DashMap::new(),
            tool_approvals: DashMap::new(),
            blog_entries: DashMap::new(),
            declarations: DashMap::new(),
            state: AtomicU8::new(SyncState::Disconnected as u8),
            repo_rev: RwLock::new(None),
            pending_events: Mutex::new(VecDeque::new()),
            updates_tx,
        })
    }

    /// Get the current sync state.
    pub fn state(&self) -> SyncState {
        SyncState::from(self.state.load(Ordering::SeqCst))
    }

    /// Set the sync state.
    pub fn set_state(&self, state: SyncState) {
        self.state.store(state as u8, Ordering::SeqCst);
        if state == SyncState::Live && self.updates_tx.send(CacheUpdate::Synchronized).is_err() {
            trace!("no subscribers for cache sync update");
        }
    }

    /// Get the current repository revision.
    pub async fn repo_rev(&self) -> Option<String> {
        self.repo_rev.read().await.clone()
    }

    /// Set the repository revision.
    pub async fn set_repo_rev(&self, rev: String) {
        *self.repo_rev.write().await = Some(rev);
    }

    /// Subscribe to cache updates.
    pub fn subscribe(&self) -> broadcast::Receiver<CacheUpdate> {
        self.updates_tx.subscribe()
    }

    /// Get a fact by rkey.
    pub fn get_fact(&self, rkey: &str) -> Option<CachedRecord<Fact>> {
        self.facts.get(rkey).map(|r| r.value().clone())
    }

    /// Get a rule by rkey.
    pub fn get_rule(&self, rkey: &str) -> Option<CachedRecord<Rule>> {
        self.rules.get(rkey).map(|r| r.value().clone())
    }

    /// List all facts.
    pub fn list_facts(&self) -> Vec<(String, CachedRecord<Fact>)> {
        self.facts
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// List all rules.
    pub fn list_rules(&self) -> Vec<(String, CachedRecord<Rule>)> {
        self.rules
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached facts.
    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }

    /// Get the number of cached rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Get enabled rule heads (deduplicated and sorted).
    /// Avoids full collection clone.
    pub fn enabled_rule_heads(&self) -> Vec<String> {
        let mut heads: Vec<_> = self
            .rules
            .iter()
            .filter(|r| r.value().value.enabled)
            .map(|r| r.value().value.head.clone())
            .collect();
        heads.sort();
        heads.dedup();
        heads
    }

    /// Insert or update a fact.
    pub fn upsert_fact(&self, rkey: String, fact: Fact, cid: String) {
        use dashmap::mapref::entry::Entry;

        // Move value into CachedRecord to avoid first clone
        let cached = CachedRecord { value: fact, cid };

        // Use entry API for atomic check-and-insert
        let is_update = match self.facts.entry(rkey.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(cached);
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(cached);
                false
            }
        };

        // Clone from cache only if there are subscribers (send returns Err if none)
        // Get the fact from cache for the update notification
        if let Some(cached_ref) = self.facts.get(&rkey) {
            let fact_clone = cached_ref.value().value.clone();
            let update = if is_update {
                CacheUpdate::FactUpdated {
                    rkey: rkey.clone(),
                    fact: fact_clone,
                }
            } else {
                CacheUpdate::FactCreated {
                    rkey: rkey.clone(),
                    fact: fact_clone,
                }
            };

            if let Err(e) = self.updates_tx.send(update) {
                trace!(error = %e, "no subscribers for fact update");
            }
            trace!(rkey = %rkey, predicate = %cached_ref.value().value.predicate, "cache: fact upserted");
        }
    }

    /// Delete a fact.
    pub fn delete_fact(&self, rkey: &str) {
        if self.facts.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::FactDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for fact delete");
            }
            trace!(rkey = %rkey, "cache: fact deleted");
        }
    }

    /// Insert or update a rule.
    pub fn upsert_rule(&self, rkey: String, rule: Rule, cid: String) {
        use dashmap::mapref::entry::Entry;

        // Move value into CachedRecord to avoid first clone
        let cached = CachedRecord { value: rule, cid };

        // Use entry API for atomic check-and-insert
        let is_update = match self.rules.entry(rkey.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(cached);
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(cached);
                false
            }
        };

        // Clone from cache only for update notification
        if let Some(cached_ref) = self.rules.get(&rkey) {
            let rule_clone = cached_ref.value().value.clone();
            let update = if is_update {
                CacheUpdate::RuleUpdated {
                    rkey: rkey.clone(),
                    rule: rule_clone,
                }
            } else {
                CacheUpdate::RuleCreated {
                    rkey: rkey.clone(),
                    rule: rule_clone,
                }
            };

            if let Err(e) = self.updates_tx.send(update) {
                trace!(error = %e, "no subscribers for rule update");
            }
            trace!(rkey = %rkey, name = %cached_ref.value().value.name, "cache: rule upserted");
        }
    }

    /// Delete a rule.
    pub fn delete_rule(&self, rkey: &str) {
        if self.rules.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::RuleDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for rule delete");
            }
            trace!(rkey = %rkey, "cache: rule deleted");
        }
    }

    /// Get a thought by rkey.
    pub fn get_thought(&self, rkey: &str) -> Option<CachedRecord<Thought>> {
        self.thoughts.get(rkey).map(|r| r.value().clone())
    }

    /// List all thoughts.
    pub fn list_thoughts(&self) -> Vec<(String, CachedRecord<Thought>)> {
        self.thoughts
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get N most recent thoughts.
    ///
    /// Thoughts are keyed by TID which is time-ordered, so we can sort
    /// by rkey descending to get most recent first.
    pub fn recent_thoughts(&self, limit: usize) -> Vec<Thought> {
        let mut thoughts: Vec<_> = self
            .thoughts
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect();
        // Sort by rkey descending (TIDs are time-ordered)
        thoughts.sort_by(|a, b| b.0.cmp(&a.0));
        thoughts
            .into_iter()
            .take(limit)
            .map(|(_, r)| r.value)
            .collect()
    }

    /// Get N most recent thoughts filtered by conversation scope.
    ///
    /// Filters thoughts based on trigger format:
    /// - `ScopeFilter::Thread { root_uri }`: matches `notification:*:root={root_uri}`
    /// - `ScopeFilter::DirectMessage { convo_id }`: matches `dm:{convo_id}:*`
    /// - `ScopeFilter::Job { name }`: matches exactly `job:{name}`
    /// - `ScopeFilter::Global`: only thoughts with no trigger
    ///
    /// Thoughts with `trigger: None` (global thoughts) are always included
    /// except in Global scope which only includes them.
    pub fn recent_thoughts_for_scope(&self, limit: usize, scope: &ScopeFilter) -> Vec<Thought> {
        let mut thoughts: Vec<_> = self
            .thoughts
            .iter()
            .filter(|r| thought_matches_scope(&r.value().value, scope))
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect();
        // Sort by rkey descending (TIDs are time-ordered)
        thoughts.sort_by(|a, b| b.0.cmp(&a.0));
        thoughts
            .into_iter()
            .take(limit)
            .map(|(_, r)| r.value)
            .collect()
    }

    /// Get the number of cached thoughts.
    pub fn thought_count(&self) -> usize {
        self.thoughts.len()
    }

    /// Insert or update a thought.
    pub fn upsert_thought(&self, rkey: String, thought: Thought, cid: String) {
        // Move value into CachedRecord to avoid first clone
        let cached = CachedRecord {
            value: thought,
            cid,
        };
        self.thoughts.insert(rkey.clone(), cached);

        // Clone from cache only for update notification
        if let Some(cached_ref) = self.thoughts.get(&rkey) {
            if let Err(e) = self.updates_tx.send(CacheUpdate::ThoughtCreated {
                rkey: rkey.clone(),
                thought: cached_ref.value().value.clone(),
            }) {
                trace!(error = %e, "no subscribers for thought update");
            }
            trace!(rkey = %rkey, kind = ?cached_ref.value().value.kind, "cache: thought upserted");
        }
    }

    /// Delete a thought.
    pub fn delete_thought(&self, rkey: &str) {
        if self.thoughts.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::ThoughtDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for thought delete");
            }
            trace!(rkey = %rkey, "cache: thought deleted");
        }
    }

    /// Get a note by rkey.
    pub fn get_note(&self, rkey: &str) -> Option<CachedRecord<Note>> {
        self.notes.get(rkey).map(|r| r.value().clone())
    }

    /// List all notes.
    pub fn list_notes(&self) -> Vec<(String, CachedRecord<Note>)> {
        self.notes
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached notes.
    pub fn note_count(&self) -> usize {
        self.notes.len()
    }

    /// Insert or update a note.
    pub fn upsert_note(&self, rkey: String, note: Note, cid: String) {
        use dashmap::mapref::entry::Entry;

        let cached = CachedRecord {
            value: note.clone(),
            cid,
        };

        let is_update = match self.notes.entry(rkey.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(cached);
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(cached);
                false
            }
        };

        let update = if is_update {
            CacheUpdate::NoteUpdated {
                rkey: rkey.clone(),
                note: note.clone(),
            }
        } else {
            CacheUpdate::NoteCreated {
                rkey: rkey.clone(),
                note: note.clone(),
            }
        };

        if let Err(e) = self.updates_tx.send(update) {
            trace!(error = %e, "no subscribers for note update");
        }
        trace!(rkey = %rkey, title = %note.title, "cache: note upserted");
    }

    /// Delete a note.
    pub fn delete_note(&self, rkey: &str) {
        if self.notes.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::NoteDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for note delete");
            }
            trace!(rkey = %rkey, "cache: note deleted");
        }
    }

    /// Get a job by rkey.
    pub fn get_job(&self, rkey: &str) -> Option<CachedRecord<Job>> {
        self.jobs.get(rkey).map(|r| r.value().clone())
    }

    /// List all jobs.
    pub fn list_jobs(&self) -> Vec<(String, CachedRecord<Job>)> {
        self.jobs
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached jobs.
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Insert or update a job.
    pub fn upsert_job(&self, rkey: String, job: Job, cid: String) {
        use dashmap::mapref::entry::Entry;

        let cached = CachedRecord {
            value: job.clone(),
            cid,
        };

        let is_update = match self.jobs.entry(rkey.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(cached);
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(cached);
                false
            }
        };

        let update = if is_update {
            CacheUpdate::JobUpdated {
                rkey: rkey.clone(),
                job: job.clone(),
            }
        } else {
            CacheUpdate::JobCreated {
                rkey: rkey.clone(),
                job: job.clone(),
            }
        };

        if let Err(e) = self.updates_tx.send(update) {
            trace!(error = %e, "no subscribers for job update");
        }
        trace!(rkey = %rkey, name = %job.name, "cache: job upserted");
    }

    /// Delete a job.
    pub fn delete_job(&self, rkey: &str) {
        if self.jobs.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::JobDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for job delete");
            }
            trace!(rkey = %rkey, "cache: job deleted");
        }
    }

    // =========================================================================
    // Follow methods
    // =========================================================================

    /// Get a follow by rkey.
    pub fn get_follow(&self, rkey: &str) -> Option<CachedRecord<Follow>> {
        self.follows.get(rkey).map(|r| r.value().clone())
    }

    /// List all follows.
    pub fn list_follows(&self) -> Vec<(String, CachedRecord<Follow>)> {
        self.follows
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached follows.
    pub fn follow_count(&self) -> usize {
        self.follows.len()
    }

    /// Insert a follow.
    pub fn insert_follow(&self, rkey: String, follow: Follow, cid: String) {
        let cached = CachedRecord {
            value: follow.clone(),
            cid,
        };
        self.follows.insert(rkey.clone(), cached);

        if let Err(e) = self.updates_tx.send(CacheUpdate::FollowCreated {
            rkey: rkey.clone(),
            follow: follow.clone(),
        }) {
            trace!(error = %e, "no subscribers for follow create");
        }
        trace!(rkey = %rkey, subject = %follow.subject, "cache: follow inserted");
    }

    /// Delete a follow.
    pub fn delete_follow(&self, rkey: &str) {
        if self.follows.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::FollowDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for follow delete");
            }
            trace!(rkey = %rkey, "cache: follow deleted");
        }
    }

    // =========================================================================
    // Like methods
    // =========================================================================

    /// Get a like by rkey.
    pub fn get_like(&self, rkey: &str) -> Option<CachedRecord<Like>> {
        self.likes.get(rkey).map(|r| r.value().clone())
    }

    /// List all likes.
    pub fn list_likes(&self) -> Vec<(String, CachedRecord<Like>)> {
        self.likes
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached likes.
    pub fn like_count(&self) -> usize {
        self.likes.len()
    }

    /// Insert a like.
    pub fn insert_like(&self, rkey: String, like: Like, cid: String) {
        let cached = CachedRecord {
            value: like.clone(),
            cid,
        };
        self.likes.insert(rkey.clone(), cached);

        if let Err(e) = self.updates_tx.send(CacheUpdate::LikeCreated {
            rkey: rkey.clone(),
            like: like.clone(),
        }) {
            trace!(error = %e, "no subscribers for like create");
        }
        trace!(rkey = %rkey, uri = %like.subject.uri, "cache: like inserted");
    }

    /// Delete a like.
    pub fn delete_like(&self, rkey: &str) {
        if self.likes.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::LikeDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for like delete");
            }
            trace!(rkey = %rkey, "cache: like deleted");
        }
    }

    // =========================================================================
    // Repost methods
    // =========================================================================

    /// Get a repost by rkey.
    pub fn get_repost(&self, rkey: &str) -> Option<CachedRecord<Repost>> {
        self.reposts.get(rkey).map(|r| r.value().clone())
    }

    /// List all reposts.
    pub fn list_reposts(&self) -> Vec<(String, CachedRecord<Repost>)> {
        self.reposts
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached reposts.
    pub fn repost_count(&self) -> usize {
        self.reposts.len()
    }

    /// Insert a repost.
    pub fn insert_repost(&self, rkey: String, repost: Repost, cid: String) {
        let cached = CachedRecord {
            value: repost.clone(),
            cid,
        };
        self.reposts.insert(rkey.clone(), cached);

        if let Err(e) = self.updates_tx.send(CacheUpdate::RepostCreated {
            rkey: rkey.clone(),
            repost: repost.clone(),
        }) {
            trace!(error = %e, "no subscribers for repost create");
        }
        trace!(rkey = %rkey, uri = %repost.subject.uri, "cache: repost inserted");
    }

    /// Delete a repost.
    pub fn delete_repost(&self, rkey: &str) {
        if self.reposts.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::RepostDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for repost delete");
            }
            trace!(rkey = %rkey, "cache: repost deleted");
        }
    }

    // =========================================================================
    // Post methods
    // =========================================================================

    /// Get a post by rkey.
    pub fn get_post(&self, rkey: &str) -> Option<CachedRecord<Post>> {
        self.posts.get(rkey).map(|r| r.value().clone())
    }

    /// List all posts.
    pub fn list_posts(&self) -> Vec<(String, CachedRecord<Post>)> {
        self.posts
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached posts.
    pub fn post_count(&self) -> usize {
        self.posts.len()
    }

    /// Insert or update a post.
    pub fn upsert_post(&self, rkey: String, post: Post, cid: String) {
        use dashmap::mapref::entry::Entry;

        let cached = CachedRecord {
            value: post.clone(),
            cid,
        };

        let is_update = match self.posts.entry(rkey.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(cached);
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(cached);
                false
            }
        };

        let update = if is_update {
            CacheUpdate::PostUpdated {
                rkey: rkey.clone(),
                post: post.clone(),
            }
        } else {
            CacheUpdate::PostCreated {
                rkey: rkey.clone(),
                post: post.clone(),
            }
        };

        if let Err(e) = self.updates_tx.send(update) {
            trace!(error = %e, "no subscribers for post update");
        }
        trace!(rkey = %rkey, "cache: post upserted");
    }

    /// Delete a post.
    pub fn delete_post(&self, rkey: &str) {
        if self.posts.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::PostDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for post delete");
            }
            trace!(rkey = %rkey, "cache: post deleted");
        }
    }

    // =========================================================================
    // Directive methods
    // =========================================================================

    /// Get a directive by rkey.
    pub fn get_directive(&self, rkey: &str) -> Option<CachedRecord<Directive>> {
        self.directives.get(rkey).map(|r| r.value().clone())
    }

    /// List all directives.
    pub fn list_directives(&self) -> Vec<(String, CachedRecord<Directive>)> {
        self.directives
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached directives.
    pub fn directive_count(&self) -> usize {
        self.directives.len()
    }

    /// Get active directives sorted by priority (descending) then created_at.
    /// Avoids full collection clone by only cloning active directives.
    pub fn active_directives_sorted(&self) -> Vec<Directive> {
        let mut directives: Vec<_> = self
            .directives
            .iter()
            .filter(|r| r.value().value.active)
            .map(|r| r.value().value.clone())
            .collect();
        directives.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        directives
    }

    /// Insert or update a directive.
    pub fn upsert_directive(&self, rkey: String, directive: Directive, cid: String) {
        use dashmap::mapref::entry::Entry;

        // Move value into CachedRecord to avoid first clone
        let cached = CachedRecord {
            value: directive,
            cid,
        };

        let is_update = match self.directives.entry(rkey.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(cached);
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(cached);
                false
            }
        };

        // Clone from cache only for update notification
        if let Some(cached_ref) = self.directives.get(&rkey) {
            let directive_clone = cached_ref.value().value.clone();
            let update = if is_update {
                CacheUpdate::DirectiveUpdated {
                    rkey: rkey.clone(),
                    directive: directive_clone,
                }
            } else {
                CacheUpdate::DirectiveCreated {
                    rkey: rkey.clone(),
                    directive: directive_clone,
                }
            };

            if let Err(e) = self.updates_tx.send(update) {
                trace!(error = %e, "no subscribers for directive update");
            }
            trace!(rkey = %rkey, kind = %cached_ref.value().value.kind, "cache: directive upserted");
        }
    }

    /// Delete a directive.
    pub fn delete_directive(&self, rkey: &str) {
        if self.directives.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::DirectiveDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for directive delete");
            }
            trace!(rkey = %rkey, "cache: directive deleted");
        }
    }

    // =========================================================================
    // CustomTool methods
    // =========================================================================

    /// Get a custom tool by rkey.
    pub fn get_tool(&self, rkey: &str) -> Option<CachedRecord<CustomTool>> {
        self.tools.get(rkey).map(|r| r.value().clone())
    }

    /// List all custom tools.
    pub fn list_tools(&self) -> Vec<(String, CachedRecord<CustomTool>)> {
        self.tools
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached custom tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Insert or update a custom tool.
    pub fn upsert_tool(&self, rkey: String, tool: CustomTool, cid: String) {
        use dashmap::mapref::entry::Entry;

        let cached = CachedRecord {
            value: tool.clone(),
            cid,
        };

        let is_update = match self.tools.entry(rkey.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(cached);
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(cached);
                false
            }
        };

        let update = if is_update {
            CacheUpdate::ToolUpdated {
                rkey: rkey.clone(),
                tool: tool.clone(),
            }
        } else {
            CacheUpdate::ToolCreated {
                rkey: rkey.clone(),
                tool: tool.clone(),
            }
        };

        if let Err(e) = self.updates_tx.send(update) {
            trace!(error = %e, "no subscribers for tool update");
        }
        trace!(rkey = %rkey, name = %tool.name, "cache: tool upserted");
    }

    /// Delete a custom tool.
    pub fn delete_tool(&self, rkey: &str) {
        if self.tools.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::ToolDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for tool delete");
            }
            trace!(rkey = %rkey, "cache: tool deleted");
        }
    }

    // =========================================================================
    // ToolApproval methods
    // =========================================================================

    /// Get a tool approval by rkey.
    pub fn get_tool_approval(&self, rkey: &str) -> Option<CachedRecord<ToolApproval>> {
        self.tool_approvals.get(rkey).map(|r| r.value().clone())
    }

    /// List all tool approvals.
    pub fn list_tool_approvals(&self) -> Vec<(String, CachedRecord<ToolApproval>)> {
        self.tool_approvals
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached tool approvals.
    pub fn tool_approval_count(&self) -> usize {
        self.tool_approvals.len()
    }

    /// Insert or update a tool approval.
    pub fn upsert_tool_approval(&self, rkey: String, approval: ToolApproval, cid: String) {
        use dashmap::mapref::entry::Entry;

        let cached = CachedRecord {
            value: approval.clone(),
            cid,
        };

        let is_update = match self.tool_approvals.entry(rkey.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(cached);
                true
            }
            Entry::Vacant(entry) => {
                entry.insert(cached);
                false
            }
        };

        let update = if is_update {
            CacheUpdate::ToolApprovalUpdated {
                rkey: rkey.clone(),
                approval: approval.clone(),
            }
        } else {
            CacheUpdate::ToolApprovalCreated {
                rkey: rkey.clone(),
                approval: approval.clone(),
            }
        };

        if let Err(e) = self.updates_tx.send(update) {
            trace!(error = %e, "no subscribers for tool approval update");
        }
        trace!(rkey = %rkey, tool_rkey = %approval.tool_rkey, "cache: tool approval upserted");
    }

    /// Delete a tool approval.
    pub fn delete_tool_approval(&self, rkey: &str) {
        if self.tool_approvals.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::ToolApprovalDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for tool approval delete");
            }
            trace!(rkey = %rkey, "cache: tool approval deleted");
        }
    }

    // =========================================================================
    // BlogEntry methods
    // =========================================================================

    /// Get a blog entry by rkey.
    pub fn get_blog_entry(&self, rkey: &str) -> Option<CachedRecord<BlogEntry>> {
        self.blog_entries.get(rkey).map(|r| r.value().clone())
    }

    /// List all blog entries.
    pub fn list_blog_entries(&self) -> Vec<(String, CachedRecord<BlogEntry>)> {
        self.blog_entries
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached blog entries.
    pub fn blog_entry_count(&self) -> usize {
        self.blog_entries.len()
    }

    /// Insert or update a blog entry.
    pub fn upsert_blog_entry(&self, rkey: String, entry: BlogEntry, cid: String) {
        use dashmap::mapref::entry::Entry;

        let cached = CachedRecord {
            value: entry.clone(),
            cid,
        };

        let is_update = match self.blog_entries.entry(rkey.clone()) {
            Entry::Occupied(mut e) => {
                e.insert(cached);
                true
            }
            Entry::Vacant(e) => {
                e.insert(cached);
                false
            }
        };

        let update = if is_update {
            CacheUpdate::BlogEntryUpdated {
                rkey: rkey.clone(),
                entry: entry.clone(),
            }
        } else {
            CacheUpdate::BlogEntryCreated {
                rkey: rkey.clone(),
                entry: entry.clone(),
            }
        };

        if let Err(e) = self.updates_tx.send(update) {
            trace!(error = %e, "no subscribers for blog entry update");
        }
        trace!(rkey = %rkey, title = %entry.title, "cache: blog entry upserted");
    }

    /// Delete a blog entry.
    pub fn delete_blog_entry(&self, rkey: &str) {
        if self.blog_entries.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::BlogEntryDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for blog entry delete");
            }
            trace!(rkey = %rkey, "cache: blog entry deleted");
        }
    }

    // =========================================================================
    // FactDeclaration methods
    // =========================================================================

    /// Get a fact declaration by rkey.
    pub fn get_declaration(&self, rkey: &str) -> Option<CachedRecord<FactDeclaration>> {
        self.declarations.get(rkey).map(|r| r.value().clone())
    }

    /// List all fact declarations.
    pub fn list_declarations(&self) -> Vec<(String, CachedRecord<FactDeclaration>)> {
        self.declarations
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Get the number of cached fact declarations.
    pub fn declaration_count(&self) -> usize {
        self.declarations.len()
    }

    /// Insert or update a fact declaration.
    pub fn upsert_declaration(&self, rkey: String, declaration: FactDeclaration, cid: String) {
        use dashmap::mapref::entry::Entry;

        let cached = CachedRecord {
            value: declaration.clone(),
            cid,
        };

        let is_update = match self.declarations.entry(rkey.clone()) {
            Entry::Occupied(mut e) => {
                e.insert(cached);
                true
            }
            Entry::Vacant(e) => {
                e.insert(cached);
                false
            }
        };

        let update = if is_update {
            CacheUpdate::DeclarationUpdated {
                rkey: rkey.clone(),
                declaration: declaration.clone(),
            }
        } else {
            CacheUpdate::DeclarationCreated {
                rkey: rkey.clone(),
                declaration: declaration.clone(),
            }
        };

        if let Err(e) = self.updates_tx.send(update) {
            trace!(error = %e, "no subscribers for declaration update");
        }
        trace!(rkey = %rkey, predicate = %declaration.predicate, "cache: declaration upserted");
    }

    /// Delete a fact declaration.
    pub fn delete_declaration(&self, rkey: &str) {
        if self.declarations.remove(rkey).is_some() {
            if let Err(e) = self.updates_tx.send(CacheUpdate::DeclarationDeleted {
                rkey: rkey.to_string(),
            }) {
                trace!(error = %e, "no subscribers for declaration delete");
            }
            trace!(rkey = %rkey, "cache: declaration deleted");
        }
    }

    /// Get the cached identity.
    pub async fn get_identity(&self) -> Option<CachedRecord<Identity>> {
        self.identity.read().await.clone()
    }

    /// Set the identity.
    pub async fn set_identity(&self, identity: Identity, cid: String) {
        let cached = CachedRecord {
            value: identity.clone(),
            cid,
        };
        *self.identity.write().await = Some(cached);

        if let Err(e) = self.updates_tx.send(CacheUpdate::IdentityUpdated {
            identity: identity.clone(),
        }) {
            trace!(error = %e, "no subscribers for identity update");
        }
        trace!("cache: identity set");
    }

    /// Get the cached daemon state.
    pub async fn get_daemon_state(&self) -> Option<CachedRecord<DaemonState>> {
        self.daemon_state.read().await.clone()
    }

    /// Set the daemon state.
    pub async fn set_daemon_state(&self, state: DaemonState, cid: String) {
        let cached = CachedRecord {
            value: state.clone(),
            cid,
        };
        *self.daemon_state.write().await = Some(cached);

        if let Err(e) = self.updates_tx.send(CacheUpdate::StateUpdated {
            state: state.clone(),
        }) {
            trace!(error = %e, "no subscribers for state update");
        }
        trace!(followers = state.followers.len(), "cache: daemon state set");
    }

    /// Get the followers from daemon state.
    pub async fn get_followers(&self) -> Vec<String> {
        self.daemon_state
            .read()
            .await
            .as_ref()
            .map(|s| s.value.followers.clone())
            .unwrap_or_default()
    }

    /// Clear all cached data.
    pub fn clear(&self) {
        self.facts.clear();
        self.rules.clear();
        self.thoughts.clear();
        self.notes.clear();
        self.jobs.clear();
        self.follows.clear();
        self.likes.clear();
        self.reposts.clear();
        self.posts.clear();
        self.directives.clear();
        self.tools.clear();
        self.tool_approvals.clear();
        self.blog_entries.clear();
        self.declarations.clear();
        debug!("cache cleared");
    }

    /// Queue a firehose commit for later replay.
    pub async fn queue_commit(&self, commit: FirehoseCommit) {
        let mut queue = self.pending_events.lock().await;
        queue.push_back(commit);
        trace!(queue_len = queue.len(), "queued firehose commit");
    }

    /// Drain all pending commits.
    pub async fn drain_pending(&self) -> Vec<FirehoseCommit> {
        let mut queue = self.pending_events.lock().await;
        queue.drain(..).collect()
    }

    /// Populate cache from CAR parse result (legacy method for facts and rules only).
    pub fn populate_from_car(
        &self,
        facts: impl IntoIterator<Item = (String, Fact, String)>,
        rules: impl IntoIterator<Item = (String, Rule, String)>,
    ) {
        for (rkey, fact, cid) in facts {
            self.facts.insert(rkey, CachedRecord { value: fact, cid });
        }

        for (rkey, rule, cid) in rules {
            self.rules.insert(rkey, CachedRecord { value: rule, cid });
        }

        debug!(
            facts = self.facts.len(),
            rules = self.rules.len(),
            "cache populated from CAR"
        );
    }

    /// Populate cache from extended CAR parse result.
    #[allow(clippy::too_many_arguments)]
    pub fn populate_from_car_extended(
        &self,
        facts: impl IntoIterator<Item = (String, Fact, String)>,
        rules: impl IntoIterator<Item = (String, Rule, String)>,
        thoughts: impl IntoIterator<Item = (String, Thought, String)>,
        notes: impl IntoIterator<Item = (String, Note, String)>,
        jobs: impl IntoIterator<Item = (String, Job, String)>,
        identity: Option<(Identity, String)>,
    ) {
        for (rkey, fact, cid) in facts {
            self.facts.insert(rkey, CachedRecord { value: fact, cid });
        }

        for (rkey, rule, cid) in rules {
            self.rules.insert(rkey, CachedRecord { value: rule, cid });
        }

        for (rkey, thought, cid) in thoughts {
            self.thoughts.insert(
                rkey,
                CachedRecord {
                    value: thought,
                    cid,
                },
            );
        }

        for (rkey, note, cid) in notes {
            self.notes.insert(rkey, CachedRecord { value: note, cid });
        }

        for (rkey, job, cid) in jobs {
            self.jobs.insert(rkey, CachedRecord { value: job, cid });
        }

        if let Some((id, cid)) = identity {
            // Use blocking write for sync context
            if let Ok(mut guard) = self.identity.try_write() {
                *guard = Some(CachedRecord { value: id, cid });
            }
        }

        debug!(
            facts = self.facts.len(),
            rules = self.rules.len(),
            thoughts = self.thoughts.len(),
            notes = self.notes.len(),
            jobs = self.jobs.len(),
            "cache populated from CAR (extended)"
        );
    }

    /// Populate cache from full CAR parse result including all record types.
    #[allow(clippy::too_many_arguments)]
    pub fn populate_from_car_full(
        &self,
        facts: impl IntoIterator<Item = (String, Fact, String)>,
        rules: impl IntoIterator<Item = (String, Rule, String)>,
        thoughts: impl IntoIterator<Item = (String, Thought, String)>,
        notes: impl IntoIterator<Item = (String, Note, String)>,
        jobs: impl IntoIterator<Item = (String, Job, String)>,
        identity: Option<(Identity, String)>,
        follows: impl IntoIterator<Item = (String, Follow, String)>,
        likes: impl IntoIterator<Item = (String, Like, String)>,
        reposts: impl IntoIterator<Item = (String, Repost, String)>,
        posts: impl IntoIterator<Item = (String, Post, String)>,
        directives: impl IntoIterator<Item = (String, Directive, String)>,
        declarations: impl IntoIterator<Item = (String, FactDeclaration, String)>,
        tools: impl IntoIterator<Item = (String, CustomTool, String)>,
        tool_approvals: impl IntoIterator<Item = (String, ToolApproval, String)>,
        blog_entries: impl IntoIterator<Item = (String, BlogEntry, String)>,
    ) {
        // Winter collections
        for (rkey, fact, cid) in facts {
            self.facts.insert(rkey, CachedRecord { value: fact, cid });
        }
        for (rkey, rule, cid) in rules {
            self.rules.insert(rkey, CachedRecord { value: rule, cid });
        }
        for (rkey, thought, cid) in thoughts {
            self.thoughts.insert(
                rkey,
                CachedRecord {
                    value: thought,
                    cid,
                },
            );
        }
        for (rkey, note, cid) in notes {
            self.notes.insert(rkey, CachedRecord { value: note, cid });
        }
        for (rkey, job, cid) in jobs {
            self.jobs.insert(rkey, CachedRecord { value: job, cid });
        }
        if let Some((id, cid)) = identity {
            if let Ok(mut guard) = self.identity.try_write() {
                *guard = Some(CachedRecord { value: id, cid });
            }
        }
        for (rkey, directive, cid) in directives {
            self.directives.insert(
                rkey,
                CachedRecord {
                    value: directive,
                    cid,
                },
            );
        }
        for (rkey, declaration, cid) in declarations {
            self.declarations.insert(
                rkey,
                CachedRecord {
                    value: declaration,
                    cid,
                },
            );
        }
        for (rkey, tool, cid) in tools {
            self.tools.insert(rkey, CachedRecord { value: tool, cid });
        }
        for (rkey, approval, cid) in tool_approvals {
            self.tool_approvals.insert(
                rkey,
                CachedRecord {
                    value: approval,
                    cid,
                },
            );
        }

        // Bluesky collections
        for (rkey, follow, cid) in follows {
            self.follows
                .insert(rkey, CachedRecord { value: follow, cid });
        }
        for (rkey, like, cid) in likes {
            self.likes.insert(rkey, CachedRecord { value: like, cid });
        }
        for (rkey, repost, cid) in reposts {
            self.reposts
                .insert(rkey, CachedRecord { value: repost, cid });
        }
        for (rkey, post, cid) in posts {
            self.posts.insert(rkey, CachedRecord { value: post, cid });
        }

        // WhiteWind blog entries
        for (rkey, entry, cid) in blog_entries {
            self.blog_entries
                .insert(rkey, CachedRecord { value: entry, cid });
        }

        debug!(
            facts = self.facts.len(),
            rules = self.rules.len(),
            thoughts = self.thoughts.len(),
            notes = self.notes.len(),
            jobs = self.jobs.len(),
            follows = self.follows.len(),
            likes = self.likes.len(),
            reposts = self.reposts.len(),
            posts = self.posts.len(),
            directives = self.directives.len(),
            tools = self.tools.len(),
            tool_approvals = self.tool_approvals.len(),
            blog_entries = self.blog_entries.len(),
            "cache populated from CAR (full)"
        );
    }
}

impl Default for RepoCache {
    fn default() -> Self {
        let (updates_tx, _) = broadcast::channel(256);
        Self {
            facts: DashMap::new(),
            rules: DashMap::new(),
            thoughts: DashMap::new(),
            notes: DashMap::new(),
            jobs: DashMap::new(),
            identity: RwLock::new(None),
            daemon_state: RwLock::new(None),
            follows: DashMap::new(),
            likes: DashMap::new(),
            reposts: DashMap::new(),
            posts: DashMap::new(),
            directives: DashMap::new(),
            tools: DashMap::new(),
            tool_approvals: DashMap::new(),
            blog_entries: DashMap::new(),
            declarations: DashMap::new(),
            state: AtomicU8::new(SyncState::Disconnected as u8),
            repo_rev: RwLock::new(None),
            pending_events: Mutex::new(VecDeque::new()),
            updates_tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_fact() -> Fact {
        Fact {
            predicate: "test".to_string(),
            args: vec!["a".to_string(), "b".to_string()],
            confidence: None,
            source: None,
            supersedes: None,
            tags: vec![],
            created_at: Utc::now(),
        }
    }

    fn test_rule() -> Rule {
        Rule {
            name: "test_rule".to_string(),
            description: "A test rule".to_string(),
            head: "result(X)".to_string(),
            body: vec!["input(X)".to_string()],
            constraints: vec![],
            enabled: true,
            priority: 0,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_cache_facts() {
        let cache = RepoCache::new();

        // Insert fact
        cache.upsert_fact("rkey1".to_string(), test_fact(), "cid1".to_string());
        assert_eq!(cache.fact_count(), 1);

        // Get fact
        let fact = cache.get_fact("rkey1");
        assert!(fact.is_some());
        assert_eq!(fact.unwrap().value.predicate, "test");

        // Delete fact
        cache.delete_fact("rkey1");
        assert_eq!(cache.fact_count(), 0);
    }

    #[test]
    fn test_cache_rules() {
        let cache = RepoCache::new();

        // Insert rule
        cache.upsert_rule("rkey1".to_string(), test_rule(), "cid1".to_string());
        assert_eq!(cache.rule_count(), 1);

        // Get rule
        let rule = cache.get_rule("rkey1");
        assert!(rule.is_some());
        assert_eq!(rule.unwrap().value.name, "test_rule");

        // Delete rule
        cache.delete_rule("rkey1");
        assert_eq!(cache.rule_count(), 0);
    }

    #[test]
    fn test_sync_state() {
        let cache = RepoCache::new();
        assert_eq!(cache.state(), SyncState::Disconnected);

        cache.set_state(SyncState::Syncing);
        assert_eq!(cache.state(), SyncState::Syncing);

        cache.set_state(SyncState::Live);
        assert_eq!(cache.state(), SyncState::Live);
    }

    #[test]
    fn test_upsert_fact_update_vs_create() {
        let cache = RepoCache::new();

        // First insert - should be create
        cache.upsert_fact("rkey1".to_string(), test_fact(), "cid1".to_string());
        assert_eq!(cache.fact_count(), 1);

        // Second insert with same key - should be update
        let mut updated_fact = test_fact();
        updated_fact.predicate = "updated".to_string();
        cache.upsert_fact("rkey1".to_string(), updated_fact, "cid2".to_string());

        // Count should still be 1
        assert_eq!(cache.fact_count(), 1);

        // Value should be updated
        let fact = cache.get_fact("rkey1").unwrap();
        assert_eq!(fact.value.predicate, "updated");
        assert_eq!(fact.cid, "cid2");
    }

    #[test]
    fn test_list_facts() {
        let cache = RepoCache::new();

        cache.upsert_fact("rkey1".to_string(), test_fact(), "cid1".to_string());
        cache.upsert_fact("rkey2".to_string(), test_fact(), "cid2".to_string());
        cache.upsert_fact("rkey3".to_string(), test_fact(), "cid3".to_string());

        let facts = cache.list_facts();
        assert_eq!(facts.len(), 3);

        let rkeys: std::collections::HashSet<_> = facts.iter().map(|(k, _)| k.as_str()).collect();
        assert!(rkeys.contains("rkey1"));
        assert!(rkeys.contains("rkey2"));
        assert!(rkeys.contains("rkey3"));
    }

    #[test]
    fn test_populate_from_car() {
        let cache = RepoCache::new();

        let facts = vec![
            ("f1".to_string(), test_fact(), "cid1".to_string()),
            ("f2".to_string(), test_fact(), "cid2".to_string()),
        ];

        let rules = vec![("r1".to_string(), test_rule(), "cid3".to_string())];

        cache.populate_from_car(facts, rules);

        assert_eq!(cache.fact_count(), 2);
        assert_eq!(cache.rule_count(), 1);
    }

    #[test]
    fn test_clear() {
        let cache = RepoCache::new();

        cache.upsert_fact("f1".to_string(), test_fact(), "cid1".to_string());
        cache.upsert_rule("r1".to_string(), test_rule(), "cid2".to_string());

        assert!(cache.fact_count() > 0);
        assert!(cache.rule_count() > 0);

        cache.clear();

        assert_eq!(cache.fact_count(), 0);
        assert_eq!(cache.rule_count(), 0);
    }

    #[test]
    fn test_delete_nonexistent() {
        let cache = RepoCache::new();

        // Should not panic
        cache.delete_fact("nonexistent");
        cache.delete_rule("nonexistent");

        assert_eq!(cache.fact_count(), 0);
        assert_eq!(cache.rule_count(), 0);
    }

    #[test]
    fn test_get_nonexistent() {
        let cache = RepoCache::new();

        assert!(cache.get_fact("nonexistent").is_none());
        assert!(cache.get_rule("nonexistent").is_none());
    }

    // Concurrent access tests

    #[test]
    fn test_concurrent_fact_inserts() {
        use std::sync::Arc;
        use std::thread;

        let cache = RepoCache::new();
        let cache = Arc::clone(&cache);

        // Spawn multiple threads that insert facts concurrently
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let cache = Arc::clone(&cache);
                thread::spawn(move || {
                    for j in 0..100 {
                        let rkey = format!("thread{}_{}", i, j);
                        cache.upsert_fact(rkey, test_fact(), format!("cid{}_{}", i, j));
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have 10 * 100 = 1000 facts
        assert_eq!(cache.fact_count(), 1000);
    }

    #[test]
    fn test_concurrent_read_write() {
        use std::sync::Arc;
        use std::thread;

        let cache = RepoCache::new();

        // Pre-populate with some facts
        for i in 0..100 {
            cache.upsert_fact(format!("rkey{}", i), test_fact(), format!("cid{}", i));
        }

        let cache = Arc::clone(&cache);

        // Spawn readers and writers concurrently
        let mut handles = Vec::new();

        // Writers
        for i in 0..5 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for j in 0..50 {
                    let rkey = format!("writer{}_{}", i, j);
                    cache.upsert_fact(rkey, test_fact(), format!("cid_w{}_{}", i, j));
                }
            }));
        }

        // Readers
        for _ in 0..5 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    // Just read, result may or may not exist
                    let _ = cache.get_fact(&format!("rkey{}", i));
                    let _ = cache.list_facts();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Original 100 + 5 writers * 50 = 350
        assert_eq!(cache.fact_count(), 350);
    }

    #[test]
    fn test_concurrent_updates_same_key() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

        let cache = RepoCache::new();
        let cache = Arc::clone(&cache);
        let update_count = Arc::new(AtomicUsize::new(0));

        // All threads update the same key
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let cache = Arc::clone(&cache);
                let count = Arc::clone(&update_count);
                thread::spawn(move || {
                    for j in 0..100 {
                        let mut fact = test_fact();
                        fact.predicate = format!("thread{}_{}", i, j);
                        cache.upsert_fact(
                            "shared_key".to_string(),
                            fact,
                            format!("cid{}_{}", i, j),
                        );
                        count.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Should still only have 1 fact (same key)
        assert_eq!(cache.fact_count(), 1);
        // All 1000 updates should have happened
        assert_eq!(update_count.load(Ordering::SeqCst), 1000);
    }

    #[tokio::test]
    async fn test_repo_rev() {
        let cache = RepoCache::new();

        assert!(cache.repo_rev().await.is_none());

        cache.set_repo_rev("rev123".to_string()).await;
        assert_eq!(cache.repo_rev().await, Some("rev123".to_string()));
    }

    #[tokio::test]
    async fn test_pending_events_queue() {
        let cache = RepoCache::new();

        // Queue some commits
        let commit1 = FirehoseCommit {
            rev: "rev1".to_string(),
            ops: vec![],
        };
        let commit2 = FirehoseCommit {
            rev: "rev2".to_string(),
            ops: vec![],
        };

        cache.queue_commit(commit1).await;
        cache.queue_commit(commit2).await;

        // Drain and verify order
        let drained = cache.drain_pending().await;
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].rev, "rev1");
        assert_eq!(drained[1].rev, "rev2");

        // Should be empty now
        let empty = cache.drain_pending().await;
        assert!(empty.is_empty());
    }

    #[test]
    fn test_subscribe_updates() {
        let cache = RepoCache::new();

        // Subscribe before inserting
        let mut rx = cache.subscribe();

        // Insert a fact
        cache.upsert_fact("rkey1".to_string(), test_fact(), "cid1".to_string());

        // Should receive the create event
        let update = rx.try_recv();
        assert!(update.is_ok());
        match update.unwrap() {
            CacheUpdate::FactCreated { rkey, .. } => assert_eq!(rkey, "rkey1"),
            _ => panic!("Expected FactCreated"),
        }
    }
}
