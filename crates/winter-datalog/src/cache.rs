//! Datalog query caching for efficient incremental execution.
//!
//! The `DatalogCache` maintains:
//! - Persistent directory for TSV fact files
//! - Predicate arities for declaration generation
//! - Facts indexed by rkey for incremental updates
//! - Cached base program (declarations + compiled rules)
//! - Generation counters for invalidation
//! - Dirty predicates needing TSV regeneration

use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{RwLock, broadcast};
use tracing::{debug, info, trace, warn};

use winter_atproto::{CacheUpdate, Fact, FactDeclaration, RepoCache, Rule, SyncState};

use crate::dependency::{METADATA_PREDICATES, PredicateDependencyGraph, is_metadata_predicate};
use crate::derived::DerivedFactGenerator;
use crate::error::DatalogError;
use crate::validator::validate_fact_against_declaration;
use crate::{RuleCompiler, SouffleExecutor};

/// Cached data for a single fact.
#[derive(Debug, Clone)]
pub struct CachedFactData {
    /// The fact record.
    pub fact: Fact,
    /// The CID of this record.
    pub cid: String,
    /// Whether this fact is superseded by another.
    pub is_superseded: bool,
}

/// Cache for datalog query execution.
///
/// Maintains persistent TSV files and cached program text for efficient
/// incremental query execution.
pub struct DatalogCache {
    /// Persistent directory for TSV files.
    fact_dir: PathBuf,

    /// Winter's DID for derived fact generation.
    /// Stored for reference but used via DerivedFactGenerator.
    #[allow(dead_code)]
    self_did: Option<String>,

    /// Winter's handle (for blog WhiteWind URLs in derived facts).
    /// Stored for reference but used via DerivedFactGenerator.
    #[allow(dead_code)]
    handle: Option<String>,

    /// Predicate name -> arity (for declaration generation).
    predicate_arities: RwLock<HashMap<String, usize>>,

    /// Facts indexed by rkey (for incremental updates).
    facts_by_rkey: RwLock<HashMap<String, CachedFactData>>,

    /// CID to rkey mapping for supersession lookups.
    cid_to_rkey: RwLock<HashMap<String, String>>,

    /// Set of CIDs that have been superseded.
    superseded_cids: RwLock<HashSet<String>>,

    /// Cached rules (for compilation), keyed by rkey.
    rules: RwLock<HashMap<String, Rule>>,

    /// Cached fact declarations (for query-time .decl generation), keyed by rkey.
    declarations: RwLock<HashMap<String, FactDeclaration>>,

    /// Fact declarations indexed by predicate name for validation.
    /// This is a secondary index maintained alongside `declarations`.
    declarations_by_predicate: RwLock<HashMap<String, FactDeclaration>>,

    /// Facts generation counter (bumped on any fact change).
    facts_generation: AtomicU64,

    /// Rules generation counter (bumped on any rule change).
    rules_generation: AtomicU64,

    /// Predicates needing TSV regeneration.
    dirty_predicates: RwLock<HashSet<String>>,

    /// Whether all predicates are dirty (full regeneration needed).
    /// When true, lazy regeneration is enabled - predicates are regenerated on-demand.
    full_regen_needed: RwLock<bool>,

    /// Predicates with fresh (up-to-date) TSV files.
    /// Used for lazy regeneration - only predicates in this set have valid TSV files.
    fresh_predicates: RwLock<HashSet<String>>,

    /// Lock to serialize regeneration operations.
    /// Prevents multiple queries from doing redundant regeneration work.
    regen_lock: tokio::sync::Mutex<()>,

    /// Soufflé executor for query execution.
    executor: SouffleExecutor,

    /// Derived fact generator for Bluesky/Winter record-based facts.
    derived: RwLock<DerivedFactGenerator>,
}

impl DatalogCache {
    /// Create a new DatalogCache with the given cache directory.
    ///
    /// The directory will be created if it doesn't exist.
    /// Use `new_with_did` to enable derived fact generation.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Result<Arc<Self>, DatalogError> {
        Self::new_with_did(cache_dir, None, None)
    }

    /// Create a new DatalogCache with a specific DID and handle for derived facts.
    ///
    /// The `self_did` is used to generate derived facts like `follows(self, target)`.
    /// The `handle` is used for WhiteWind blog URLs.
    pub fn new_with_did(
        cache_dir: impl Into<PathBuf>,
        self_did: Option<String>,
        handle: Option<String>,
    ) -> Result<Arc<Self>, DatalogError> {
        let fact_dir = cache_dir.into();
        std::fs::create_dir_all(&fact_dir)?;

        // Create derived generator with empty values if not provided
        let derived_did = self_did.clone().unwrap_or_default();
        let derived_handle = handle.clone().unwrap_or_default();

        Ok(Arc::new(Self {
            fact_dir,
            self_did,
            handle,
            predicate_arities: RwLock::new(HashMap::new()),
            facts_by_rkey: RwLock::new(HashMap::new()),
            cid_to_rkey: RwLock::new(HashMap::new()),
            superseded_cids: RwLock::new(HashSet::new()),
            rules: RwLock::new(HashMap::new()),
            declarations: RwLock::new(HashMap::new()),
            declarations_by_predicate: RwLock::new(HashMap::new()),
            facts_generation: AtomicU64::new(0),
            rules_generation: AtomicU64::new(0),
            dirty_predicates: RwLock::new(HashSet::new()),
            full_regen_needed: RwLock::new(true),
            fresh_predicates: RwLock::new(HashSet::new()),
            regen_lock: tokio::sync::Mutex::new(()),
            executor: SouffleExecutor::new(),
            derived: RwLock::new(DerivedFactGenerator::new(derived_did, derived_handle)),
        }))
    }

    /// Create a new DatalogCache using a temp directory.
    pub fn new_temp() -> Result<Arc<Self>, DatalogError> {
        let temp_dir = tempfile::tempdir()?;
        // Keep the TempDir around by converting it to PathBuf
        let path = temp_dir.keep();
        Self::new(path)
    }

    /// Set the DID and handle for derived fact generation.
    ///
    /// This recreates the derived generator with the new values.
    pub async fn set_self_did(&self, did: String, handle: String) {
        let mut derived = self.derived.write().await;
        *derived = DerivedFactGenerator::new(did, handle);
        derived.mark_all_dirty();
    }

    /// Get the fact directory path.
    pub fn fact_dir(&self) -> &Path {
        &self.fact_dir
    }

    /// Start listening for updates from a RepoCache.
    ///
    /// This spawns a background task that processes cache updates
    /// and maintains the datalog cache in sync. When the RepoCache
    /// becomes synchronized, it will also perform initial population
    /// of derived facts.
    ///
    /// If the RepoCache is already synchronized when this is called,
    /// population happens immediately.
    pub fn start_update_listener(self: &Arc<Self>, repo_cache: Arc<RepoCache>) {
        let mut rx = repo_cache.subscribe();
        let cache = Arc::clone(self);

        // Check if already synchronized - if so, we need to populate immediately
        // because we won't receive the Synchronized event
        let already_live = repo_cache.state() == SyncState::Live;

        tokio::spawn(async move {
            let mut populated = false;

            // If already live when we started, populate now
            if already_live {
                debug!("repo cache already synchronized, populating datalog cache immediately");
                cache.populate_from_repo_cache(&repo_cache).await;
                populated = true;
            }

            loop {
                match rx.recv().await {
                    Ok(CacheUpdate::Synchronized) => {
                        if !populated {
                            debug!("repo cache synchronized, populating datalog cache");
                            cache.populate_from_repo_cache(&repo_cache).await;
                            populated = true;
                        }
                    }
                    Ok(update) => {
                        if let Err(e) = cache.handle_update(update).await {
                            warn!(error = %e, "failed to handle cache update");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // This happens when updates come in faster than we can process them
                        // (e.g., during firehose reconnection with many catch-up events).
                        // Re-populate from repo cache to ensure consistency.
                        warn!(
                            skipped = n,
                            "datalog cache update listener lagged, re-populating from repo cache"
                        );
                        cache.populate_from_repo_cache(&repo_cache).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("repo cache closed, stopping datalog cache listener");
                        break;
                    }
                }
            }
        });
    }

    /// Populate the cache from a RepoCache snapshot.
    ///
    /// This should be called once after the RepoCache is synchronized.
    pub async fn populate_from_repo_cache(&self, repo_cache: &RepoCache) {
        info!(
            repo_state = ?repo_cache.state(),
            repo_notes = repo_cache.note_count(),
            repo_thoughts = repo_cache.thought_count(),
            repo_blog_entries = repo_cache.blog_entry_count(),
            "starting datalog cache population from repo cache"
        );

        info!("loading facts and rules from repo cache");
        let facts = repo_cache.list_facts();
        let rules = repo_cache.list_rules();
        info!(
            facts = facts.len(),
            rules = rules.len(),
            "loaded facts and rules"
        );

        // Clear existing data
        info!("clearing existing datalog cache data");
        {
            let mut facts_guard = self.facts_by_rkey.write().await;
            let mut cid_guard = self.cid_to_rkey.write().await;
            let mut arities_guard = self.predicate_arities.write().await;
            let mut superseded_guard = self.superseded_cids.write().await;

            facts_guard.clear();
            cid_guard.clear();
            arities_guard.clear();
            superseded_guard.clear();

            // First pass: collect all superseded CIDs
            for (_, cached) in &facts {
                if let Some(ref supersedes) = cached.value.supersedes {
                    superseded_guard.insert(supersedes.clone());
                }
            }

            // Second pass: insert facts
            for (rkey, cached) in facts.clone() {
                let is_superseded = superseded_guard.contains(&cached.cid);

                // Track arity
                arities_guard
                    .entry(cached.value.predicate.clone())
                    .or_insert_with(|| cached.value.args.len());

                // Track CID -> rkey
                cid_guard.insert(cached.cid.clone(), rkey.clone());

                facts_guard.insert(
                    rkey,
                    CachedFactData {
                        fact: cached.value,
                        cid: cached.cid,
                        is_superseded,
                    },
                );
            }
        }

        // Insert rules (keyed by rkey)
        info!("inserting rules into cache");
        {
            let mut rules_guard = self.rules.write().await;
            rules_guard.clear();
            for (rkey, cached) in rules {
                rules_guard.insert(rkey, cached.value);
            }
        }
        info!("rules inserted, starting derived facts population");

        // Populate derived facts generator with all record types
        {
            let mut derived = self.derived.write().await;

            // Facts (for tags)
            info!(count = facts.len(), "populating facts (for tags)");
            for (rkey, cached) in &facts {
                derived.handle_update(&CacheUpdate::FactCreated {
                    rkey: rkey.clone(),
                    fact: cached.value.clone(),
                });
            }

            // Follows
            let follows_list = repo_cache.list_follows();
            info!(count = follows_list.len(), "populating follows");
            for (rkey, cached) in follows_list {
                derived.handle_update(&CacheUpdate::FollowCreated {
                    rkey,
                    follow: cached.value,
                });
            }

            // Likes
            let likes_list = repo_cache.list_likes();
            info!(count = likes_list.len(), "populating likes");
            for (rkey, cached) in likes_list {
                derived.handle_update(&CacheUpdate::LikeCreated {
                    rkey,
                    like: cached.value,
                });
            }

            // Reposts
            let reposts_list = repo_cache.list_reposts();
            info!(count = reposts_list.len(), "populating reposts");
            for (rkey, cached) in reposts_list {
                derived.handle_update(&CacheUpdate::RepostCreated {
                    rkey,
                    repost: cached.value,
                });
            }

            // Posts
            let posts_list = repo_cache.list_posts();
            info!(count = posts_list.len(), "populating posts");
            for (rkey, cached) in posts_list {
                derived.handle_update(&CacheUpdate::PostCreated {
                    rkey,
                    post: cached.value,
                });
            }

            // Directives
            let directives_list = repo_cache.list_directives();
            info!(count = directives_list.len(), "populating directives");
            for (rkey, cached) in directives_list {
                derived.handle_update(&CacheUpdate::DirectiveCreated {
                    rkey,
                    directive: cached.value,
                });
            }

            // Tools
            let tools_list = repo_cache.list_tools();
            info!(count = tools_list.len(), "populating tools");
            for (rkey, cached) in tools_list {
                derived.handle_update(&CacheUpdate::ToolCreated {
                    rkey,
                    tool: cached.value,
                });
            }

            // Tool approvals
            let tool_approvals_list = repo_cache.list_tool_approvals();
            info!(
                count = tool_approvals_list.len(),
                "populating tool approvals"
            );
            for (rkey, cached) in tool_approvals_list {
                derived.handle_update(&CacheUpdate::ToolApprovalCreated {
                    rkey,
                    approval: cached.value,
                });
            }

            // Jobs
            let jobs_list = repo_cache.list_jobs();
            info!(count = jobs_list.len(), "populating jobs");
            for (rkey, cached) in jobs_list {
                derived.handle_update(&CacheUpdate::JobCreated {
                    rkey,
                    job: cached.value,
                });
            }

            // Notes
            let notes_list = repo_cache.list_notes();
            info!(count = notes_list.len(), "populating notes");
            for (rkey, cached) in notes_list {
                derived.handle_update(&CacheUpdate::NoteCreated {
                    rkey,
                    note: cached.value,
                });
            }

            // Thoughts
            let thoughts_list = repo_cache.list_thoughts();
            info!(count = thoughts_list.len(), "populating thoughts");
            for (rkey, cached) in thoughts_list {
                derived.handle_update(&CacheUpdate::ThoughtCreated {
                    rkey,
                    thought: cached.value,
                });
            }

            // Blog entries
            let blog_list = repo_cache.list_blog_entries();
            info!(count = blog_list.len(), "populating blog entries");
            for (rkey, cached) in blog_list {
                derived.handle_update(&CacheUpdate::BlogEntryCreated {
                    rkey,
                    entry: cached.value,
                });
            }

            // Wiki entries
            let wiki_entries_list = repo_cache.list_wiki_entries();
            info!(count = wiki_entries_list.len(), "populating wiki entries");
            for (rkey, cached) in wiki_entries_list {
                derived.handle_update(&CacheUpdate::WikiEntryCreated {
                    rkey,
                    entry: cached.value,
                });
            }

            // Wiki links
            let wiki_links_list = repo_cache.list_wiki_links();
            info!(count = wiki_links_list.len(), "populating wiki links");
            for (rkey, cached) in wiki_links_list {
                derived.handle_update(&CacheUpdate::WikiLinkCreated {
                    rkey,
                    link: cached.value,
                });
            }

            // Triggers
            let triggers_list = repo_cache.list_triggers();
            info!(count = triggers_list.len(), "populating triggers");
            for (rkey, cached) in triggers_list {
                derived.handle_update(&CacheUpdate::TriggerCreated {
                    rkey,
                    trigger: cached.value,
                });
            }

            // Followers from daemon state (stored in PDS so MCP can access via CAR)
            let followers = repo_cache.get_followers().await;
            if !followers.is_empty() {
                info!(count = followers.len(), "populating followers from state");
                let followers_set: std::collections::HashSet<String> =
                    followers.into_iter().collect();
                derived.set_followers(followers_set);
            }

            // Mark all derived predicates as needing regeneration
            derived.mark_all_dirty();
            info!("all derived facts populated and marked dirty");
        }

        // Populate fact declarations (both by rkey and by predicate)
        {
            let declarations_list = repo_cache.list_declarations();
            info!(
                count = declarations_list.len(),
                "populating fact declarations"
            );
            let mut decls = self.declarations.write().await;
            let mut decls_by_pred = self.declarations_by_predicate.write().await;
            for (rkey, cached) in declarations_list {
                decls_by_pred.insert(cached.value.predicate.clone(), cached.value.clone());
                decls.insert(rkey, cached.value);
            }
        }

        // Mark everything as needing regeneration (lazy mode)
        *self.full_regen_needed.write().await = true;
        // Clear fresh predicates - all are stale
        self.fresh_predicates.write().await.clear();
        self.facts_generation.fetch_add(1, Ordering::SeqCst);
        self.rules_generation.fetch_add(1, Ordering::SeqCst);

        // Log derived fact counts for diagnostics
        let facts_count = self.facts_by_rkey.read().await.len();
        let rules_count = self.rules.read().await.len();
        let stats = {
            let derived = self.derived.read().await;
            derived.stats()
        };
        info!(
            facts = facts_count,
            rules = rules_count,
            notes = stats.notes,
            thoughts = stats.thoughts,
            blog_entries = stats.blog_entries,
            follows = stats.follows,
            likes = stats.likes,
            directives = stats.directives,
            "datalog cache populated (lazy regen enabled - TSV files generated on-demand)"
        );

        // With lazy regeneration, we don't flush all TSV files upfront.
        // They will be generated on-demand when queries need them.
        // Just call flush_dirty_predicates to transition to lazy mode.
        if let Err(e) = self.flush_dirty_predicates().await {
            warn!(error = %e, "failed to initialize lazy regen mode after population");
        }
    }

    /// Handle a cache update event.
    pub async fn handle_update(&self, update: CacheUpdate) -> Result<(), DatalogError> {
        match update {
            CacheUpdate::FactCreated { rkey, fact } => {
                self.add_fact(rkey, fact, String::new()).await;
            }
            CacheUpdate::FactUpdated { rkey, fact } => {
                self.add_fact(rkey, fact, String::new()).await;
            }
            CacheUpdate::FactDeleted { rkey } => {
                self.remove_fact(&rkey).await;
            }
            CacheUpdate::RuleCreated { rkey, rule } => {
                self.add_rule(rkey, rule).await;
            }
            CacheUpdate::RuleUpdated { rkey, rule } => {
                self.update_rule(&rkey, rule).await;
            }
            CacheUpdate::RuleDeleted { rkey } => {
                self.remove_rule(&rkey).await;
            }
            CacheUpdate::Synchronized => {
                debug!("repo cache synchronized");
            }
            // Non-datalog records (identity only)
            CacheUpdate::IdentityUpdated { .. } => {
                // Ignored - this doesn't affect datalog queries
            }
            // Notes, thoughts, blog entries - forward to DerivedFactGenerator
            ref update @ CacheUpdate::NoteCreated { .. }
            | ref update @ CacheUpdate::NoteUpdated { .. }
            | ref update @ CacheUpdate::NoteDeleted { .. }
            | ref update @ CacheUpdate::ThoughtCreated { .. }
            | ref update @ CacheUpdate::ThoughtDeleted { .. }
            | ref update @ CacheUpdate::BlogEntryCreated { .. }
            | ref update @ CacheUpdate::BlogEntryUpdated { .. }
            | ref update @ CacheUpdate::BlogEntryDeleted { .. }
            | ref update @ CacheUpdate::WikiEntryCreated { .. }
            | ref update @ CacheUpdate::WikiEntryUpdated { .. }
            | ref update @ CacheUpdate::WikiEntryDeleted { .. }
            | ref update @ CacheUpdate::WikiLinkCreated { .. }
            | ref update @ CacheUpdate::WikiLinkDeleted { .. } => {
                // Forward to DerivedFactGenerator
                let mut derived = self.derived.write().await;
                derived.handle_update(update);
            }
            // Bluesky records - forward to DerivedFactGenerator
            ref update @ CacheUpdate::FollowCreated { .. }
            | ref update @ CacheUpdate::FollowDeleted { .. }
            | ref update @ CacheUpdate::LikeCreated { .. }
            | ref update @ CacheUpdate::LikeDeleted { .. }
            | ref update @ CacheUpdate::RepostCreated { .. }
            | ref update @ CacheUpdate::RepostDeleted { .. }
            | ref update @ CacheUpdate::PostCreated { .. }
            | ref update @ CacheUpdate::PostUpdated { .. }
            | ref update @ CacheUpdate::PostDeleted { .. }
            // Winter records - forward to DerivedFactGenerator
            | ref update @ CacheUpdate::DirectiveCreated { .. }
            | ref update @ CacheUpdate::DirectiveUpdated { .. }
            | ref update @ CacheUpdate::DirectiveDeleted { .. }
            | ref update @ CacheUpdate::ToolCreated { .. }
            | ref update @ CacheUpdate::ToolUpdated { .. }
            | ref update @ CacheUpdate::ToolDeleted { .. }
            | ref update @ CacheUpdate::ToolApprovalCreated { .. }
            | ref update @ CacheUpdate::ToolApprovalUpdated { .. }
            | ref update @ CacheUpdate::ToolApprovalDeleted { .. }
            | ref update @ CacheUpdate::JobCreated { .. }
            | ref update @ CacheUpdate::JobUpdated { .. }
            | ref update @ CacheUpdate::JobDeleted { .. }
            // Trigger records
            | ref update @ CacheUpdate::TriggerCreated { .. }
            | ref update @ CacheUpdate::TriggerUpdated { .. }
            | ref update @ CacheUpdate::TriggerDeleted { .. } => {
                // Forward to DerivedFactGenerator
                let mut derived = self.derived.write().await;
                derived.handle_update(update);
            }
            // Declaration records - store for query-time .decl generation
            CacheUpdate::DeclarationCreated { rkey, declaration }
            | CacheUpdate::DeclarationUpdated { rkey, declaration } => {
                // Update both indexes
                {
                    let mut decls = self.declarations.write().await;
                    let mut decls_by_pred = self.declarations_by_predicate.write().await;
                    decls_by_pred.insert(declaration.predicate.clone(), declaration.clone());
                    decls.insert(rkey, declaration);
                }
                // Mark for full regen since validation rules may have changed
                *self.full_regen_needed.write().await = true;
            }
            CacheUpdate::DeclarationDeleted { rkey } => {
                // Remove from both indexes
                {
                    let mut decls = self.declarations.write().await;
                    let mut decls_by_pred = self.declarations_by_predicate.write().await;
                    if let Some(removed) = decls.remove(&rkey) {
                        decls_by_pred.remove(&removed.predicate);
                    }
                }
                // Mark for full regen since validation rules may have changed
                *self.full_regen_needed.write().await = true;
            }
            // State updates - extract followers for is_followed_by predicate
            CacheUpdate::StateUpdated { state } => {
                let followers_set: std::collections::HashSet<String> =
                    state.followers.into_iter().collect();
                let mut derived = self.derived.write().await;
                derived.set_followers(followers_set);
            }
        }
        Ok(())
    }

    /// Add or update a fact.
    async fn add_fact(&self, rkey: String, fact: Fact, cid: String) {
        let predicate = fact.predicate.clone();
        let arity = fact.args.len();

        // Check if this fact supersedes another
        if let Some(ref supersedes_cid) = fact.supersedes {
            let mut superseded = self.superseded_cids.write().await;
            superseded.insert(supersedes_cid.clone());

            // Mark the superseded fact
            let cid_to_rkey = self.cid_to_rkey.read().await;
            if let Some(old_rkey) = cid_to_rkey.get(supersedes_cid) {
                let mut facts = self.facts_by_rkey.write().await;
                if let Some(old_fact) = facts.get_mut(old_rkey) {
                    old_fact.is_superseded = true;
                }
            }
        }

        // Track arity
        {
            let mut arities = self.predicate_arities.write().await;
            arities.entry(predicate.clone()).or_insert(arity);
        }

        // Track CID -> rkey
        if !cid.is_empty() {
            let mut cid_map = self.cid_to_rkey.write().await;
            cid_map.insert(cid.clone(), rkey.clone());
        }

        // Check if superseded
        let is_superseded = {
            let superseded = self.superseded_cids.read().await;
            superseded.contains(&cid)
        };

        // Insert fact
        {
            let mut facts = self.facts_by_rkey.write().await;
            facts.insert(
                rkey,
                CachedFactData {
                    fact,
                    cid,
                    is_superseded,
                },
            );
        }

        // Mark predicate as dirty
        {
            let mut dirty = self.dirty_predicates.write().await;
            dirty.insert(predicate);
        }

        self.facts_generation.fetch_add(1, Ordering::SeqCst);
        trace!("fact added, generation bumped");
    }

    /// Remove a fact.
    async fn remove_fact(&self, rkey: &str) {
        let predicate = {
            let mut facts = self.facts_by_rkey.write().await;
            if let Some(removed) = facts.remove(rkey) {
                // Remove from CID map
                let mut cid_map = self.cid_to_rkey.write().await;
                cid_map.remove(&removed.cid);
                Some(removed.fact.predicate)
            } else {
                None
            }
        };

        if let Some(pred) = predicate {
            let mut dirty = self.dirty_predicates.write().await;
            dirty.insert(pred);
            self.facts_generation.fetch_add(1, Ordering::SeqCst);
            trace!(rkey, "fact removed, generation bumped");
        }
    }

    /// Add a rule.
    async fn add_rule(&self, rkey: String, rule: Rule) {
        let mut rules = self.rules.write().await;
        rules.insert(rkey, rule);
        drop(rules);

        self.rules_generation.fetch_add(1, Ordering::SeqCst);
        trace!("rule added");
    }

    /// Update a rule by rkey.
    async fn update_rule(&self, rkey: &str, rule: Rule) {
        let mut rules = self.rules.write().await;
        rules.insert(rkey.to_string(), rule);
        drop(rules);

        self.rules_generation.fetch_add(1, Ordering::SeqCst);
        trace!(rkey = %rkey, "rule updated");
    }

    /// Remove a rule by rkey.
    async fn remove_rule(&self, rkey: &str) {
        let mut rules = self.rules.write().await;
        rules.remove(rkey);
        drop(rules);

        self.rules_generation.fetch_add(1, Ordering::SeqCst);
        trace!(rkey = %rkey, "rule removed");
    }

    /// Execute a query using the cache.
    ///
    /// This will:
    /// 1. Flush any dirty predicates (regenerate changed TSV files)
    /// 2. Get or generate the base program
    /// 3. Append extra facts, rules, and query
    /// 4. Execute with Soufflé
    ///
    /// The `extra_facts` parameter allows injecting ephemeral facts at query time
    /// without persisting them. Useful for runtime context like thread state.
    pub async fn execute_query(
        &self,
        query: &str,
        extra_rules: Option<&str>,
    ) -> Result<Vec<Vec<String>>, DatalogError> {
        self.execute_query_with_facts(query, extra_rules, None)
            .await
    }

    /// Execute a query with optional ephemeral facts.
    ///
    /// Like `execute_query`, but also accepts `extra_facts` - inline facts that
    /// are included in the query but not persisted to the PDS.
    pub async fn execute_query_with_facts(
        &self,
        query: &str,
        extra_rules: Option<&str>,
        extra_facts: Option<&[String]>,
    ) -> Result<Vec<Vec<String>>, DatalogError> {
        self.execute_query_with_facts_and_declarations(query, extra_rules, extra_facts, None)
            .await
    }

    /// Execute a query with optional ephemeral facts and ad-hoc declarations.
    ///
    /// Like `execute_query_with_facts`, but also accepts `extra_declarations` -
    /// ad-hoc predicate declarations (e.g., "my_pred(arg1: symbol, arg2: symbol)")
    /// for predicates not yet stored.
    ///
    /// Uses lazy regeneration: only generates TSV files for predicates actually
    /// needed by the query.
    pub async fn execute_query_with_facts_and_declarations(
        &self,
        query: &str,
        extra_rules: Option<&str>,
        extra_facts: Option<&[String]>,
        extra_declarations: Option<&[String]>,
    ) -> Result<Vec<Vec<String>>, DatalogError> {
        // Flush dirty predicates (marks stale, doesn't regenerate)
        self.flush_dirty_predicates().await?;

        // Build dependency graph from stored rules
        let rules_guard = self.rules.read().await;
        let rules_vec: Vec<Rule> = rules_guard.values().cloned().collect();
        drop(rules_guard);
        let dep_graph = PredicateDependencyGraph::from_rules(&rules_vec);

        // Extract predicates from query
        let mut root_predicates = PredicateDependencyGraph::extract_query_predicates(query);

        // Extract predicates from extra_rules
        if let Some(extra) = extra_rules {
            root_predicates.extend(PredicateDependencyGraph::extract_query_predicates(extra));
        }

        // Extract predicates from extra_facts
        if let Some(facts) = extra_facts {
            for fact in facts {
                root_predicates.extend(PredicateDependencyGraph::extract_query_predicates(fact));
            }
        }

        // Get transitive closure of required predicates
        let required_predicates = dep_graph.get_required_predicates(&root_predicates);

        debug!(
            query = %query,
            root_predicates = root_predicates.len(),
            required_predicates = required_predicates.len(),
            "lazy regen: computed required predicates"
        );

        // Include ALL derived predicates to ensure their TSV files exist
        // This prevents missing .decl errors when derived predicates are used in rule bodies
        let mut predicates_to_ensure = required_predicates.clone();
        for (pred, _) in DerivedFactGenerator::arities() {
            predicates_to_ensure.insert(pred.to_string());
        }
        self.ensure_predicates_exist(&predicates_to_ensure).await?;

        // Parse extra_rules for explicit .decl statements BEFORE generating program
        // This prevents duplicate declarations when stored rules define predicates
        // that are also declared in extra_rules
        let mut user_declared: HashSet<String> =
            extra_rules.map(parse_decl_statements).unwrap_or_default();

        // Build predicate type map from all declaration sources (first-write-wins)
        let mut predicate_types: HashMap<String, Vec<String>> = HashMap::new();

        // 1. Stored declarations from PDS (highest priority — explicit user schemas)
        {
            let stored_decls = self.declarations_by_predicate.read().await;
            for (pred_name, decl) in stored_decls.iter() {
                let mut types: Vec<String> =
                    decl.args.iter().map(|a| a.r#type.clone()).collect();
                types.push("symbol".to_string()); // rkey is always symbol
                predicate_types.insert(pred_name.clone(), types);
            }
        }

        // 1b. Stored rule head type annotations (lower priority than FactDeclarations)
        {
            let rules = self.rules.read().await;
            for rule in rules.values() {
                if !rule.enabled || rule.args.is_empty() {
                    continue;
                }
                if let Some(head_pred) = extract_rule_head_predicate(&rule.head) {
                    // Rule-derived predicates do NOT have rkey appended,
                    // so we use the args types directly without adding a trailing symbol.
                    let types: Vec<String> =
                        rule.args.iter().map(|a| a.r#type.clone()).collect();
                    predicate_types.entry(head_pred).or_insert(types);
                }
            }
        }

        // 2. extra_declarations parameter
        if let Some(decls) = extra_declarations {
            for decl_str in decls {
                if let Some((name, types)) = parse_declaration_arg_types(decl_str) {
                    predicate_types.entry(name).or_insert(types);
                }
            }
        }

        // 3. .decl lines from extra_rules
        if let Some(extra) = extra_rules {
            for line in extra.lines() {
                let line = line.trim();
                if line.starts_with(".decl ") {
                    if let Some((name, types)) = parse_declaration_arg_types(line) {
                        predicate_types.entry(name).or_insert(types);
                    }
                }
            }
        }

        // Generate program for the required predicates
        let (base_program, declared_predicates) = self
            .generate_program_for_predicates(
                &required_predicates,
                &user_declared,
                &predicate_types,
            )
            .await?;

        // Build full program
        let mut program = base_program;

        // Track predicates declared by ad-hoc rules/facts
        let mut all_declared = declared_predicates;

        // Add stored declarations from PDS (fact declarations created via MCP tools)
        // Only include declarations for predicates that are actually needed by the query
        {
            let stored_decls = self.declarations.read().await;
            if !stored_decls.is_empty() {
                let mut added_any = false;
                for (_, decl) in stored_decls.iter() {
                    // Skip if not needed by this query
                    if !required_predicates.contains(&decl.predicate) {
                        continue;
                    }
                    // Skip if already declared
                    if all_declared.contains(&decl.predicate)
                        || user_declared.contains(&decl.predicate)
                    {
                        continue;
                    }
                    if !added_any {
                        program.push_str("// Stored fact declarations\n");
                        added_any = true;
                    }
                    // Generate .decl statement from FactDeclaration
                    let args: Vec<String> = decl
                        .args
                        .iter()
                        .map(|a| format!("{}: {}", a.name, a.r#type))
                        .collect();
                    program.push_str(&format!(".decl {}({})\n", decl.predicate, args.join(", ")));
                    all_declared.insert(decl.predicate.clone());
                }
                if added_any {
                    program.push('\n');
                }
            }
        }

        // Process extra_declarations parameter (ad-hoc declarations like "my_pred(arg1: symbol)")
        if let Some(decls) = extra_declarations {
            program.push_str("// Ad-hoc declarations\n");
            for decl in decls {
                let decl = decl.trim();
                // Parse predicate name from declaration
                if let Some(paren_idx) = decl.find('(') {
                    let pred_name = decl[..paren_idx].trim();
                    if !all_declared.contains(pred_name) && !user_declared.contains(pred_name) {
                        program.push_str(&format!(".decl {}\n", decl));
                        all_declared.insert(pred_name.to_string());
                        user_declared.insert(pred_name.to_string());
                    }
                }
            }
            program.push('\n');
        }

        // Add user's explicit declarations FIRST (from extra_rules)
        // This ensures typed declarations appear before inline facts that use them
        if let Some(extra) = extra_rules {
            // Extract and add only the .decl statements first
            for line in extra.lines() {
                let line_trimmed = line.trim();
                if line_trimmed.starts_with(".decl ") {
                    program.push_str(line);
                    program.push('\n');
                }
            }
        }

        // Generate declarations and assertions for extra_facts
        if let Some(facts) = extra_facts
            && !facts.is_empty()
        {
            // Parse facts to extract predicates and arities
            let parsed_facts = parse_extra_facts(facts);

            // Auto-declare any new predicates (skip if user declared explicitly)
            for (name, arity) in &parsed_facts {
                if !all_declared.contains(name) && !user_declared.contains(name) {
                    let params: Vec<String> = if let Some(types) = predicate_types.get(name) {
                        types
                            .iter()
                            .enumerate()
                            .map(|(i, t)| format!("arg{}: {}", i, t))
                            .collect()
                    } else {
                        (0..*arity).map(|i| format!("arg{}: symbol", i)).collect()
                    };
                    program.push_str(&format!(".decl {}({})\n", name, params.join(", ")));
                    all_declared.insert(name.clone());
                }
            }

            // Add the facts as inline assertions
            program.push_str("// Ephemeral facts\n");
            for fact in facts {
                // Ensure fact ends with a period
                let fact = fact.trim();
                if fact.ends_with('.') {
                    program.push_str(fact);
                } else {
                    program.push_str(fact);
                    program.push('.');
                }
                program.push('\n');
            }
            program.push('\n');
        }

        // Generate declarations for ad-hoc rule heads
        if let Some(extra) = extra_rules {
            let heads = RuleCompiler::parse_extra_rules_heads(extra);
            for (name, arity) in heads {
                if !all_declared.contains(&name) && !user_declared.contains(&name) {
                    let params: Vec<String> = if let Some(types) = predicate_types.get(&name) {
                        types
                            .iter()
                            .enumerate()
                            .map(|(i, t)| format!("arg{}: {}", i, t))
                            .collect()
                    } else {
                        (0..arity).map(|i| format!("arg{}: symbol", i)).collect()
                    };
                    program.push_str(&format!(".decl {}({})\n", name, params.join(", ")));
                    all_declared.insert(name);
                }
            }

            // Add rules (skip .decl lines since we already added them)
            program.push_str("// Ad-hoc rules\n");
            for line in extra.lines() {
                let line_trimmed = line.trim();
                if !line_trimmed.starts_with(".decl ") {
                    program.push_str(line);
                    program.push('\n');
                }
            }
            program.push('\n');
        }

        // Generate wrapper rule that properly handles constants as filters
        let (wrapper, _result_arity) =
            generate_query_wrapper(query, Some(&all_declared), &predicate_types);
        program.push_str(&wrapper);

        // Log program details for debugging derived predicate issues
        if query.contains("has_note") || query.contains("has_thought") || query.contains("has_blog")
        {
            info!(
                query = %query,
                program_len = program.len(),
                program_preview = %program.chars().take(2000).collect::<String>(),
                "executing derived predicate query"
            );
        } else {
            debug!(
                query = %query,
                program_len = program.len(),
                "executing cached query"
            );
        }

        // Execute
        let output = self.executor.execute(&program, &self.fact_dir).await?;

        // Parse results
        let results = SouffleExecutor::parse_output(&output);

        // Log if query returned no results but we expected some (for debugging)
        if results.is_empty() && !output.trim().is_empty() {
            debug!(
                query = %query,
                output_len = output.len(),
                output_preview = %output.chars().take(500).collect::<String>(),
                "query returned no parsed results despite non-empty output"
            );
        }

        Ok(results)
    }

    /// Flush dirty predicates by regenerating their TSV files.
    ///
    /// This method now supports lazy regeneration:
    /// - When `full_regen_needed` is true, it clears `fresh_predicates` and returns
    /// - Actual TSV generation is deferred until `ensure_predicates_exist` is called
    /// - Incremental updates (dirty predicates) are still flushed immediately
    ///
    /// A regeneration lock prevents multiple queries from doing redundant
    /// regeneration work concurrently.
    pub async fn flush_dirty_predicates(&self) -> Result<(), DatalogError> {
        let start = std::time::Instant::now();

        // Acquire regeneration lock to prevent concurrent regenerations.
        let _regen_guard = self.regen_lock.lock().await;
        let lock_acquired = start.elapsed();

        // Check if full regen is needed (another query may have done it while we waited)
        let full_regen = *self.full_regen_needed.read().await;

        if full_regen {
            debug!(
                lock_wait_ms = lock_acquired.as_millis(),
                "lazy regen: clearing fresh predicates, deferring TSV generation"
            );

            // Clear fresh predicates - all predicates are now stale
            self.fresh_predicates.write().await.clear();
            // Clear dirty predicates tracking
            self.dirty_predicates.write().await.clear();
            // Mark full regen as handled (lazy mode enabled)
            *self.full_regen_needed.write().await = false;

            return Ok(());
        }

        // Handle incremental dirty predicates (mark as stale)
        let dirty: HashSet<String> = {
            let mut dirty_guard = self.dirty_predicates.write().await;
            std::mem::take(&mut *dirty_guard)
        };

        if !dirty.is_empty() {
            debug!(predicates = ?dirty, "marking predicates as stale");
            let mut fresh = self.fresh_predicates.write().await;
            for pred in &dirty {
                fresh.remove(pred);
            }
        }

        // Mark dirty derived predicates as stale
        let dirty_derived: Option<HashSet<String>> = {
            let derived = self.derived.read().await;
            if derived.has_dirty_predicates() {
                Some(derived.dirty_predicates_snapshot())
            } else {
                None
            }
        };

        if let Some(dirty_preds) = dirty_derived {
            let mut fresh = self.fresh_predicates.write().await;
            for pred in &dirty_preds {
                fresh.remove(pred);
            }
        }

        Ok(())
    }

    /// Ensure the specified predicates have fresh TSV files.
    ///
    /// This is the core of lazy regeneration - only generates TSV files for
    /// predicates that are actually needed by the query.
    pub async fn ensure_predicates_exist(
        &self,
        predicates: &HashSet<String>,
    ) -> Result<(), DatalogError> {
        let start = std::time::Instant::now();

        // Check which predicates are stale
        let stale: HashSet<String> = {
            let fresh = self.fresh_predicates.read().await;
            predicates
                .iter()
                .filter(|p| !fresh.contains(*p))
                .cloned()
                .collect()
        };

        if stale.is_empty() {
            trace!("all {} predicates are fresh", predicates.len());
            return Ok(());
        }

        // Acquire regeneration lock
        let _regen_guard = self.regen_lock.lock().await;

        // Re-check after acquiring lock (another query may have generated them)
        let stale: HashSet<String> = {
            let fresh = self.fresh_predicates.read().await;
            predicates
                .iter()
                .filter(|p| !fresh.contains(*p))
                .cloned()
                .collect()
        };

        if stale.is_empty() {
            return Ok(());
        }

        debug!(
            predicates = stale.len(),
            stale = ?stale,
            "regenerating stale predicates"
        );

        // Separate user facts, metadata, and derived predicates
        let mut user_predicates = HashSet::new();
        let mut derived_predicates = HashSet::new();
        let mut need_metadata = false;

        for pred in &stale {
            if is_metadata_predicate(pred) {
                need_metadata = true;
            } else if DerivedFactGenerator::is_derived(pred) {
                derived_predicates.insert(pred.clone());
            } else {
                user_predicates.insert(pred.clone());
                // User predicates require metadata
                need_metadata = true;
            }
        }

        // Generate user fact files and metadata if needed
        // Note: If we need _validation_error, we must regenerate ALL user predicates
        // to run validation on all facts
        let user_preds_to_regenerate = if stale.contains("_validation_error") {
            // Need to regenerate all user predicates to collect validation errors
            let arities = self.predicate_arities.read().await;
            arities.keys().cloned().collect()
        } else {
            user_predicates
        };

        if !user_preds_to_regenerate.is_empty() || need_metadata {
            let user_start = std::time::Instant::now();
            self.regenerate_user_predicates(&user_preds_to_regenerate, need_metadata)
                .await?;
            info!(
                elapsed_ms = user_start.elapsed().as_millis(),
                predicate_count = user_preds_to_regenerate.len(),
                include_metadata = need_metadata,
                "regenerated user predicates"
            );
        }

        // Generate derived predicate files
        if !derived_predicates.is_empty() {
            let derived_start = std::time::Instant::now();
            self.regenerate_derived_predicates(&derived_predicates)
                .await?;
            info!(
                elapsed_ms = derived_start.elapsed().as_millis(),
                predicate_count = derived_predicates.len(),
                predicates = ?derived_predicates,
                "regenerated derived predicates"
            );
        }

        // Mark all requested predicates as fresh
        {
            let mut fresh = self.fresh_predicates.write().await;
            fresh.extend(stale);
        }

        // Clear dirty state for derived predicates we regenerated
        if !derived_predicates.is_empty() {
            let mut derived = self.derived.write().await;
            derived.clear_dirty();
        }

        debug!(
            elapsed_ms = start.elapsed().as_millis(),
            predicates = predicates.len(),
            "predicates ensured fresh"
        );

        Ok(())
    }

    /// Regenerate TSV files for user-defined fact predicates.
    async fn regenerate_user_predicates(
        &self,
        predicates: &HashSet<String>,
        include_metadata: bool,
    ) -> Result<(), DatalogError> {
        // Collect snapshots
        let (facts_snapshot, arities_snapshot, decls_snapshot, cid_map_snapshot) = {
            let facts = self.facts_by_rkey.read().await;
            let arities = self.predicate_arities.read().await;
            let decls_by_pred = self.declarations_by_predicate.read().await;
            let cid_to_rkey = self.cid_to_rkey.read().await;
            (
                facts.clone(),
                arities.clone(),
                decls_by_pred.clone(),
                cid_to_rkey.clone(),
            )
        };

        if include_metadata {
            // Write metadata files
            self.write_metadata_files(&facts_snapshot, &cid_map_snapshot)?;
        }

        // Write predicate-specific files
        for predicate in predicates {
            if let Some(&arity) = arities_snapshot.get(predicate) {
                self.regenerate_predicate_files(
                    predicate,
                    arity,
                    &facts_snapshot,
                    &decls_snapshot,
                )?;
            } else {
                // Predicate has no facts - create empty file
                self.create_empty_predicate_file(predicate)?;
            }
        }

        // Mark metadata predicates as fresh
        if include_metadata {
            let mut fresh = self.fresh_predicates.write().await;
            for pred in METADATA_PREDICATES {
                fresh.insert((*pred).to_string());
            }
        }

        Ok(())
    }

    /// Regenerate TSV files for derived predicates.
    async fn regenerate_derived_predicates(
        &self,
        predicates: &HashSet<String>,
    ) -> Result<(), DatalogError> {
        let derived_snapshot = {
            let derived = self.derived.read().await;
            derived.clone_for_flush()
        };

        derived_snapshot.write_predicates_subset(&self.fact_dir, predicates)?;

        Ok(())
    }

    /// Write all metadata files (_fact, _confidence, etc).
    fn write_metadata_files(
        &self,
        facts: &HashMap<String, CachedFactData>,
        cid_map: &HashMap<String, String>,
    ) -> Result<(), DatalogError> {
        let mut fact_file =
            BufWriter::new(std::fs::File::create(self.fact_dir.join("_fact.facts"))?);
        let mut confidence_file = BufWriter::new(std::fs::File::create(
            self.fact_dir.join("_confidence.facts"),
        )?);
        let mut source_file =
            BufWriter::new(std::fs::File::create(self.fact_dir.join("_source.facts"))?);
        let mut supersedes_file = BufWriter::new(std::fs::File::create(
            self.fact_dir.join("_supersedes.facts"),
        )?);
        let mut created_at_file = BufWriter::new(std::fs::File::create(
            self.fact_dir.join("_created_at.facts"),
        )?);
        let mut expires_at_file = BufWriter::new(std::fs::File::create(
            self.fact_dir.join("_expires_at.facts"),
        )?);
        // Create empty validation errors file - errors written per-predicate
        std::fs::File::create(self.fact_dir.join("_validation_error.facts"))?;

        for (rkey, data) in facts.iter() {
            writeln!(fact_file, "{}\t{}\t{}", rkey, data.fact.predicate, data.cid)?;

            if let Some(conf) = data.fact.confidence
                && (conf - 1.0).abs() > f64::EPSILON
            {
                writeln!(confidence_file, "{}\t{}", rkey, conf)?;
            }

            if let Some(ref source) = data.fact.source {
                writeln!(source_file, "{}\t{}", rkey, source)?;
            }

            if let Some(ref supersedes_cid) = data.fact.supersedes
                && let Some(old_rkey) = cid_map.get(supersedes_cid)
            {
                writeln!(supersedes_file, "{}\t{}", rkey, old_rkey)?;
            }

            writeln!(
                created_at_file,
                "{}\t{}",
                rkey,
                data.fact.created_at.to_rfc3339()
            )?;

            if let Some(ref ea) = data.fact.expires_at {
                writeln!(expires_at_file, "{}\t{}", rkey, ea.to_rfc3339())?;
            }
        }

        Ok(())
    }

    /// Create an empty TSV file for a predicate.
    fn create_empty_predicate_file(&self, predicate: &str) -> Result<(), DatalogError> {
        let path = self.fact_dir.join(format!("{}.facts", predicate));
        std::fs::File::create(&path)?;

        // Also create _all_ variant for user predicates
        if !predicate.starts_with('_') && !DerivedFactGenerator::is_derived(predicate) {
            let all_path = self.fact_dir.join(format!("_all_{}.facts", predicate));
            std::fs::File::create(&all_path)?;
        }

        Ok(())
    }

    /// Regenerate TSV files for a single predicate.
    ///
    /// Facts are validated against declarations if one exists for the predicate.
    /// Invalid facts are skipped from TSV output and logged for investigation.
    fn regenerate_predicate_files(
        &self,
        predicate: &str,
        arity: usize,
        facts: &HashMap<String, CachedFactData>,
        declarations_by_predicate: &HashMap<String, FactDeclaration>,
    ) -> Result<(), DatalogError> {
        // Current facts file (non-superseded, non-expired only)
        let current_path = self.fact_dir.join(format!("{}.facts", predicate));
        let mut current_file = BufWriter::new(std::fs::File::create(&current_path)?);

        // All facts file (with rkey prefix)
        let all_path = self.fact_dir.join(format!("_all_{}.facts", predicate));
        let mut all_file = BufWriter::new(std::fs::File::create(&all_path)?);

        // Validation errors file (append mode for incremental predicates)
        let errors_path = self.fact_dir.join("_validation_error.facts");
        let mut errors_file = BufWriter::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&errors_path)?,
        );

        let now = chrono::Utc::now();

        for (rkey, data) in facts.iter() {
            if data.fact.predicate != predicate {
                continue;
            }

            // Validate against declaration if one exists
            if let Some(error) =
                validate_fact_against_declaration(&data.fact, declarations_by_predicate)
            {
                warn!(
                    rkey = %rkey,
                    predicate = %data.fact.predicate,
                    error = %error,
                    "skipping fact due to schema validation failure"
                );
                // Write to validation errors file for investigation
                writeln!(errors_file, "{}\t{}\t{}", rkey, data.fact.predicate, error)?;
                continue; // Skip writing to TSV
            }

            // Escape tabs and newlines in arguments to prevent TSV corruption
            let args: Vec<String> = data
                .fact
                .args
                .iter()
                .map(|a| a.replace(['\t', '\n'], " "))
                .collect();
            let args_str = args.join("\t");

            // Write to all file (always, rkey at end)
            writeln!(all_file, "{}\t{}", args_str, rkey)?;

            // Write to current file (only if not superseded and not expired, rkey at end)
            let is_expired = data.fact.expires_at.map_or(false, |ea| ea <= now);
            if !data.is_superseded && !is_expired {
                writeln!(current_file, "{}\t{}", args_str, rkey)?;
            }
        }

        trace!(predicate, arity, "regenerated predicate files");
        Ok(())
    }

    /// Generate a Soufflé program for the specified predicates.
    ///
    /// Only includes declarations and rules relevant to the required predicates,
    /// enabling lazy regeneration by not requiring all TSV files to exist.
    ///
    /// The `exclude_predicates` set contains predicates that should NOT be declared
    /// (e.g., because the caller will provide their own .decl statements via extra_rules).
    async fn generate_program_for_predicates(
        &self,
        required_predicates: &HashSet<String>,
        exclude_predicates: &HashSet<String>,
        predicate_types: &HashMap<String, Vec<String>>,
    ) -> Result<(String, HashSet<String>), DatalogError> {
        let mut program = String::new();
        let mut declared_predicates = HashSet::new();

        // Generate input declarations for metadata predicates (if needed)
        let needs_metadata = required_predicates
            .iter()
            .any(|p| is_metadata_predicate(p) || !DerivedFactGenerator::is_derived(p));

        if needs_metadata {
            program.push_str(
                ".decl _fact(rkey: symbol, predicate: symbol, cid: symbol)\n\
                 .input _fact\n\n\
                 .decl _confidence(rkey: symbol, value: symbol)\n\
                 .input _confidence\n\n\
                 .decl _source(rkey: symbol, source_cid: symbol)\n\
                 .input _source\n\n\
                 .decl _supersedes(new_rkey: symbol, old_rkey: symbol)\n\
                 .input _supersedes\n\n\
                 .decl _created_at(rkey: symbol, timestamp: symbol)\n\
                 .input _created_at\n\n\
                 .decl _expires_at(rkey: symbol, timestamp: symbol)\n\
                 .input _expires_at\n\n\
                 .decl _now(timestamp: symbol)\n\n\
                 .decl _expired(rkey: symbol)\n\
                 _expired(R) :- _expires_at(R, E), _now(T), E < T.\n\n\
                 .decl _validation_error(rkey: symbol, predicate: symbol, error_msg: symbol)\n\
                 .input _validation_error\n\n",
            );
            for pred in METADATA_PREDICATES {
                declared_predicates.insert((*pred).to_string());
            }
            declared_predicates.insert("_now".to_string());
            declared_predicates.insert("_expired".to_string());
        }

        // Generate input declarations for user fact predicates
        let arities = self.predicate_arities.read().await;
        for (predicate, &arity) in arities.iter() {
            if !required_predicates.contains(predicate) {
                continue;
            }

            // Current predicate (with rkey suffix)
            // Look up types from the predicate type map; fall back to all-symbol
            let params: Vec<String> = if let Some(types) = predicate_types.get(predicate) {
                // Type map already includes rkey as last element
                types
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        if i == types.len() - 1 {
                            format!("rkey: {}", t)
                        } else {
                            format!("arg{}: {}", i, t)
                        }
                    })
                    .collect()
            } else {
                (0..arity)
                    .map(|i| format!("arg{}: symbol", i))
                    .chain(std::iter::once("rkey: symbol".to_string()))
                    .collect()
            };
            program.push_str(&format!(
                ".decl {}({})\n.input {}\n\n",
                predicate,
                params.join(", "),
                predicate
            ));
            declared_predicates.insert(predicate.clone());

            // _all_{predicate} variant (same types as current)
            let all_name = format!("_all_{}", predicate);
            if required_predicates.contains(&all_name) {
                program.push_str(&format!(
                    ".decl {}({})\n.input {}\n\n",
                    all_name,
                    params.join(", "),
                    all_name
                ));
                declared_predicates.insert(all_name);
            }
        }
        drop(arities);

        // Generate input declarations for ALL derived predicates unconditionally
        // This ensures derived predicates used in rule bodies are always declared,
        // even when they're not directly referenced in the query
        for (predicate, arity) in DerivedFactGenerator::arities() {
            if declared_predicates.contains(predicate) || exclude_predicates.contains(predicate) {
                continue;
            }

            let params: Vec<String> = (0..arity).map(|i| format!("arg{}: symbol", i)).collect();
            program.push_str(&format!(
                ".decl {}({})\n.input {}\n\n",
                predicate,
                params.join(", "),
                predicate
            ));
            declared_predicates.insert(predicate.to_string());
        }

        // Generate declarations for rule heads and compile rules
        let rules = self.rules.read().await;

        // Filter rules to only those relevant to required predicates and enabled
        let relevant_rules: Vec<&winter_atproto::Rule> = rules
            .values()
            .filter(|rule| {
                // Skip disabled rules entirely — they should not contribute
                // declarations or compiled output to the program
                if !rule.enabled {
                    return false;
                }
                // Include rule if its head is in required predicates
                if let Some(head_pred) = extract_rule_head_predicate(&rule.head) {
                    required_predicates.contains(&head_pred)
                } else {
                    false
                }
            })
            .collect();

        // Generate declarations for rule heads (using stored type info when available)
        for rule in &relevant_rules {
            if let Some((name, arity)) = extract_rule_head_with_arity(&rule.head)
                && !declared_predicates.contains(&name)
                && !exclude_predicates.contains(&name)
            {
                let params: Vec<String> = if let Some(types) = predicate_types.get(&name) {
                    types
                        .iter()
                        .enumerate()
                        .map(|(i, t)| format!("arg{}: {}", i, t))
                        .collect()
                } else {
                    (0..arity).map(|i| format!("arg{}: symbol", i)).collect()
                };
                program.push_str(&format!(".decl {}({})\n\n", name, params.join(", ")));
                declared_predicates.insert(name);
            }
        }

        // Compile relevant rules
        if !relevant_rules.is_empty() {
            program.push_str("// Rules\n");
            for rule in relevant_rules {
                if !rule.constraints.is_empty() {
                    debug!(
                        rule_name = %rule.name,
                        constraints = ?rule.constraints,
                        "compiling rule with constraints"
                    );
                }
                let compiled = RuleCompiler::compile_single_rule(rule)?;
                program.push_str(&compiled);
                program.push('\n');
            }
        }

        Ok((program, declared_predicates))
    }

    /// Get or generate the base program (declarations + compiled rules).
    ///
    /// Get access to the derived fact generator (for follower sync).
    pub async fn derived(&self) -> tokio::sync::RwLockReadGuard<'_, DerivedFactGenerator> {
        self.derived.read().await
    }

    /// Get mutable access to the derived fact generator (for follower sync).
    pub async fn derived_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, DerivedFactGenerator> {
        self.derived.write().await
    }

    /// Update the followers set from an external sync.
    ///
    /// This is called by the daemon after fetching followers from the Bluesky API.
    pub async fn set_followers(&self, followers: HashSet<String>) {
        let mut derived = self.derived.write().await;
        derived.set_followers(followers);
    }

    /// Add a single follower (from Follow notification).
    ///
    /// Returns true if this was a new follower.
    pub async fn add_follower(&self, did: String) -> bool {
        let mut derived = self.derived.write().await;
        derived.add_follower(did)
    }

    /// Get the current facts generation counter.
    pub fn facts_generation(&self) -> u64 {
        self.facts_generation.load(Ordering::SeqCst)
    }

    /// Get the current rules generation counter.
    pub fn rules_generation(&self) -> u64 {
        self.rules_generation.load(Ordering::SeqCst)
    }

    /// Get the number of cached facts.
    pub async fn fact_count(&self) -> usize {
        self.facts_by_rkey.read().await.len()
    }

    /// Get the number of cached rules.
    pub async fn rule_count(&self) -> usize {
        self.rules.read().await.len()
    }
}

/// Generate input declarations from a map of predicate arities.
///
/// Returns a tuple of (declarations string, set of declared predicate names).
pub fn generate_input_declarations_from_arities(
    arities: &HashMap<String, usize>,
) -> (String, HashSet<String>) {
    let mut declarations = String::new();
    let mut declared_set = HashSet::new();

    // Metadata relations (always generated)
    declarations.push_str(
        ".decl _fact(rkey: symbol, predicate: symbol, cid: symbol)\n\
         .input _fact\n\n\
         .decl _confidence(rkey: symbol, value: symbol)\n\
         .input _confidence\n\n\
         .decl _source(rkey: symbol, source_cid: symbol)\n\
         .input _source\n\n\
         .decl _supersedes(new_rkey: symbol, old_rkey: symbol)\n\
         .input _supersedes\n\n\
         .decl _created_at(rkey: symbol, timestamp: symbol)\n\
         .input _created_at\n\n\
         .decl _expires_at(rkey: symbol, timestamp: symbol)\n\
         .input _expires_at\n\n\
         .decl _now(timestamp: symbol)\n\n\
         .decl _expired(rkey: symbol)\n\
         _expired(R) :- _expires_at(R, E), _now(T), E < T.\n\n\
         .decl _validation_error(rkey: symbol, predicate: symbol, error_msg: symbol)\n\
         .input _validation_error\n\n",
    );
    declared_set.insert("_fact".to_string());
    declared_set.insert("_confidence".to_string());
    declared_set.insert("_source".to_string());
    declared_set.insert("_supersedes".to_string());
    declared_set.insert("_created_at".to_string());
    declared_set.insert("_expires_at".to_string());
    declared_set.insert("_now".to_string());
    declared_set.insert("_expired".to_string());
    declared_set.insert("_validation_error".to_string());

    // User predicates (current facts only) and _all_{predicate} (all facts with rkey at end)
    for (predicate, &arity) in arities {
        // Current predicate (with rkey suffix)
        let params: Vec<String> = (0..arity)
            .map(|i| format!("arg{}: symbol", i))
            .chain(std::iter::once("rkey: symbol".to_string()))
            .collect();
        declarations.push_str(&format!(
            ".decl {}({})\n.input {}\n\n",
            predicate,
            params.join(", "),
            predicate
        ));
        declared_set.insert(predicate.clone());

        // _all_{predicate} (with rkey at end, same format as current)
        let all_params: Vec<String> = (0..arity)
            .map(|i| format!("arg{}: symbol", i))
            .chain(std::iter::once("rkey: symbol".to_string()))
            .collect();
        let all_name = format!("_all_{}", predicate);
        declarations.push_str(&format!(
            ".decl {}({})\n.input {}\n\n",
            all_name,
            all_params.join(", "),
            all_name
        ));
        declared_set.insert(all_name);
    }

    (declarations, declared_set)
}

/// Represents a query argument - either a variable or a constant.
#[derive(Debug, Clone, PartialEq)]
enum QueryArg {
    Variable(String),
    Constant(String),
}

/// Parsed query with predicate name and arguments.
#[derive(Debug)]
struct ParsedQuery {
    name: String,
    args: Vec<QueryArg>,
}

impl ParsedQuery {
    /// Get the variables in this query (for use in result predicate).
    /// Excludes anonymous variables (`_`) since they can't appear in rule heads.
    fn variables(&self) -> Vec<&str> {
        self.args
            .iter()
            .filter_map(|arg| match arg {
                QueryArg::Variable(v) if v != "_" => Some(v.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get the arity (number of arguments).
    fn arity(&self) -> usize {
        self.args.len()
    }
}

/// Parse a query to extract predicate name and typed arguments.
fn parse_query(query: &str) -> Option<ParsedQuery> {
    let paren_idx = query.find('(')?;
    let name = query[..paren_idx].trim().to_string();

    let close_paren = query.rfind(')')?;
    let args_str = &query[paren_idx + 1..close_paren];

    if args_str.trim().is_empty() {
        return Some(ParsedQuery { name, args: vec![] });
    }

    // Parse arguments, handling quoted strings and nested parens
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut depth = 0;

    for c in args_str.chars() {
        match c {
            '"' if depth == 0 => {
                in_string = !in_string;
                current.push(c);
            }
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if !in_string && depth == 0 => {
                let arg = current.trim().to_string();
                if !arg.is_empty() {
                    args.push(parse_single_arg(&arg));
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }

    // Don't forget the last argument
    let arg = current.trim().to_string();
    if !arg.is_empty() {
        args.push(parse_single_arg(&arg));
    }

    Some(ParsedQuery { name, args })
}

/// Parse a single argument to determine if it's a variable or constant.
fn parse_single_arg(arg: &str) -> QueryArg {
    let arg = arg.trim();
    if arg.starts_with('"') && arg.ends_with('"') {
        // String constant
        QueryArg::Constant(arg.to_string())
    } else if arg
        .chars()
        .next()
        .map(|c| c.is_uppercase() || c == '_')
        .unwrap_or(false)
    {
        // Starts with uppercase or underscore = variable in Datalog convention
        QueryArg::Variable(arg.to_string())
    } else {
        // Numeric constant or other literal
        QueryArg::Constant(arg.to_string())
    }
}

/// Parse a declaration string like `"my_pred(x: number, y: symbol)"` into
/// `("my_pred", vec!["number", "symbol"])`.
///
/// Handles bare argument names (defaults to "symbol") and the `name: type` format.
/// Returns `None` if the string doesn't look like a declaration.
fn parse_declaration_arg_types(decl: &str) -> Option<(String, Vec<String>)> {
    let decl = decl.trim().strip_prefix(".decl ").unwrap_or(decl);
    let paren_idx = decl.find('(')?;
    let close_paren = decl.rfind(')')?;
    let name = decl[..paren_idx].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let args_str = &decl[paren_idx + 1..close_paren];
    if args_str.trim().is_empty() {
        return Some((name, vec![]));
    }
    let types = args_str
        .split(',')
        .map(|arg| {
            let arg = arg.trim();
            if let Some(colon_idx) = arg.find(':') {
                arg[colon_idx + 1..].trim().to_string()
            } else {
                "symbol".to_string()
            }
        })
        .collect();
    Some((name, types))
}

/// Parse `.decl` statements from extra_rules to find user-declared predicates.
///
/// This allows us to skip auto-declaring predicates that the user explicitly declared,
/// avoiding conflicts when parameter names differ.
fn parse_decl_statements(rules: &str) -> HashSet<String> {
    let mut declared = HashSet::new();
    for line in rules.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(".decl ") {
            // Extract predicate name: ".decl predicate_name(..."
            if let Some(paren_idx) = rest.find('(') {
                let name = rest[..paren_idx].trim();
                if !name.is_empty() {
                    declared.insert(name.to_string());
                }
            }
        }
    }
    declared
}

/// Parse extra facts to extract predicate names and arities.
///
/// Each fact should be in the form `predicate(arg1, arg2, ...)` with or without trailing period.
/// Returns a list of (predicate_name, arity) pairs.
fn parse_extra_facts(facts: &[String]) -> Vec<(String, usize)> {
    let mut result = Vec::new();

    for fact in facts {
        let fact = fact.trim().trim_end_matches('.');
        if let Some(paren_idx) = fact.find('(') {
            let name = fact[..paren_idx].trim().to_string();
            if let Some(close_paren) = fact.rfind(')') {
                let args_str = &fact[paren_idx + 1..close_paren];
                // Count arguments by counting commas (handling quoted strings)
                let arity = if args_str.trim().is_empty() {
                    0
                } else {
                    count_args(args_str)
                };
                result.push((name, arity));
            }
        }
    }

    result
}

/// Count the number of arguments in an argument string, handling quoted strings.
fn count_args(args_str: &str) -> usize {
    let mut count = 1; // At least one arg if non-empty
    let mut in_string = false;
    let mut depth = 0;

    for c in args_str.chars() {
        match c {
            '"' if depth == 0 => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth -= 1,
            ',' if !in_string && depth == 0 => count += 1,
            _ => {}
        }
    }

    count
}

/// Generate a wrapper rule and output declaration for a query.
/// This ensures constants in the query are properly used as filters.
///
/// The `predicate_types` map provides per-predicate argument types so that
/// `_query_result` columns and fallback base declarations use the correct
/// Soufflé types (e.g. `number`) instead of always defaulting to `symbol`.
fn generate_query_wrapper(
    query: &str,
    declared_predicates: Option<&HashSet<String>>,
    predicate_types: &HashMap<String, Vec<String>>,
) -> (String, usize) {
    let parsed = match parse_query(query) {
        Some(p) => p,
        None => {
            // Fallback: treat as nullary predicate
            return (
                format!(
                    ".decl _query_result()\n.output _query_result\n_query_result() :- {}.\n",
                    query
                ),
                0,
            );
        }
    };

    let variables = parsed.variables();
    let result_arity = if variables.is_empty() {
        // No named variables - count only constants (anonymous _ are filtered out)
        parsed
            .args
            .iter()
            .filter(|a| matches!(a, QueryArg::Constant(_)))
            .count()
    } else {
        variables.len()
    };

    // Look up the source predicate types to determine result column types.
    // Map each result column back to its position in the source predicate.
    let source_types = predicate_types.get(&parsed.name);

    let result_column_types: Vec<String> = if variables.is_empty() {
        // All constants/anonymous — result columns are the constants in order
        parsed
            .args
            .iter()
            .enumerate()
            .filter_map(|(pos, a)| match a {
                QueryArg::Constant(_) => {
                    let t = source_types
                        .and_then(|ts| ts.get(pos))
                        .map(|s| s.as_str())
                        .unwrap_or("symbol");
                    Some(t.to_string())
                }
                QueryArg::Variable(v) if v != "_" => {
                    let t = source_types
                        .and_then(|ts| ts.get(pos))
                        .map(|s| s.as_str())
                        .unwrap_or("symbol");
                    Some(t.to_string())
                }
                QueryArg::Variable(_) => None,
            })
            .collect()
    } else {
        // Named variables — map each variable back to its position in the args list
        parsed
            .args
            .iter()
            .enumerate()
            .filter_map(|(pos, a)| match a {
                QueryArg::Variable(v) if v != "_" => {
                    let t = source_types
                        .and_then(|ts| ts.get(pos))
                        .map(|s| s.as_str())
                        .unwrap_or("symbol");
                    Some(t.to_string())
                }
                _ => None,
            })
            .collect()
    };

    // Build the result predicate declaration
    let decl = if result_arity > 0 {
        let params: Vec<String> = result_column_types
            .iter()
            .enumerate()
            .map(|(i, t)| format!("arg{}: {}", i, t))
            .collect();
        format!(".decl _query_result({})\n", params.join(", "))
    } else {
        ".decl _query_result()\n".to_string()
    };

    // Build the wrapper rule head
    let head_args = if variables.is_empty() {
        // All constants (or all anonymous variables) - include constants in output
        parsed
            .args
            .iter()
            .filter_map(|a| match a {
                QueryArg::Constant(c) => Some(c.as_str()),
                QueryArg::Variable(v) if v != "_" => Some(v.as_str()),
                QueryArg::Variable(_) => None, // Skip anonymous variables
            })
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        variables.join(", ")
    };

    let head = if head_args.is_empty() {
        "_query_result()".to_string()
    } else {
        format!("_query_result({})", head_args)
    };

    // Check if the base predicate needs declaration
    let base_decl = if let Some(declared) = declared_predicates {
        if !declared.contains(&parsed.name) && parsed.arity() > 0 {
            let params: Vec<String> = if let Some(types) = source_types {
                types
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("arg{}: {}", i, t))
                    .collect()
            } else {
                (0..parsed.arity())
                    .map(|i| format!("arg{}: symbol", i))
                    .collect()
            };
            format!(".decl {}({})\n", parsed.name, params.join(", "))
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let wrapper = format!(
        "{}{}.output _query_result\n{} :- {}.\n",
        base_decl, decl, head, query
    );

    (wrapper, result_arity)
}

/// Extract the predicate name from a rule head.
fn extract_rule_head_predicate(head: &str) -> Option<String> {
    let paren_idx = head.find('(')?;
    let name = head[..paren_idx].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Extract predicate name and arity from a rule head.
fn extract_rule_head_with_arity(head: &str) -> Option<(String, usize)> {
    RuleCompiler::parse_head(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_fact(predicate: &str, args: Vec<&str>) -> Fact {
        Fact {
            predicate: predicate.to_string(),
            args: args.into_iter().map(String::from).collect(),
            confidence: None,
            source: None,
            supersedes: None,
            tags: vec![],
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    #[test]
    fn test_parse_query() {
        let parsed = parse_query("mutual_follow(X, Y)").unwrap();
        assert_eq!(parsed.name, "mutual_follow");
        assert_eq!(parsed.arity(), 2);
        assert_eq!(parsed.variables(), vec!["X", "Y"]);

        let parsed = parse_query("is_active(X)").unwrap();
        assert_eq!(parsed.name, "is_active");
        assert_eq!(parsed.arity(), 1);

        let parsed = parse_query("has_data()").unwrap();
        assert_eq!(parsed.name, "has_data");
        assert_eq!(parsed.arity(), 0);
    }

    #[test]
    fn test_parse_query_with_constants() {
        let parsed = parse_query(r#"should_engage("did:plc:abc")"#).unwrap();
        assert_eq!(parsed.name, "should_engage");
        assert_eq!(parsed.arity(), 1);
        assert!(parsed.variables().is_empty());
        assert_eq!(
            parsed.args,
            vec![QueryArg::Constant(r#""did:plc:abc""#.to_string())]
        );
    }

    #[test]
    fn test_generate_query_wrapper_with_constant() {
        let empty_types = HashMap::new();
        let (wrapper, arity) =
            generate_query_wrapper(r#"should_engage("did:plc:abc")"#, None, &empty_types);
        assert_eq!(arity, 1);
        assert!(wrapper.contains(".decl _query_result(arg0: symbol)"));
        assert!(wrapper.contains(".output _query_result"));
        assert!(
            wrapper.contains(r#"_query_result("did:plc:abc") :- should_engage("did:plc:abc")."#)
        );
    }

    #[test]
    fn test_generate_query_wrapper_mixed_args() {
        let empty_types = HashMap::new();
        let (wrapper, arity) =
            generate_query_wrapper(r#"follows(X, "did:plc:abc")"#, None, &empty_types);
        assert_eq!(arity, 1);
        assert!(wrapper.contains(r#"_query_result(X) :- follows(X, "did:plc:abc")."#));
    }

    #[test]
    fn test_generate_query_wrapper_with_underscore() {
        // Underscore (anonymous variable) should be excluded from the head
        let empty_types = HashMap::new();
        let (wrapper, arity) =
            generate_query_wrapper(r#"did_handle(DID, Handle, _)"#, None, &empty_types);
        assert_eq!(arity, 2);
        // Head should NOT contain underscore
        assert!(wrapper.contains("_query_result(DID, Handle) :- did_handle(DID, Handle, _)."));
        assert!(!wrapper.contains("_query_result(DID, Handle, _)"));
    }

    #[test]
    fn test_generate_query_wrapper_all_underscores() {
        // Query with all anonymous variables should produce nullary result
        let empty_types = HashMap::new();
        let (wrapper, arity) =
            generate_query_wrapper(r#"did_handle(_, _, _)"#, None, &empty_types);
        assert_eq!(arity, 0);
        assert!(wrapper.contains("_query_result() :- did_handle(_, _, _)."));
    }

    #[test]
    fn test_generate_query_wrapper_with_typed_predicate() {
        // When predicate_types has number types, _query_result should use them
        let mut types = HashMap::new();
        types.insert(
            "scored".to_string(),
            vec![
                "symbol".to_string(),
                "number".to_string(),
                "symbol".to_string(),
            ],
        );
        let (wrapper, arity) = generate_query_wrapper("scored(X, Y, _)", None, &types);
        assert_eq!(arity, 2);
        // X is at position 0 (symbol), Y is at position 1 (number)
        assert!(
            wrapper.contains(".decl _query_result(arg0: symbol, arg1: number)"),
            "wrapper was: {}",
            wrapper
        );
        assert!(wrapper.contains("_query_result(X, Y) :- scored(X, Y, _)."));
    }

    #[test]
    fn test_generate_query_wrapper_typed_base_decl() {
        // When the base predicate is not declared, it should use types from the map
        let mut types = HashMap::new();
        types.insert(
            "metric".to_string(),
            vec!["symbol".to_string(), "number".to_string()],
        );
        let declared = HashSet::new();
        let (wrapper, _arity) =
            generate_query_wrapper("metric(X, Y)", Some(&declared), &types);
        assert!(
            wrapper.contains(".decl metric(arg0: symbol, arg1: number)"),
            "wrapper was: {}",
            wrapper
        );
        assert!(wrapper.contains(".decl _query_result(arg0: symbol, arg1: number)"));
    }

    #[test]
    fn test_generate_query_wrapper_typed_constant_only() {
        // When all args are constants, result columns should still use correct types
        let mut types = HashMap::new();
        types.insert(
            "threshold".to_string(),
            vec!["symbol".to_string(), "number".to_string()],
        );
        let (wrapper, arity) =
            generate_query_wrapper(r#"threshold("high", 42)"#, None, &types);
        assert_eq!(arity, 2);
        assert!(
            wrapper.contains(".decl _query_result(arg0: symbol, arg1: number)"),
            "wrapper was: {}",
            wrapper
        );
    }

    #[test]
    fn test_parse_declaration_arg_types() {
        // Basic name: type format
        let (name, types) =
            parse_declaration_arg_types("my_pred(x: number, y: symbol)").unwrap();
        assert_eq!(name, "my_pred");
        assert_eq!(types, vec!["number", "symbol"]);

        // With .decl prefix
        let (name, types) =
            parse_declaration_arg_types(".decl scored(name: symbol, val: number)").unwrap();
        assert_eq!(name, "scored");
        assert_eq!(types, vec!["symbol", "number"]);

        // Bare names default to symbol
        let (name, types) = parse_declaration_arg_types("bare(x, y)").unwrap();
        assert_eq!(name, "bare");
        assert_eq!(types, vec!["symbol", "symbol"]);

        // Empty args
        let (name, types) = parse_declaration_arg_types("nullary()").unwrap();
        assert_eq!(name, "nullary");
        assert!(types.is_empty());

        // Invalid input
        assert!(parse_declaration_arg_types("no_parens").is_none());
    }

    #[test]
    fn test_generate_input_declarations_from_arities() {
        let mut arities = HashMap::new();
        arities.insert("follows".to_string(), 2);
        arities.insert("interested_in".to_string(), 2);

        let (decls, declared) = generate_input_declarations_from_arities(&arities);

        // Should have metadata relations
        assert!(decls.contains(".decl _fact"));
        assert!(decls.contains(".decl _confidence"));
        assert!(decls.contains(".decl _created_at(rkey: symbol, timestamp: symbol)"));

        // Should have user predicates (with rkey suffix)
        assert!(decls.contains(".decl follows(arg0: symbol, arg1: symbol, rkey: symbol)"));
        assert!(decls.contains(".input follows"));
        assert!(decls.contains(".decl _all_follows(arg0: symbol, arg1: symbol, rkey: symbol)"));

        // Check declared set
        assert!(declared.contains("follows"));
        assert!(declared.contains("_all_follows"));
        assert!(declared.contains("_fact"));
    }

    #[tokio::test]
    async fn test_datalog_cache_basic() {
        let cache = DatalogCache::new_temp().unwrap();

        // Add a fact
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("follows", vec!["did:a", "did:b"]),
                "cid1".to_string(),
            )
            .await;

        assert_eq!(cache.fact_count().await, 1);
        assert_eq!(cache.facts_generation(), 1);
    }

    #[tokio::test]
    async fn test_datalog_cache_dirty_tracking() {
        let cache = DatalogCache::new_temp().unwrap();

        // Add facts
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("follows", vec!["did:a", "did:b"]),
                "cid1".to_string(),
            )
            .await;
        cache
            .add_fact(
                "rkey2".to_string(),
                make_fact("interested_in", vec!["did:a", "rust"]),
                "cid2".to_string(),
            )
            .await;

        // Check dirty predicates
        let dirty = cache.dirty_predicates.read().await;
        assert!(dirty.contains("follows"));
        assert!(dirty.contains("interested_in"));
    }

    #[tokio::test]
    async fn test_extra_rules_constant_filtering() {
        // This test demonstrates the bug: constant arguments in ad-hoc rules
        // should filter results, not return all rows.

        let cache = DatalogCache::new_temp().unwrap();

        // Add facts with different second arguments
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("category", vec!["did:a", "protocol_design"]),
                "cid1".to_string(),
            )
            .await;
        cache
            .add_fact(
                "rkey2".to_string(),
                make_fact("category", vec!["did:b", "social"]),
                "cid2".to_string(),
            )
            .await;
        cache
            .add_fact(
                "rkey3".to_string(),
                make_fact("category", vec!["did:c", "protocol_design"]),
                "cid3".to_string(),
            )
            .await;
        cache
            .add_fact(
                "rkey4".to_string(),
                make_fact("category", vec!["did:d", "governance"]),
                "cid4".to_string(),
            )
            .await;

        // Query with constant filter - should only return protocol_design rows
        // Note: category now has rkey as last arg, so use _ to ignore it
        let result = cache
            .execute_query(
                "filtered(X)",
                Some(r#"filtered(X) :- category(X, "protocol_design", _)."#),
            )
            .await
            .unwrap();

        // BUG: Without fix, this returns ALL 4 rows
        // FIXED: Should return only 2 rows (did:a and did:c)
        assert_eq!(
            result.len(),
            2,
            "Expected 2 results for protocol_design filter, got {}",
            result.len()
        );

        let dids: Vec<&str> = result.iter().map(|r| r[0].as_str()).collect();
        assert!(dids.contains(&"did:a"));
        assert!(dids.contains(&"did:c"));
        assert!(!dids.contains(&"did:b")); // social
        assert!(!dids.contains(&"did:d")); // governance
    }

    #[tokio::test]
    async fn test_extra_rules_multiple_constants() {
        // Test with multiple constant filters in same rule
        let cache = DatalogCache::new_temp().unwrap();

        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("triple", vec!["a", "type", "person"]),
                "cid1".to_string(),
            )
            .await;
        cache
            .add_fact(
                "rkey2".to_string(),
                make_fact("triple", vec!["b", "type", "org"]),
                "cid2".to_string(),
            )
            .await;
        cache
            .add_fact(
                "rkey3".to_string(),
                make_fact("triple", vec!["c", "status", "active"]),
                "cid3".to_string(),
            )
            .await;

        // Note: triple now has rkey as last arg, so use _ to ignore it
        let result = cache
            .execute_query(
                "person(X)",
                Some(r#"person(X) :- triple(X, "type", "person", _)."#),
            )
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], "a");
    }

    #[tokio::test]
    async fn test_extra_rules_without_constants_still_works() {
        // Regression test: extra rules without constants should still work
        // Note: using "link" instead of "follows" since "follows" is a derived predicate
        let cache = DatalogCache::new_temp().unwrap();

        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("link", vec!["a", "b"]),
                "cid1".to_string(),
            )
            .await;
        cache
            .add_fact(
                "rkey2".to_string(),
                make_fact("link", vec!["b", "c"]),
                "cid2".to_string(),
            )
            .await;

        // Note: link now has rkey as last arg, so use _ to ignore it
        let result = cache
            .execute_query("reachable(X, Y)", Some("reachable(X, Y) :- link(X, Y, _)."))
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn test_created_at_temporal_query() {
        let cache = DatalogCache::new_temp().unwrap();

        // Add a fact
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("knows", vec!["alice", "bob"]),
                "cid1".to_string(),
            )
            .await;

        // Query for recent facts (after year 2020)
        let result = cache
            .execute_query(
                "recent(R)",
                Some(r#"recent(R) :- _created_at(R, T), T > "2020-01-01T00:00:00Z"."#),
            )
            .await
            .unwrap();

        // Should find the fact we just created
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], "rkey1");
    }

    #[tokio::test]
    async fn test_created_at_file_generation() {
        let cache = DatalogCache::new_temp().unwrap();

        // Add facts
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("follows", vec!["did:a", "did:b"]),
                "cid1".to_string(),
            )
            .await;
        cache
            .add_fact(
                "rkey2".to_string(),
                make_fact("follows", vec!["did:b", "did:c"]),
                "cid2".to_string(),
            )
            .await;

        // Flush to mark predicates as stale (lazy regeneration)
        cache.flush_dirty_predicates().await.unwrap();

        // Trigger lazy regeneration by ensuring predicates exist
        let predicates: HashSet<String> = ["follows".to_string(), "_created_at".to_string()]
            .into_iter()
            .collect();
        cache.ensure_predicates_exist(&predicates).await.unwrap();

        // Check _created_at.facts exists and has correct format
        let created_at_path = cache.fact_dir.join("_created_at.facts");
        let content = std::fs::read_to_string(&created_at_path).unwrap();

        // Should have entries for both facts
        assert!(content.contains("rkey1\t"));
        assert!(content.contains("rkey2\t"));

        // Each line should have ISO8601 timestamp format
        for line in content.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            assert_eq!(parts.len(), 2);
            let timestamp = parts[1];
            assert!(
                timestamp.contains('T'),
                "timestamp should be ISO8601 format"
            );
        }
    }

    #[tokio::test]
    async fn test_extra_facts_ephemeral() {
        let cache = DatalogCache::new_temp().unwrap();

        // Create a durable rule that uses ephemeral predicates
        // Query with ephemeral facts injected at query time
        let extra_facts = vec![
            r#"thread_depth("at://test/thread", "7")"#.to_string(),
            r#"my_reply_count("at://test/thread", "4")"#.to_string(),
        ];

        let result = cache
            .execute_query_with_facts(
                "should_not_reply(T)",
                Some(r#"should_not_reply(T) :- thread_depth(T, D), D > "5", my_reply_count(T, C), C > "3"."#),
                Some(&extra_facts),
            )
            .await
            .unwrap();

        // Should match since depth=7>5 and reply_count=4>3
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], "at://test/thread");
    }

    #[tokio::test]
    async fn test_extra_facts_no_match() {
        let cache = DatalogCache::new_temp().unwrap();

        // Ephemeral facts that don't satisfy the rule
        let extra_facts = vec![
            r#"thread_depth("at://test/thread", "3")"#.to_string(), // depth=3, not > 5
            r#"my_reply_count("at://test/thread", "4")"#.to_string(),
        ];

        let result = cache
            .execute_query_with_facts(
                "should_not_reply(T)",
                Some(r#"should_not_reply(T) :- thread_depth(T, D), D > "5", my_reply_count(T, C), C > "3"."#),
                Some(&extra_facts),
            )
            .await
            .unwrap();

        // Should not match since depth=3 is not > 5
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_extra_facts_combined_with_durable() {
        let cache = DatalogCache::new_temp().unwrap();

        // Add a durable fact
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("interested_in", vec!["did:test", "rust"]),
                "cid1".to_string(),
            )
            .await;

        // Inject ephemeral context
        let extra_facts = vec![r#"current_topic("rust")"#.to_string()];

        // Note: interested_in now has rkey as last arg, so use _ to ignore it
        let result = cache
            .execute_query_with_facts(
                "relevant_interest(Who, Topic)",
                Some(r#"relevant_interest(Who, Topic) :- interested_in(Who, Topic, _), current_topic(Topic)."#),
                Some(&extra_facts),
            )
            .await
            .unwrap();

        // Should find the match combining durable and ephemeral facts
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], "did:test");
        assert_eq!(result[0][1], "rust");
    }

    #[tokio::test]
    async fn test_extra_facts_direct_query() {
        // Test querying the extra_facts predicate directly
        let cache = DatalogCache::new_temp().unwrap();

        // Add a durable placeholder fact
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("thread_depth", vec!["at://placeholder", "0"]),
                "cid1".to_string(),
            )
            .await;

        // Inject ephemeral fact for same predicate (must include rkey placeholder)
        let extra_facts = vec![r#"thread_depth("at://test/uri", "7", "ephemeral")"#.to_string()];

        // Query the predicate directly - should see BOTH facts
        // Note: thread_depth now has rkey as last arg, use R variable to capture it
        let result = cache
            .execute_query_with_facts("thread_depth(X, Y, R)", None, Some(&extra_facts))
            .await
            .unwrap();

        // Should have both the durable and ephemeral facts
        assert_eq!(
            result.len(),
            2,
            "Expected 2 results (durable + ephemeral), got {:?}",
            result
        );

        let uris: Vec<&str> = result.iter().map(|r| r[0].as_str()).collect();
        assert!(uris.contains(&"at://placeholder"), "Missing durable fact");
        assert!(uris.contains(&"at://test/uri"), "Missing ephemeral fact");
    }

    #[tokio::test]
    async fn test_extra_facts_with_rules_and_declarations() {
        // Test the full scenario: extra_facts + extra_rules with declarations
        let cache = DatalogCache::new_temp().unwrap();

        let extra_facts = vec![
            r#"thread_depth("at://test/thread", "7")"#.to_string(),
            r#"reply_cnt("at://test/thread", "4")"#.to_string(),
        ];

        // Include declarations in extra_rules (multi-line)
        // Note: avoid reserved words like "count" in parameter names
        let extra_rules = r#".decl thread_depth(uri: symbol, depth: symbol)
.decl reply_cnt(uri: symbol, cnt: symbol)
test_result(Uri) :- thread_depth(Uri, D), D > "5", reply_cnt(Uri, C), C > "3"."#;

        let result = cache
            .execute_query_with_facts("test_result(X)", Some(extra_rules), Some(&extra_facts))
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], "at://test/thread");
    }

    #[tokio::test]
    async fn test_fact_validation_conforming_fact() {
        use winter_atproto::{FactDeclArg, FactDeclaration};

        let cache = DatalogCache::new_temp().unwrap();

        // Add a declaration for 2-arg predicate
        let declaration = FactDeclaration {
            predicate: "test_pred".to_string(),
            args: vec![
                FactDeclArg {
                    name: "arg1".to_string(),
                    r#type: "symbol".to_string(),
                    description: Some("First argument".to_string()),
                },
                FactDeclArg {
                    name: "arg2".to_string(),
                    r#type: "symbol".to_string(),
                    description: Some("Second argument".to_string()),
                },
            ],
            description: "Test predicate".to_string(),
            tags: vec![],
            created_at: Utc::now(),
            last_updated: None,
        };

        // Insert declaration
        {
            let mut decls = cache.declarations.write().await;
            let mut decls_by_pred = cache.declarations_by_predicate.write().await;
            decls_by_pred.insert(declaration.predicate.clone(), declaration.clone());
            decls.insert("decl_rkey".to_string(), declaration);
        }

        // Add a conforming fact (2 args)
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("test_pred", vec!["a", "b"]),
                "cid1".to_string(),
            )
            .await;

        // Flush to mark predicates as stale
        cache.flush_dirty_predicates().await.unwrap();

        // Trigger lazy regeneration
        let predicates: HashSet<String> =
            ["test_pred".to_string(), "_validation_error".to_string()]
                .into_iter()
                .collect();
        cache.ensure_predicates_exist(&predicates).await.unwrap();

        // Check that the fact appears in the TSV file
        let tsv_path = cache.fact_dir.join("test_pred.facts");
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        assert!(
            content.contains("a\tb\trkey1"),
            "conforming fact should be in TSV"
        );

        // Check that no validation errors were logged
        let errors_path = cache.fact_dir.join("_validation_error.facts");
        let errors = std::fs::read_to_string(&errors_path).unwrap_or_default();
        assert!(
            !errors.contains("rkey1"),
            "conforming fact should not have validation errors"
        );
    }

    #[tokio::test]
    async fn test_fact_validation_non_conforming_fact() {
        use winter_atproto::{FactDeclArg, FactDeclaration};

        let cache = DatalogCache::new_temp().unwrap();

        // Add a declaration for 2-arg predicate
        let declaration = FactDeclaration {
            predicate: "test_pred".to_string(),
            args: vec![
                FactDeclArg {
                    name: "arg1".to_string(),
                    r#type: "symbol".to_string(),
                    description: Some("First argument".to_string()),
                },
                FactDeclArg {
                    name: "arg2".to_string(),
                    r#type: "symbol".to_string(),
                    description: Some("Second argument".to_string()),
                },
            ],
            description: "Test predicate".to_string(),
            tags: vec![],
            created_at: Utc::now(),
            last_updated: None,
        };

        // Insert declaration
        {
            let mut decls = cache.declarations.write().await;
            let mut decls_by_pred = cache.declarations_by_predicate.write().await;
            decls_by_pred.insert(declaration.predicate.clone(), declaration.clone());
            decls.insert("decl_rkey".to_string(), declaration);
        }

        // Add a non-conforming fact (3 args instead of 2)
        cache
            .add_fact(
                "rkey_bad".to_string(),
                make_fact("test_pred", vec!["a", "b", "c"]), // 3 args, declaration says 2
                "cid_bad".to_string(),
            )
            .await;

        // Flush to mark predicates as stale
        cache.flush_dirty_predicates().await.unwrap();

        // Trigger lazy regeneration
        let predicates: HashSet<String> =
            ["test_pred".to_string(), "_validation_error".to_string()]
                .into_iter()
                .collect();
        cache.ensure_predicates_exist(&predicates).await.unwrap();

        // Check that the bad fact does NOT appear in the TSV file
        let tsv_path = cache.fact_dir.join("test_pred.facts");
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        assert!(
            !content.contains("rkey_bad"),
            "non-conforming fact should NOT be in TSV"
        );

        // Check that validation error was logged
        let errors_path = cache.fact_dir.join("_validation_error.facts");
        let errors = std::fs::read_to_string(&errors_path).unwrap();
        assert!(
            errors.contains("rkey_bad"),
            "non-conforming fact should have validation error"
        );
        assert!(
            errors.contains("test_pred"),
            "error should mention predicate name"
        );
        assert!(
            errors.contains("arity mismatch"),
            "error should describe the issue"
        );
    }

    #[tokio::test]
    async fn test_fact_validation_no_declaration_is_permissive() {
        let cache = DatalogCache::new_temp().unwrap();

        // Add a fact without any declaration
        cache
            .add_fact(
                "rkey1".to_string(),
                make_fact("undeclared_pred", vec!["a", "b", "c", "d"]),
                "cid1".to_string(),
            )
            .await;

        // Flush to mark predicates as stale
        cache.flush_dirty_predicates().await.unwrap();

        // Trigger lazy regeneration
        let predicates: HashSet<String> = [
            "undeclared_pred".to_string(),
            "_validation_error".to_string(),
        ]
        .into_iter()
        .collect();
        cache.ensure_predicates_exist(&predicates).await.unwrap();

        // Check that the fact appears in TSV (permissive when no declaration)
        let tsv_path = cache.fact_dir.join("undeclared_pred.facts");
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        assert!(
            content.contains("rkey1"),
            "fact without declaration should be in TSV"
        );

        // Check that no validation errors were logged
        let errors_path = cache.fact_dir.join("_validation_error.facts");
        let errors = std::fs::read_to_string(&errors_path).unwrap_or_default();
        assert!(
            !errors.contains("rkey1"),
            "undeclared fact should not have validation errors"
        );
    }

    #[tokio::test]
    async fn test_validation_error_queryable() {
        use winter_atproto::{FactDeclArg, FactDeclaration};

        let cache = DatalogCache::new_temp().unwrap();

        // Add a declaration for 2-arg predicate
        let declaration = FactDeclaration {
            predicate: "validated_pred".to_string(),
            args: vec![
                FactDeclArg {
                    name: "arg1".to_string(),
                    r#type: "symbol".to_string(),
                    description: Some("First argument".to_string()),
                },
                FactDeclArg {
                    name: "arg2".to_string(),
                    r#type: "symbol".to_string(),
                    description: Some("Second argument".to_string()),
                },
            ],
            description: "Test predicate".to_string(),
            tags: vec![],
            created_at: Utc::now(),
            last_updated: None,
        };

        // Insert declaration
        {
            let mut decls = cache.declarations.write().await;
            let mut decls_by_pred = cache.declarations_by_predicate.write().await;
            decls_by_pred.insert(declaration.predicate.clone(), declaration.clone());
            decls.insert("decl_rkey".to_string(), declaration);
        }

        // Add a non-conforming fact
        cache
            .add_fact(
                "bad_fact".to_string(),
                make_fact("validated_pred", vec!["a", "b", "c"]), // 3 args, declaration says 2
                "cid_bad".to_string(),
            )
            .await;

        // Query validation errors
        let result = cache
            .execute_query("_validation_error(R, P, E)", None)
            .await
            .unwrap();

        // Should find the validation error
        assert_eq!(result.len(), 1, "should have one validation error");
        assert_eq!(result[0][0], "bad_fact", "rkey should match");
        assert_eq!(result[0][1], "validated_pred", "predicate should match");
        assert!(
            result[0][2].contains("arity mismatch"),
            "error message should describe the issue"
        );
    }
}
