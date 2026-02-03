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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{RwLock, broadcast};
use tracing::{debug, info, trace, warn};

use winter_atproto::{CacheUpdate, Fact, FactDeclaration, RepoCache, Rule, SyncState};

use crate::derived::DerivedFactGenerator;
use crate::error::DatalogError;
use crate::validator::{validate_fact_against_declaration, ValidationError};
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

/// Cached Soufflé program with metadata.
struct CachedProgram {
    /// The full program text (declarations + compiled rules).
    program_text: String,
    /// Predicates that have been declared.
    declared_predicates: HashSet<String>,
    /// Rules generation when this program was built.
    rules_generation: u64,
    /// Facts generation when this program was built.
    facts_generation: u64,
}

/// Cache for datalog query execution.
///
/// Maintains persistent TSV files and cached program text for efficient
/// incremental query execution.
pub struct DatalogCache {
    /// Persistent directory for TSV files.
    fact_dir: PathBuf,

    /// Winter's DID for derived fact generation.
    self_did: Option<String>,

    /// Winter's handle (for blog WhiteWind URLs in derived facts).
    handle: Option<String>,

    /// Predicate name -> arity (for declaration generation).
    predicate_arities: RwLock<HashMap<String, usize>>,

    /// Facts indexed by rkey (for incremental updates).
    facts_by_rkey: RwLock<HashMap<String, CachedFactData>>,

    /// CID to rkey mapping for supersession lookups.
    cid_to_rkey: RwLock<HashMap<String, String>>,

    /// Set of CIDs that have been superseded.
    superseded_cids: RwLock<HashSet<String>>,

    /// Cached rules (for compilation).
    rules: RwLock<Vec<Rule>>,

    /// Cached fact declarations (for query-time .decl generation), keyed by rkey.
    declarations: RwLock<HashMap<String, FactDeclaration>>,

    /// Fact declarations indexed by predicate name for validation.
    /// This is a secondary index maintained alongside `declarations`.
    declarations_by_predicate: RwLock<HashMap<String, FactDeclaration>>,

    /// Cached base program (declarations + compiled rules).
    base_program: RwLock<Option<CachedProgram>>,

    /// Facts generation counter (bumped on any fact change).
    facts_generation: AtomicU64,

    /// Rules generation counter (bumped on any rule change).
    rules_generation: AtomicU64,

    /// Predicates needing TSV regeneration.
    dirty_predicates: RwLock<HashSet<String>>,

    /// Whether all predicates are dirty (full regeneration needed).
    full_regen_needed: RwLock<bool>,

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
            rules: RwLock::new(Vec::new()),
            declarations: RwLock::new(HashMap::new()),
            declarations_by_predicate: RwLock::new(HashMap::new()),
            base_program: RwLock::new(None),
            facts_generation: AtomicU64::new(0),
            rules_generation: AtomicU64::new(0),
            dirty_predicates: RwLock::new(HashSet::new()),
            full_regen_needed: RwLock::new(true),
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
                        warn!(skipped = n, "datalog cache update listener lagged");
                        // Mark everything dirty since we missed updates
                        *cache.full_regen_needed.write().await = true;
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
        info!(facts = facts.len(), rules = rules.len(), "loaded facts and rules");

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

        // Insert rules
        info!("inserting rules into cache");
        {
            let mut rules_guard = self.rules.write().await;
            rules_guard.clear();
            for (_, cached) in rules {
                rules_guard.push(cached.value);
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
            info!(count = tool_approvals_list.len(), "populating tool approvals");
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
            info!(count = declarations_list.len(), "populating fact declarations");
            let mut decls = self.declarations.write().await;
            let mut decls_by_pred = self.declarations_by_predicate.write().await;
            for (rkey, cached) in declarations_list {
                decls_by_pred.insert(cached.value.predicate.clone(), cached.value.clone());
                decls.insert(rkey, cached.value);
            }
        }

        // Mark everything as needing regeneration
        *self.full_regen_needed.write().await = true;
        self.facts_generation.fetch_add(1, Ordering::SeqCst);
        self.rules_generation.fetch_add(1, Ordering::SeqCst);

        // Invalidate cached program
        *self.base_program.write().await = None;

        // Log derived fact counts for diagnostics
        // Collect all values first to avoid holding locks across debug! macro
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
            "datalog cache populated, flushing TSV files"
        );

        // Immediately flush all TSV files so they exist before any query runs
        if let Err(e) = self.flush_dirty_predicates().await {
            warn!(error = %e, "failed to flush TSV files after population");
        } else {
            info!("TSV files flushed successfully");
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
            CacheUpdate::RuleCreated { rule, .. } => {
                self.add_rule(rule).await;
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
            | ref update @ CacheUpdate::BlogEntryDeleted { .. } => {
                // Forward to DerivedFactGenerator
                let mut derived = self.derived.write().await;
                derived.handle_update(update);
                // Invalidate cached program since derived predicates may have changed
                if derived.has_dirty_predicates() {
                    *self.base_program.write().await = None;
                }
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
            | ref update @ CacheUpdate::JobDeleted { .. } => {
                // Forward to DerivedFactGenerator
                let mut derived = self.derived.write().await;
                derived.handle_update(update);
                // Invalidate cached program since derived predicates may have changed
                if derived.has_dirty_predicates() {
                    *self.base_program.write().await = None;
                }
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
                // Invalidate base program since declarations changed
                let mut prog = self.base_program.write().await;
                *prog = None;
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
                // Invalidate base program since declarations changed
                let mut prog = self.base_program.write().await;
                *prog = None;
                // Mark for full regen since validation rules may have changed
                *self.full_regen_needed.write().await = true;
            }
            // State updates - extract followers for is_followed_by predicate
            CacheUpdate::StateUpdated { state } => {
                let followers_set: std::collections::HashSet<String> =
                    state.followers.into_iter().collect();
                let mut derived = self.derived.write().await;
                derived.set_followers(followers_set);
                // Invalidate cached program since derived predicates may have changed
                if derived.has_dirty_predicates() {
                    *self.base_program.write().await = None;
                }
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
    async fn add_rule(&self, rule: Rule) {
        let mut rules = self.rules.write().await;
        rules.push(rule);
        drop(rules);

        self.rules_generation.fetch_add(1, Ordering::SeqCst);
        *self.base_program.write().await = None;
        trace!("rule added, program invalidated");
    }

    /// Update a rule.
    async fn update_rule(&self, _rkey: &str, rule: Rule) {
        // For simplicity, just replace rules with same name
        let mut rules = self.rules.write().await;
        if let Some(existing) = rules.iter_mut().find(|r| r.name == rule.name) {
            *existing = rule;
        } else {
            rules.push(rule);
        }
        drop(rules);

        self.rules_generation.fetch_add(1, Ordering::SeqCst);
        *self.base_program.write().await = None;
        trace!("rule updated, program invalidated");
    }

    /// Remove a rule.
    async fn remove_rule(&self, _rkey: &str) {
        // We don't have rkey -> rule mapping, so just invalidate
        self.rules_generation.fetch_add(1, Ordering::SeqCst);
        *self.base_program.write().await = None;
        trace!("rule removed, program invalidated");
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
        self.execute_query_with_facts(query, extra_rules, None).await
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
    pub async fn execute_query_with_facts_and_declarations(
        &self,
        query: &str,
        extra_rules: Option<&str>,
        extra_facts: Option<&[String]>,
        extra_declarations: Option<&[String]>,
    ) -> Result<Vec<Vec<String>>, DatalogError> {
        // Flush dirty predicates
        self.flush_dirty_predicates().await?;

        // Get or generate base program
        let (base_program, declared_predicates) = self.get_or_generate_base_program().await?;

        // Build full program
        let mut program = base_program;

        // Track predicates declared by ad-hoc rules/facts
        let mut all_declared = declared_predicates;

        // Parse extra_rules for explicit .decl statements to avoid conflicts
        let mut user_declared: HashSet<String> = extra_rules
            .map(|rules| parse_decl_statements(rules))
            .unwrap_or_default();

        // Add stored declarations from PDS (fact declarations created via MCP tools)
        {
            let stored_decls = self.declarations.read().await;
            if !stored_decls.is_empty() {
                program.push_str("// Stored fact declarations\n");
                for (_, decl) in stored_decls.iter() {
                    // Skip if already declared
                    if all_declared.contains(&decl.predicate) || user_declared.contains(&decl.predicate) {
                        continue;
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
                program.push('\n');
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
        if let Some(facts) = extra_facts {
            if !facts.is_empty() {
                // Parse facts to extract predicates and arities
                let parsed_facts = parse_extra_facts(facts);

                // Auto-declare any new predicates (skip if user declared explicitly)
                for (name, arity) in &parsed_facts {
                    if !all_declared.contains(name) && !user_declared.contains(name) {
                        let params: Vec<String> = (0..*arity)
                            .map(|i| format!("arg{}: symbol", i))
                            .collect();
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
        }

        // Generate declarations for ad-hoc rule heads
        if let Some(extra) = extra_rules {
            let heads = RuleCompiler::parse_extra_rules_heads(extra);
            for (name, arity) in heads {
                if !all_declared.contains(&name) && !user_declared.contains(&name) {
                    let params: Vec<String> = (0..arity)
                        .map(|i| format!("arg{}: symbol", i))
                        .collect();
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
        let (wrapper, _result_arity) = generate_query_wrapper(query, Some(&all_declared));
        program.push_str(&wrapper);

        // Log program details for debugging derived predicate issues
        if query.contains("has_note") || query.contains("has_thought") || query.contains("has_blog") {
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
    pub async fn flush_dirty_predicates(&self) -> Result<(), DatalogError> {
        let full_regen = *self.full_regen_needed.read().await;

        if full_regen {
            // Full regeneration needed
            self.regenerate_all_files().await?;
            *self.full_regen_needed.write().await = false;
            self.dirty_predicates.write().await.clear();

            // Also regenerate all derived facts
            let mut derived = self.derived.write().await;
            derived.regenerate_all(&self.fact_dir)?;
            return Ok(());
        }

        // Get dirty predicates (user facts)
        let dirty: HashSet<String> = {
            let mut dirty_guard = self.dirty_predicates.write().await;
            std::mem::take(&mut *dirty_guard)
        };

        if !dirty.is_empty() {
            debug!(predicates = ?dirty, "flushing dirty predicates");

            // Get facts, arities, and declarations for validation
            let facts = self.facts_by_rkey.read().await;
            let arities = self.predicate_arities.read().await;
            let decls_by_pred = self.declarations_by_predicate.read().await;

            for predicate in dirty {
                if let Some(&arity) = arities.get(&predicate) {
                    self.regenerate_predicate_files(&predicate, arity, &facts, &decls_by_pred)?;
                }
            }
        }

        // Flush derived predicates
        let mut derived = self.derived.write().await;
        if derived.has_dirty_predicates() {
            derived.flush_to_dir(&self.fact_dir)?;
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
        // Current facts file (non-superseded only)
        let current_path = self.fact_dir.join(format!("{}.facts", predicate));
        let mut current_file = std::fs::File::create(&current_path)?;

        // All facts file (with rkey prefix)
        let all_path = self.fact_dir.join(format!("_all_{}.facts", predicate));
        let mut all_file = std::fs::File::create(&all_path)?;

        // Validation errors file (append mode for incremental predicates)
        let errors_path = self.fact_dir.join("_validation_error.facts");
        let mut errors_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&errors_path)?;

        for (rkey, data) in facts.iter() {
            if data.fact.predicate != predicate {
                continue;
            }

            // Validate against declaration if one exists
            if let Some(error) = validate_fact_against_declaration(&data.fact, declarations_by_predicate) {
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
                .map(|a| a.replace('\t', " ").replace('\n', " "))
                .collect();
            let args_str = args.join("\t");

            // Write to all file (always, rkey at end)
            writeln!(all_file, "{}\t{}", args_str, rkey)?;

            // Write to current file (only if not superseded, rkey at end)
            if !data.is_superseded {
                writeln!(current_file, "{}\t{}", args_str, rkey)?;
            }
        }

        trace!(predicate, arity, "regenerated predicate files");
        Ok(())
    }

    /// Regenerate all TSV files from scratch.
    ///
    /// Facts are validated against declarations. Invalid facts are:
    /// - Still written to metadata files (_fact, _confidence, etc.)
    /// - Skipped from predicate TSV files to prevent Soufflé errors
    /// - Logged to _validation_error.facts for investigation
    async fn regenerate_all_files(&self) -> Result<(), DatalogError> {
        let facts = self.facts_by_rkey.read().await;
        let arities = self.predicate_arities.read().await;
        let decls_by_pred = self.declarations_by_predicate.read().await;

        // Clear and regenerate metadata files
        let mut fact_file = std::fs::File::create(self.fact_dir.join("_fact.facts"))?;
        let mut confidence_file = std::fs::File::create(self.fact_dir.join("_confidence.facts"))?;
        let mut source_file = std::fs::File::create(self.fact_dir.join("_source.facts"))?;
        let mut supersedes_file = std::fs::File::create(self.fact_dir.join("_supersedes.facts"))?;
        let mut created_at_file = std::fs::File::create(self.fact_dir.join("_created_at.facts"))?;

        // Clear validation errors file (we're doing full regen)
        let mut errors_file = std::fs::File::create(self.fact_dir.join("_validation_error.facts"))?;

        // Group facts by predicate for efficient file writing
        // Also track validation state per fact
        let mut by_predicate: HashMap<&str, Vec<(&str, &CachedFactData, Option<ValidationError>)>> = HashMap::new();

        for (rkey, data) in facts.iter() {
            // Validate against declaration
            let validation_error = validate_fact_against_declaration(&data.fact, &decls_by_pred);

            // Log validation errors
            if let Some(ref error) = validation_error {
                warn!(
                    rkey = %rkey,
                    predicate = %data.fact.predicate,
                    error = %error,
                    "fact fails schema validation"
                );
                writeln!(errors_file, "{}\t{}\t{}", rkey, data.fact.predicate, error)?;
            }

            by_predicate
                .entry(&data.fact.predicate)
                .or_default()
                .push((rkey.as_str(), data, validation_error));

            // Write metadata (always, even for invalid facts - they still exist in PDS)
            writeln!(fact_file, "{}\t{}\t{}", rkey, data.fact.predicate, data.cid)?;

            if let Some(conf) = data.fact.confidence {
                if (conf - 1.0).abs() > f64::EPSILON {
                    writeln!(confidence_file, "{}\t{}", rkey, conf)?;
                }
            }

            if let Some(ref source) = data.fact.source {
                writeln!(source_file, "{}\t{}", rkey, source)?;
            }

            if let Some(ref supersedes_cid) = data.fact.supersedes {
                // Look up the rkey for the superseded CID
                let cid_to_rkey = self.cid_to_rkey.read().await;
                if let Some(old_rkey) = cid_to_rkey.get(supersedes_cid) {
                    writeln!(supersedes_file, "{}\t{}", rkey, old_rkey)?;
                }
            }

            // Write to _created_at.facts (dense - every fact)
            writeln!(created_at_file, "{}\t{}", rkey, data.fact.created_at.to_rfc3339())?;
        }

        // Write predicate files (skip invalid facts)
        for (predicate, facts_for_pred) in by_predicate {
            if let Some(&_arity) = arities.get(predicate) {
                let current_path = self.fact_dir.join(format!("{}.facts", predicate));
                let all_path = self.fact_dir.join(format!("_all_{}.facts", predicate));

                let mut current_file = std::fs::File::create(&current_path)?;
                let mut all_file = std::fs::File::create(&all_path)?;

                for (rkey, data, validation_error) in facts_for_pred {
                    // Skip invalid facts from predicate files
                    if validation_error.is_some() {
                        continue;
                    }

                    // Escape tabs and newlines in arguments to prevent TSV corruption
                    let args: Vec<String> = data
                        .fact
                        .args
                        .iter()
                        .map(|a| a.replace('\t', " ").replace('\n', " "))
                        .collect();
                    let args_str = args.join("\t");

                    // Write with rkey at end
                    writeln!(all_file, "{}\t{}", args_str, rkey)?;

                    if !data.is_superseded {
                        writeln!(current_file, "{}\t{}", args_str, rkey)?;
                    }
                }
            }
        }

        debug!(
            facts_count = facts.len(),
            predicates = arities.len(),
            "regenerated all TSV files"
        );

        Ok(())
    }

    /// Get or generate the base program (declarations + compiled rules).
    async fn get_or_generate_base_program(
        &self,
    ) -> Result<(String, HashSet<String>), DatalogError> {
        let current_facts_gen = self.facts_generation.load(Ordering::SeqCst);
        let current_rules_gen = self.rules_generation.load(Ordering::SeqCst);

        // Check if cached program is still valid
        {
            let cached = self.base_program.read().await;
            if let Some(ref prog) = *cached {
                if prog.facts_generation == current_facts_gen
                    && prog.rules_generation == current_rules_gen
                {
                    trace!("using cached base program");
                    return Ok((prog.program_text.clone(), prog.declared_predicates.clone()));
                }
            }
        }

        // Generate new program
        debug!("generating new base program");

        let mut program = String::new();
        let mut declared_predicates = HashSet::new();

        // Generate input declarations from user fact arities
        let arities = self.predicate_arities.read().await;
        let (input_decls, input_declared) = generate_input_declarations_from_arities(&arities);
        program.push_str(&input_decls);
        declared_predicates.extend(input_declared);

        // Generate input declarations for derived predicates
        let (derived_input_decls, derived_input_declared) =
            generate_derived_input_declarations(&declared_predicates);
        program.push_str(&derived_input_decls);
        declared_predicates.extend(derived_input_declared);

        // Generate derived declarations from rules (skip already-declared predicates)
        let rules = self.rules.read().await;
        let (derived_decls, derived_declared) =
            RuleCompiler::generate_derived_declarations(&rules, Some(&declared_predicates));
        program.push_str(&derived_decls);
        declared_predicates.extend(derived_declared);

        // Compile rules
        let compiled_rules = RuleCompiler::compile_rules(&rules)?;
        program.push_str(&compiled_rules);

        // Cache the program
        {
            let mut cached = self.base_program.write().await;
            *cached = Some(CachedProgram {
                program_text: program.clone(),
                declared_predicates: declared_predicates.clone(),
                rules_generation: current_rules_gen,
                facts_generation: current_facts_gen,
            });
        }

        Ok((program, declared_predicates))
    }

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
         .decl _validation_error(rkey: symbol, predicate: symbol, error_msg: symbol)\n\
         .input _validation_error\n\n",
    );
    declared_set.insert("_fact".to_string());
    declared_set.insert("_confidence".to_string());
    declared_set.insert("_source".to_string());
    declared_set.insert("_supersedes".to_string());
    declared_set.insert("_created_at".to_string());
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

/// Generate input declarations for derived predicates (from DerivedFactGenerator).
///
/// Returns a tuple of (declarations string, set of declared predicate names).
fn generate_derived_input_declarations(
    already_declared: &HashSet<String>,
) -> (String, HashSet<String>) {
    let mut declarations = String::new();
    let mut declared_set = HashSet::new();

    // Note: We used to declare _derived here but it's not needed and the file
    // wasn't being created, causing Soufflé to fail when it couldn't find the input.

    // Get arities from DerivedFactGenerator
    for (predicate, arity) in DerivedFactGenerator::arities() {
        if already_declared.contains(predicate) {
            continue;
        }

        let params: Vec<String> = (0..arity).map(|i| format!("arg{}: symbol", i)).collect();
        declarations.push_str(&format!(
            ".decl {}({})\n.input {}\n\n",
            predicate,
            params.join(", "),
            predicate
        ));
        declared_set.insert(predicate.to_string());
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
        return Some(ParsedQuery {
            name,
            args: vec![],
        });
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

/// Parse `.decl` statements from extra_rules to find user-declared predicates.
///
/// This allows us to skip auto-declaring predicates that the user explicitly declared,
/// avoiding conflicts when parameter names differ.
fn parse_decl_statements(rules: &str) -> HashSet<String> {
    let mut declared = HashSet::new();
    for line in rules.lines() {
        let line = line.trim();
        if line.starts_with(".decl ") {
            // Extract predicate name: ".decl predicate_name(..."
            let rest = &line[6..]; // Skip ".decl "
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
fn generate_query_wrapper(
    query: &str,
    declared_predicates: Option<&HashSet<String>>,
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

    // Build the result predicate declaration
    let decl = if result_arity > 0 {
        let params: Vec<String> = (0..result_arity)
            .map(|i| format!("arg{}: symbol", i))
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
            let params: Vec<String> = (0..parsed.arity())
                .map(|i| format!("arg{}: symbol", i))
                .collect();
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
        let (wrapper, arity) = generate_query_wrapper(r#"should_engage("did:plc:abc")"#, None);
        assert_eq!(arity, 1);
        assert!(wrapper.contains(".decl _query_result(arg0: symbol)"));
        assert!(wrapper.contains(".output _query_result"));
        assert!(wrapper.contains(r#"_query_result("did:plc:abc") :- should_engage("did:plc:abc")."#));
    }

    #[test]
    fn test_generate_query_wrapper_mixed_args() {
        let (wrapper, arity) = generate_query_wrapper(r#"follows(X, "did:plc:abc")"#, None);
        assert_eq!(arity, 1);
        assert!(wrapper.contains(r#"_query_result(X) :- follows(X, "did:plc:abc")."#));
    }

    #[test]
    fn test_generate_query_wrapper_with_underscore() {
        // Underscore (anonymous variable) should be excluded from the head
        let (wrapper, arity) = generate_query_wrapper(r#"did_handle(DID, Handle, _)"#, None);
        assert_eq!(arity, 2);
        // Head should NOT contain underscore
        assert!(wrapper.contains("_query_result(DID, Handle) :- did_handle(DID, Handle, _)."));
        assert!(!wrapper.contains("_query_result(DID, Handle, _)"));
    }

    #[test]
    fn test_generate_query_wrapper_all_underscores() {
        // Query with all anonymous variables should produce nullary result
        let (wrapper, arity) = generate_query_wrapper(r#"did_handle(_, _, _)"#, None);
        assert_eq!(arity, 0);
        assert!(wrapper.contains("_query_result() :- did_handle(_, _, _)."));
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
            .execute_query(
                "reachable(X, Y)",
                Some("reachable(X, Y) :- link(X, Y, _)."),
            )
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

        // Flush to generate files
        cache.flush_dirty_predicates().await.unwrap();

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
            assert!(timestamp.contains('T'), "timestamp should be ISO8601 format");
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
        assert_eq!(result.len(), 2, "Expected 2 results (durable + ephemeral), got {:?}", result);

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
        use winter_atproto::{FactDeclaration, FactDeclArg};

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

        // Flush to generate TSV
        cache.flush_dirty_predicates().await.unwrap();

        // Check that the fact appears in the TSV file
        let tsv_path = cache.fact_dir.join("test_pred.facts");
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        assert!(content.contains("a\tb\trkey1"), "conforming fact should be in TSV");

        // Check that no validation errors were logged
        let errors_path = cache.fact_dir.join("_validation_error.facts");
        let errors = std::fs::read_to_string(&errors_path).unwrap_or_default();
        assert!(!errors.contains("rkey1"), "conforming fact should not have validation errors");
    }

    #[tokio::test]
    async fn test_fact_validation_non_conforming_fact() {
        use winter_atproto::{FactDeclaration, FactDeclArg};

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

        // Flush to generate TSV
        cache.flush_dirty_predicates().await.unwrap();

        // Check that the bad fact does NOT appear in the TSV file
        let tsv_path = cache.fact_dir.join("test_pred.facts");
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        assert!(!content.contains("rkey_bad"), "non-conforming fact should NOT be in TSV");

        // Check that validation error was logged
        let errors_path = cache.fact_dir.join("_validation_error.facts");
        let errors = std::fs::read_to_string(&errors_path).unwrap();
        assert!(errors.contains("rkey_bad"), "non-conforming fact should have validation error");
        assert!(errors.contains("test_pred"), "error should mention predicate name");
        assert!(errors.contains("arity mismatch"), "error should describe the issue");
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

        // Flush to generate TSV
        cache.flush_dirty_predicates().await.unwrap();

        // Check that the fact appears in TSV (permissive when no declaration)
        let tsv_path = cache.fact_dir.join("undeclared_pred.facts");
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        assert!(content.contains("rkey1"), "fact without declaration should be in TSV");

        // Check that no validation errors were logged
        let errors_path = cache.fact_dir.join("_validation_error.facts");
        let errors = std::fs::read_to_string(&errors_path).unwrap_or_default();
        assert!(!errors.contains("rkey1"), "undeclared fact should not have validation errors");
    }

    #[tokio::test]
    async fn test_validation_error_queryable() {
        use winter_atproto::{FactDeclaration, FactDeclArg};

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
        assert!(result[0][2].contains("arity mismatch"), "error message should describe the issue");
    }
}
