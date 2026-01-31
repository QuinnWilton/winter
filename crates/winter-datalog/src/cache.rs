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
use tracing::{debug, trace, warn};

use winter_atproto::{CacheUpdate, Fact, RepoCache, Rule};

use crate::error::DatalogError;
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
}

impl DatalogCache {
    /// Create a new DatalogCache with the given cache directory.
    ///
    /// The directory will be created if it doesn't exist.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Result<Arc<Self>, DatalogError> {
        let fact_dir = cache_dir.into();
        std::fs::create_dir_all(&fact_dir)?;

        Ok(Arc::new(Self {
            fact_dir,
            predicate_arities: RwLock::new(HashMap::new()),
            facts_by_rkey: RwLock::new(HashMap::new()),
            cid_to_rkey: RwLock::new(HashMap::new()),
            superseded_cids: RwLock::new(HashSet::new()),
            rules: RwLock::new(Vec::new()),
            base_program: RwLock::new(None),
            facts_generation: AtomicU64::new(0),
            rules_generation: AtomicU64::new(0),
            dirty_predicates: RwLock::new(HashSet::new()),
            full_regen_needed: RwLock::new(true),
            executor: SouffleExecutor::new(),
        }))
    }

    /// Create a new DatalogCache using a temp directory.
    pub fn new_temp() -> Result<Arc<Self>, DatalogError> {
        let temp_dir = tempfile::tempdir()?;
        // Keep the TempDir around by converting it to PathBuf
        let path = temp_dir.keep();
        Self::new(path)
    }

    /// Get the fact directory path.
    pub fn fact_dir(&self) -> &Path {
        &self.fact_dir
    }

    /// Start listening for updates from a RepoCache.
    ///
    /// This spawns a background task that processes cache updates
    /// and maintains the datalog cache in sync.
    pub fn start_update_listener(self: &Arc<Self>, repo_cache: &RepoCache) {
        let mut rx = repo_cache.subscribe();
        let cache = Arc::clone(self);

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
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
        let facts = repo_cache.list_facts();
        let rules = repo_cache.list_rules();

        // Clear existing data
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
            for (rkey, cached) in facts {
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
        {
            let mut rules_guard = self.rules.write().await;
            rules_guard.clear();
            for (_, cached) in rules {
                rules_guard.push(cached.value);
            }
        }

        // Mark everything as needing regeneration
        *self.full_regen_needed.write().await = true;
        self.facts_generation.fetch_add(1, Ordering::SeqCst);
        self.rules_generation.fetch_add(1, Ordering::SeqCst);

        // Invalidate cached program
        *self.base_program.write().await = None;

        debug!(
            facts = self.facts_by_rkey.read().await.len(),
            rules = self.rules.read().await.len(),
            "datalog cache populated from repo cache"
        );
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
            // DatalogCache only cares about facts and rules
            CacheUpdate::ThoughtCreated { .. }
            | CacheUpdate::ThoughtDeleted { .. }
            | CacheUpdate::NoteCreated { .. }
            | CacheUpdate::NoteUpdated { .. }
            | CacheUpdate::NoteDeleted { .. }
            | CacheUpdate::JobCreated { .. }
            | CacheUpdate::JobUpdated { .. }
            | CacheUpdate::JobDeleted { .. }
            | CacheUpdate::IdentityUpdated { .. } => {
                // Ignored - these don't affect datalog queries
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
    /// 3. Append the query and extra rules
    /// 4. Execute with Soufflé
    pub async fn execute_query(
        &self,
        query: &str,
        extra_rules: Option<&str>,
    ) -> Result<Vec<Vec<String>>, DatalogError> {
        // Flush dirty predicates
        self.flush_dirty_predicates().await?;

        // Get or generate base program
        let (base_program, declared_predicates) = self.get_or_generate_base_program().await?;

        // Parse query to get predicate info
        let (query_pred, query_arity) = parse_query_predicate(query);

        // Build full program
        let mut program = base_program;

        // Add extra ad-hoc rules if provided
        if let Some(extra) = extra_rules {
            program.push_str("// Ad-hoc rules\n");
            program.push_str(extra);
            program.push_str("\n\n");
        }

        // Generate output declaration for the query predicate
        program.push_str(&RuleCompiler::generate_output_declaration(
            &query_pred,
            query_arity,
            Some(&declared_predicates),
        ));

        debug!(
            query = %query,
            program_len = program.len(),
            "executing cached query"
        );

        // Execute
        let output = self.executor.execute(&program, &self.fact_dir).await?;

        // Parse results
        Ok(SouffleExecutor::parse_output(&output))
    }

    /// Flush dirty predicates by regenerating their TSV files.
    async fn flush_dirty_predicates(&self) -> Result<(), DatalogError> {
        let full_regen = *self.full_regen_needed.read().await;

        if full_regen {
            // Full regeneration needed
            self.regenerate_all_files().await?;
            *self.full_regen_needed.write().await = false;
            self.dirty_predicates.write().await.clear();
            return Ok(());
        }

        // Get dirty predicates
        let dirty: HashSet<String> = {
            let mut dirty_guard = self.dirty_predicates.write().await;
            std::mem::take(&mut *dirty_guard)
        };

        if dirty.is_empty() {
            return Ok(());
        }

        debug!(predicates = ?dirty, "flushing dirty predicates");

        // Get facts and arities
        let facts = self.facts_by_rkey.read().await;
        let arities = self.predicate_arities.read().await;

        for predicate in dirty {
            if let Some(&arity) = arities.get(&predicate) {
                self.regenerate_predicate_files(&predicate, arity, &facts)?;
            }
        }

        Ok(())
    }

    /// Regenerate TSV files for a single predicate.
    fn regenerate_predicate_files(
        &self,
        predicate: &str,
        arity: usize,
        facts: &HashMap<String, CachedFactData>,
    ) -> Result<(), DatalogError> {
        // Current facts file (non-superseded only)
        let current_path = self.fact_dir.join(format!("{}.facts", predicate));
        let mut current_file = std::fs::File::create(&current_path)?;

        // All facts file (with rkey prefix)
        let all_path = self.fact_dir.join(format!("_all_{}.facts", predicate));
        let mut all_file = std::fs::File::create(&all_path)?;

        for (rkey, data) in facts.iter() {
            if data.fact.predicate != predicate {
                continue;
            }

            let args = data.fact.args.join("\t");

            // Write to all file (always)
            writeln!(all_file, "{}\t{}", rkey, args)?;

            // Write to current file (only if not superseded)
            if !data.is_superseded {
                writeln!(current_file, "{}", args)?;
            }
        }

        trace!(predicate, arity, "regenerated predicate files");
        Ok(())
    }

    /// Regenerate all TSV files from scratch.
    async fn regenerate_all_files(&self) -> Result<(), DatalogError> {
        let facts = self.facts_by_rkey.read().await;
        let arities = self.predicate_arities.read().await;

        // Clear and regenerate metadata files
        let mut fact_file = std::fs::File::create(self.fact_dir.join("_fact.facts"))?;
        let mut confidence_file = std::fs::File::create(self.fact_dir.join("_confidence.facts"))?;
        let mut source_file = std::fs::File::create(self.fact_dir.join("_source.facts"))?;
        let mut supersedes_file = std::fs::File::create(self.fact_dir.join("_supersedes.facts"))?;

        // Group facts by predicate for efficient file writing
        let mut by_predicate: HashMap<&str, Vec<(&str, &CachedFactData)>> = HashMap::new();

        for (rkey, data) in facts.iter() {
            by_predicate
                .entry(&data.fact.predicate)
                .or_default()
                .push((rkey.as_str(), data));

            // Write metadata
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
        }

        // Write predicate files
        for (predicate, facts_for_pred) in by_predicate {
            if let Some(&_arity) = arities.get(predicate) {
                let current_path = self.fact_dir.join(format!("{}.facts", predicate));
                let all_path = self.fact_dir.join(format!("_all_{}.facts", predicate));

                let mut current_file = std::fs::File::create(&current_path)?;
                let mut all_file = std::fs::File::create(&all_path)?;

                for (rkey, data) in facts_for_pred {
                    let args = data.fact.args.join("\t");

                    writeln!(all_file, "{}\t{}", rkey, args)?;

                    if !data.is_superseded {
                        writeln!(current_file, "{}", args)?;
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

        // Generate input declarations from arities
        let arities = self.predicate_arities.read().await;
        let (input_decls, input_declared) = generate_input_declarations_from_arities(&arities);
        program.push_str(&input_decls);
        declared_predicates.extend(input_declared);

        // Generate derived declarations from rules (skip already-declared input predicates)
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
         .decl _confidence(rkey: symbol, value: float)\n\
         .input _confidence\n\n\
         .decl _source(rkey: symbol, source_cid: symbol)\n\
         .input _source\n\n\
         .decl _supersedes(new_rkey: symbol, old_rkey: symbol)\n\
         .input _supersedes\n\n",
    );
    declared_set.insert("_fact".to_string());
    declared_set.insert("_confidence".to_string());
    declared_set.insert("_source".to_string());
    declared_set.insert("_supersedes".to_string());

    // User predicates (current facts only) and _all_{predicate} (all facts with rkey)
    for (predicate, &arity) in arities {
        // Current predicate (no rkey prefix)
        let params: Vec<String> = (0..arity).map(|i| format!("arg{}: symbol", i)).collect();
        declarations.push_str(&format!(
            ".decl {}({})\n.input {}\n\n",
            predicate,
            params.join(", "),
            predicate
        ));
        declared_set.insert(predicate.clone());

        // _all_{predicate} (with rkey prefix)
        let all_params: Vec<String> = std::iter::once("rkey: symbol".to_string())
            .chain((0..arity).map(|i| format!("arg{}: symbol", i)))
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

/// Parse a query predicate to extract the name and arity.
/// e.g., "mutual_follow(X, Y)" -> ("mutual_follow", 2)
fn parse_query_predicate(query: &str) -> (String, usize) {
    if let Some(paren_idx) = query.find('(') {
        let name = query[..paren_idx].trim().to_string();
        let args_part = &query[paren_idx..];

        // Count arguments by counting commas + 1
        let arity = if args_part.contains(',') {
            args_part.matches(',').count() + 1
        } else if args_part.contains("()") || args_part.trim() == "()" {
            0
        } else {
            1
        };

        (name, arity)
    } else {
        // No parentheses, assume it's just a predicate name with arity 0
        (query.trim().to_string(), 0)
    }
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
    fn test_parse_query_predicate() {
        let (name, arity) = parse_query_predicate("mutual_follow(X, Y)");
        assert_eq!(name, "mutual_follow");
        assert_eq!(arity, 2);

        let (name, arity) = parse_query_predicate("is_active(X)");
        assert_eq!(name, "is_active");
        assert_eq!(arity, 1);

        let (name, arity) = parse_query_predicate("has_data()");
        assert_eq!(name, "has_data");
        assert_eq!(arity, 0);
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

        // Should have user predicates
        assert!(decls.contains(".decl follows(arg0: symbol, arg1: symbol)"));
        assert!(decls.contains(".input follows"));
        assert!(decls.contains(".decl _all_follows(rkey: symbol, arg0: symbol, arg1: symbol)"));

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
}
