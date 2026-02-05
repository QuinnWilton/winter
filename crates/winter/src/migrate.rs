//! Migration framework for Winter data transformations.
//!
//! This module provides a general-purpose migration framework that supports:
//! - `--dry-run` mode to preview changes without applying them
//! - Multiple named migrations that can be run independently
//! - Extensible design for future data migrations

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::Utc;
use miette::Result;
use tracing::info;

use winter_atproto::{
    AtUri, AtprotoClient, DIRECTIVE_COLLECTION, Directive, DirectiveKind, FACT_COLLECTION, Fact,
    IDENTITY_COLLECTION, IDENTITY_KEY, Identity, LegacyIdentity, NOTE_COLLECTION, Note,
    RULE_COLLECTION, Rule, Tid,
};
use winter_datalog::DerivedFactGenerator;

// =============================================================================
// Migration Framework Types
// =============================================================================

/// Preview of what a migration would change.
pub struct MigrationPreview {
    /// Number of records that would be updated.
    pub records_to_update: usize,
    /// Human-readable descriptions of changes.
    pub changes: Vec<String>,
}

/// Result of applying a migration.
pub struct MigrationResult {
    /// Number of records that were updated.
    pub records_updated: usize,
    /// Errors encountered (non-fatal, migration continued).
    pub errors: Vec<String>,
}

/// A migration that can be applied to the PDS.
#[async_trait]
pub trait Migration: Send + Sync {
    /// Unique name for this migration.
    fn name(&self) -> &'static str;

    /// Human-readable description.
    fn description(&self) -> &'static str;

    /// Check if this migration needs to be applied.
    async fn needs_migration(&self, client: &AtprotoClient) -> Result<bool>;

    /// Preview what would change (dry-run).
    async fn preview(&self, client: &AtprotoClient) -> Result<MigrationPreview>;

    /// Apply the migration.
    async fn apply(&self, client: &AtprotoClient) -> Result<MigrationResult>;
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Extract the rkey from an AT URI.
fn extract_rkey(uri: &str) -> String {
    AtUri::extract_rkey(uri).to_string()
}

/// Check if a reference value needs conversion to AT URI format.
fn needs_conversion(value: &str) -> bool {
    !value.is_empty() && !value.starts_with("at://")
}

/// Build maps from CID and rkey to AT URI for all records of a collection.
async fn build_reference_maps(
    client: &AtprotoClient,
    collection: &str,
) -> Result<(HashMap<String, String>, HashMap<String, String>)> {
    let records = client
        .list_all_records::<serde_json::Value>(collection)
        .await
        .map_err(|e| miette::miette!("{}", e))?;
    let mut cid_map = HashMap::new();
    let mut rkey_map = HashMap::new();

    for record in records {
        let rkey = extract_rkey(&record.uri);
        cid_map.insert(record.cid.clone(), record.uri.clone());
        rkey_map.insert(rkey, record.uri.clone());
    }

    Ok((cid_map, rkey_map))
}

// =============================================================================
// Migration: Fact References to URIs
// =============================================================================

/// Migration: Convert fact source/supersedes from CID to AT URI.
struct FactReferencesToUris;

#[async_trait]
impl Migration for FactReferencesToUris {
    fn name(&self) -> &'static str {
        "fact-references-to-uris"
    }

    fn description(&self) -> &'static str {
        "Convert fact source and supersedes fields from CID to AT URI format"
    }

    async fn needs_migration(&self, client: &AtprotoClient) -> Result<bool> {
        let facts = client
            .list_all_records::<Fact>(FACT_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        for record in facts {
            if record
                .value
                .source
                .as_ref()
                .is_some_and(|s| needs_conversion(s))
            {
                return Ok(true);
            }
            if record
                .value
                .supersedes
                .as_ref()
                .is_some_and(|s| needs_conversion(s))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn preview(&self, client: &AtprotoClient) -> Result<MigrationPreview> {
        let facts = client
            .list_all_records::<Fact>(FACT_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;
        let mut changes = Vec::new();
        let mut count = 0;

        for record in facts {
            let rkey = extract_rkey(&record.uri);
            let needs_source = record
                .value
                .source
                .as_ref()
                .is_some_and(|s| needs_conversion(s));
            let needs_supersedes = record
                .value
                .supersedes
                .as_ref()
                .is_some_and(|s| needs_conversion(s));

            if needs_source || needs_supersedes {
                count += 1;
                let fields: Vec<&str> = [
                    needs_source.then_some("source"),
                    needs_supersedes.then_some("supersedes"),
                ]
                .into_iter()
                .flatten()
                .collect();
                changes.push(format!(
                    "Fact {} ({}): convert {} to AT URI",
                    record.value.predicate,
                    rkey,
                    fields.join(", ")
                ));
            }
        }

        Ok(MigrationPreview {
            records_to_update: count,
            changes,
        })
    }

    async fn apply(&self, client: &AtprotoClient) -> Result<MigrationResult> {
        let (cid_map, _) = build_reference_maps(client, FACT_COLLECTION).await?;
        let facts = client
            .list_all_records::<Fact>(FACT_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;
        let mut updated = 0;
        let mut errors = Vec::new();

        for record in facts {
            let rkey = extract_rkey(&record.uri);
            let mut fact = record.value;
            let mut changed = false;

            if let Some(ref source) = fact.source
                && needs_conversion(source)
            {
                if let Some(uri) = cid_map.get(source) {
                    fact.source = Some(uri.clone());
                    changed = true;
                } else {
                    errors.push(format!(
                        "Fact {}: Could not resolve source CID {}",
                        rkey, source
                    ));
                }
            }

            if let Some(ref supersedes) = fact.supersedes
                && needs_conversion(supersedes)
            {
                if let Some(uri) = cid_map.get(supersedes) {
                    fact.supersedes = Some(uri.clone());
                    changed = true;
                } else {
                    errors.push(format!(
                        "Fact {}: Could not resolve supersedes CID {}",
                        rkey, supersedes
                    ));
                }
            }

            if changed {
                client
                    .put_record(FACT_COLLECTION, &rkey, &fact)
                    .await
                    .map_err(|e| miette::miette!("{}", e))?;
                updated += 1;
            }
        }

        Ok(MigrationResult {
            records_updated: updated,
            errors,
        })
    }
}

// =============================================================================
// Migration: Directive Supersedes to URIs
// =============================================================================

/// Migration: Convert directive supersedes from rkey to AT URI.
struct DirectiveSupersedesToUris;

#[async_trait]
impl Migration for DirectiveSupersedesToUris {
    fn name(&self) -> &'static str {
        "directive-supersedes-to-uris"
    }

    fn description(&self) -> &'static str {
        "Convert directive supersedes field from rkey to AT URI format"
    }

    async fn needs_migration(&self, client: &AtprotoClient) -> Result<bool> {
        let directives = client
            .list_all_records::<Directive>(DIRECTIVE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        for record in directives {
            if record
                .value
                .supersedes
                .as_ref()
                .is_some_and(|s| needs_conversion(s))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn preview(&self, client: &AtprotoClient) -> Result<MigrationPreview> {
        let directives = client
            .list_all_records::<Directive>(DIRECTIVE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;
        let mut changes = Vec::new();
        let mut count = 0;

        for record in directives {
            if record
                .value
                .supersedes
                .as_ref()
                .is_some_and(|s| needs_conversion(s))
            {
                count += 1;
                let rkey = extract_rkey(&record.uri);
                changes.push(format!(
                    "Directive {:?} ({}): convert supersedes '{}' to AT URI",
                    record.value.kind,
                    rkey,
                    record.value.supersedes.as_ref().unwrap()
                ));
            }
        }

        Ok(MigrationPreview {
            records_to_update: count,
            changes,
        })
    }

    async fn apply(&self, client: &AtprotoClient) -> Result<MigrationResult> {
        let (_, rkey_map) = build_reference_maps(client, DIRECTIVE_COLLECTION).await?;
        let did = client
            .did()
            .await
            .ok_or_else(|| miette::miette!("not authenticated"))?;
        let directives = client
            .list_all_records::<Directive>(DIRECTIVE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;
        let mut updated = 0;
        let errors = Vec::new();

        for record in directives {
            let rkey = extract_rkey(&record.uri);
            let mut directive = record.value;

            if let Some(ref supersedes) = directive.supersedes
                && needs_conversion(supersedes)
            {
                // Try rkey_map first, then construct URI directly
                let uri = rkey_map.get(supersedes).cloned().unwrap_or_else(|| {
                    format!("at://{}/{}/{}", did, DIRECTIVE_COLLECTION, supersedes)
                });
                directive.supersedes = Some(uri);
                directive.last_updated = Some(Utc::now());
                client
                    .put_record(DIRECTIVE_COLLECTION, &rkey, &directive)
                    .await
                    .map_err(|e| miette::miette!("{}", e))?;
                updated += 1;
            }
        }

        Ok(MigrationResult {
            records_updated: updated,
            errors,
        })
    }
}

// =============================================================================
// Migration: Note RelatedFacts to URIs
// =============================================================================

/// Migration: Convert note relatedFacts from CIDs to AT URIs.
struct NoteRelatedFactsToUris;

#[async_trait]
impl Migration for NoteRelatedFactsToUris {
    fn name(&self) -> &'static str {
        "note-related-facts-to-uris"
    }

    fn description(&self) -> &'static str {
        "Convert note relatedFacts from CID format to AT URI format"
    }

    async fn needs_migration(&self, client: &AtprotoClient) -> Result<bool> {
        let notes = client
            .list_all_records::<Note>(NOTE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        for record in notes {
            for rf in &record.value.related_facts {
                if needs_conversion(rf) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    async fn preview(&self, client: &AtprotoClient) -> Result<MigrationPreview> {
        let notes = client
            .list_all_records::<Note>(NOTE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;
        let mut changes = Vec::new();
        let mut count = 0;

        for record in notes {
            let cid_refs: Vec<_> = record
                .value
                .related_facts
                .iter()
                .filter(|rf| needs_conversion(rf))
                .collect();
            if !cid_refs.is_empty() {
                count += 1;
                let rkey = extract_rkey(&record.uri);
                changes.push(format!(
                    "Note '{}' ({}): {} CID-format relatedFacts to convert",
                    record.value.title,
                    rkey,
                    cid_refs.len()
                ));
            }
        }

        Ok(MigrationPreview {
            records_to_update: count,
            changes,
        })
    }

    async fn apply(&self, client: &AtprotoClient) -> Result<MigrationResult> {
        let (cid_map, _) = build_reference_maps(client, FACT_COLLECTION).await?;
        let notes = client
            .list_all_records::<Note>(NOTE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;
        let mut updated = 0;
        let mut errors = Vec::new();

        for record in notes {
            let rkey = extract_rkey(&record.uri);
            let mut note = record.value;
            let mut changed = false;

            for rf in &mut note.related_facts {
                if needs_conversion(rf) {
                    if let Some(uri) = cid_map.get(rf.as_str()) {
                        *rf = uri.clone();
                        changed = true;
                    } else {
                        errors.push(format!(
                            "Note '{}': Could not resolve CID {}",
                            note.title, rf
                        ));
                    }
                }
            }

            if changed {
                note.last_updated = Utc::now();
                client
                    .put_record(NOTE_COLLECTION, &rkey, &note)
                    .await
                    .map_err(|e| miette::miette!("{}", e))?;
                updated += 1;
            }
        }

        Ok(MigrationResult {
            records_updated: updated,
            errors,
        })
    }
}

// =============================================================================
// Migration: Legacy Identity to Directives
// =============================================================================

/// Migration: Convert legacy identity (values, interests, selfDescription) to directives.
struct LegacyIdentityToDirectives;

#[async_trait]
impl Migration for LegacyIdentityToDirectives {
    fn name(&self) -> &'static str {
        "legacy-identity-to-directives"
    }

    fn description(&self) -> &'static str {
        "Convert legacy identity format (values, interests, selfDescription) to directive records"
    }

    async fn needs_migration(&self, client: &AtprotoClient) -> Result<bool> {
        // Try to load as legacy format
        match client
            .get_record::<LegacyIdentity>(IDENTITY_COLLECTION, IDENTITY_KEY)
            .await
        {
            Ok(record) => {
                let legacy = record.value;
                // If any legacy fields are populated, needs migration
                Ok(!legacy.values.is_empty()
                    || !legacy.interests.is_empty()
                    || !legacy.self_description.is_empty())
            }
            Err(winter_atproto::AtprotoError::NotFound { .. }) => Ok(false),
            Err(e) => Err(miette::miette!("Failed to check identity: {}", e)),
        }
    }

    async fn preview(&self, client: &AtprotoClient) -> Result<MigrationPreview> {
        let legacy = match client
            .get_record::<LegacyIdentity>(IDENTITY_COLLECTION, IDENTITY_KEY)
            .await
        {
            Ok(record) => record.value,
            Err(winter_atproto::AtprotoError::NotFound { .. }) => {
                return Ok(MigrationPreview {
                    records_to_update: 0,
                    changes: vec!["No identity record found".to_string()],
                });
            }
            Err(e) => return Err(miette::miette!("Failed to get identity: {}", e)),
        };

        let mut changes = Vec::new();

        if !legacy.self_description.is_empty() {
            changes.push("Create self_concept directive from selfDescription".to_string());
        }
        for value in &legacy.values {
            changes.push(format!("Create value directive: '{}'", value));
        }
        for interest in &legacy.interests {
            changes.push(format!("Create interest directive: '{}'", interest));
        }
        if !changes.is_empty() {
            changes.push("Update identity record to slim version".to_string());
        }

        let count = if changes.is_empty() { 0 } else { 1 }; // The identity record

        Ok(MigrationPreview {
            records_to_update: count,
            changes,
        })
    }

    async fn apply(&self, client: &AtprotoClient) -> Result<MigrationResult> {
        // Load existing identity (try as legacy format)
        let legacy = match client
            .get_record::<LegacyIdentity>(IDENTITY_COLLECTION, IDENTITY_KEY)
            .await
        {
            Ok(record) => record.value,
            Err(winter_atproto::AtprotoError::NotFound { .. }) => {
                return Err(miette::miette!(
                    "Identity not found. Run 'winter bootstrap' to create a new identity."
                ));
            }
            Err(e) => return Err(miette::miette!("Failed to get identity: {}", e)),
        };

        // Check if already migrated
        if legacy.values.is_empty()
            && legacy.interests.is_empty()
            && legacy.self_description.is_empty()
        {
            info!("identity appears to already be migrated (no legacy fields found)");
            return Ok(MigrationResult {
                records_updated: 0,
                errors: vec![],
            });
        }

        info!(
            values = legacy.values.len(),
            interests = legacy.interests.len(),
            has_self_description = !legacy.self_description.is_empty(),
            "found legacy identity data"
        );

        let now = Utc::now();
        let mut directives_created = 0;

        // Create self_concept directive from selfDescription
        if !legacy.self_description.is_empty() {
            let directive = Directive {
                kind: DirectiveKind::SelfConcept,
                content: legacy.self_description.clone(),
                summary: None,
                active: true,
                confidence: None,
                source: Some("migrated from legacy identity".to_string()),
                supersedes: None,
                tags: vec!["migrated".to_string()],
                priority: 0,
                created_at: now,
                last_updated: None,
            };
            let rkey = Tid::now().to_string();
            client
                .create_record(DIRECTIVE_COLLECTION, Some(&rkey), &directive)
                .await
                .map_err(|e| miette::miette!("{}", e))?;
            info!("created self_concept directive from selfDescription");
            directives_created += 1;
        }

        // Create value directives
        for value in &legacy.values {
            let directive = Directive {
                kind: DirectiveKind::Value,
                content: value.clone(),
                summary: None,
                active: true,
                confidence: None,
                source: Some("migrated from legacy identity".to_string()),
                supersedes: None,
                tags: vec!["migrated".to_string()],
                priority: 0,
                created_at: now,
                last_updated: None,
            };
            let rkey = Tid::now().to_string();
            client
                .create_record(DIRECTIVE_COLLECTION, Some(&rkey), &directive)
                .await
                .map_err(|e| miette::miette!("{}", e))?;
            info!(value = %value, "created value directive");
            directives_created += 1;
        }

        // Create interest directives
        for interest in &legacy.interests {
            let directive = Directive {
                kind: DirectiveKind::Interest,
                content: interest.clone(),
                summary: None,
                active: true,
                confidence: None,
                source: Some("migrated from legacy identity".to_string()),
                supersedes: None,
                tags: vec!["migrated".to_string()],
                priority: 0,
                created_at: now,
                last_updated: None,
            };
            let rkey = Tid::now().to_string();
            client
                .create_record(DIRECTIVE_COLLECTION, Some(&rkey), &directive)
                .await
                .map_err(|e| miette::miette!("{}", e))?;
            info!(interest = %interest, "created interest directive");
            directives_created += 1;
        }

        // Update identity to slim version
        let slim_identity = Identity {
            operator_did: legacy.operator_did,
            created_at: legacy.created_at,
            last_updated: now,
        };

        client
            .put_record(IDENTITY_COLLECTION, IDENTITY_KEY, &slim_identity)
            .await
            .map_err(|e| miette::miette!("{}", e))?;
        info!("updated identity record to slim version");

        info!(
            directives_created = directives_created,
            "migration complete"
        );

        Ok(MigrationResult {
            records_updated: directives_created + 1, // directives + identity record
            errors: vec![],
        })
    }
}

// =============================================================================
// Migration: Rule Predicate Arity (add rkey argument)
// =============================================================================

/// Migration: Update rule bodies to add rkey argument to predicates.
///
/// This migration adds `_` as the last argument to all predicates that now
/// include rkey. For example, `follows(X, Y)` becomes `follows(X, Y, _)`.
///
/// This includes:
/// - Derived predicates (follows, liked, has_value, etc.) except is_followed_by
/// - User-defined fact predicates
/// - `_all_<predicate>` variants for both
struct RulePredicateArityMigration;

impl RulePredicateArityMigration {
    /// Get derived predicates that need rkey added (all except is_followed_by).
    fn derived_predicates_with_arity() -> HashMap<String, usize> {
        DerivedFactGenerator::arities()
            .into_iter()
            .filter(|(name, _)| *name != "is_followed_by")
            .map(|(name, arity)| (name.to_string(), arity))
            .collect()
    }

    /// Build complete arity map including user-defined predicates and _all_ variants.
    fn build_arity_map(user_predicates: &HashMap<String, usize>) -> HashMap<String, usize> {
        let mut arities = Self::derived_predicates_with_arity();

        // Add _all_ variants for derived predicates (same arity)
        let all_variants: Vec<_> = arities
            .iter()
            .map(|(name, &arity)| (format!("_all_{}", name), arity))
            .collect();
        arities.extend(all_variants);

        // Add user-defined predicates
        arities.extend(user_predicates.clone());

        // Add _all_ variants for user-defined predicates
        let user_all_variants: Vec<_> = user_predicates
            .iter()
            .map(|(name, &arity)| (format!("_all_{}", name), arity))
            .collect();
        arities.extend(user_all_variants);

        arities
    }

    /// Check if a rule body clause references an affected predicate without rkey.
    fn clause_needs_update(clause: &str, arities: &HashMap<String, usize>) -> Option<String> {
        let clause = clause.trim();

        // Find the predicate name (before the opening paren)
        let paren_idx = clause.find('(')?;

        let predicate_name = clause[..paren_idx].trim();

        // Get expected arity (includes rkey)
        let expected_arity = *arities.get(predicate_name)?;

        // Count current arguments
        let close_paren = clause.rfind(')')?;

        let args_str = &clause[paren_idx + 1..close_paren];
        let current_arity = if args_str.trim().is_empty() {
            0
        } else {
            Self::count_args(args_str)
        };

        // If current arity is one less than expected, add _ for rkey
        if current_arity == expected_arity - 1 {
            // Insert _, before the closing paren
            let updated = format!(
                "{}{})",
                &clause[..close_paren],
                if args_str.trim().is_empty() {
                    "_"
                } else {
                    ", _"
                },
            );
            Some(updated)
        } else {
            None
        }
    }

    /// Count arguments in a predicate, handling nested parens and strings.
    fn count_args(args_str: &str) -> usize {
        if args_str.trim().is_empty() {
            return 0;
        }

        let mut count = 1;
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

    /// Update a rule's body clauses to add rkey where needed.
    fn update_rule_body(body: &[String], arities: &HashMap<String, usize>) -> (Vec<String>, bool) {
        let mut updated_body = Vec::new();
        let mut changed = false;

        for clause in body {
            if let Some(updated) = Self::clause_needs_update(clause, arities) {
                updated_body.push(updated);
                changed = true;
            } else {
                updated_body.push(clause.clone());
            }
        }

        (updated_body, changed)
    }

    /// Fetch user-defined predicates and their arities from facts in the PDS.
    async fn fetch_user_predicates(client: &AtprotoClient) -> Result<HashMap<String, usize>> {
        let facts = client
            .list_all_records::<Fact>(FACT_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        let mut predicates: HashMap<String, usize> = HashMap::new();

        for record in facts {
            let predicate = &record.value.predicate;
            // New arity = args count + 1 for rkey
            let arity = record.value.args.len() + 1;
            predicates.insert(predicate.clone(), arity);
        }

        Ok(predicates)
    }
}

#[async_trait]
impl Migration for RulePredicateArityMigration {
    fn name(&self) -> &'static str {
        "rule-predicate-arity"
    }

    fn description(&self) -> &'static str {
        "Update rule bodies to add rkey argument (as _) to predicates that now include it"
    }

    async fn needs_migration(&self, client: &AtprotoClient) -> Result<bool> {
        let user_predicates = Self::fetch_user_predicates(client).await?;
        let arities = Self::build_arity_map(&user_predicates);

        let rules = client
            .list_all_records::<Rule>(RULE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        for record in rules {
            for clause in &record.value.body {
                if Self::clause_needs_update(clause, &arities).is_some() {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    async fn preview(&self, client: &AtprotoClient) -> Result<MigrationPreview> {
        let user_predicates = Self::fetch_user_predicates(client).await?;
        let arities = Self::build_arity_map(&user_predicates);

        info!(
            derived_count = Self::derived_predicates_with_arity().len(),
            user_count = user_predicates.len(),
            "built arity map"
        );

        let rules = client
            .list_all_records::<Rule>(RULE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        let mut changes = Vec::new();
        let mut count = 0;

        for record in rules {
            let (updated_body, changed) = Self::update_rule_body(&record.value.body, &arities);
            if changed {
                count += 1;
                let rkey = extract_rkey(&record.uri);
                // Show which clauses changed
                let clause_changes: Vec<_> = record
                    .value
                    .body
                    .iter()
                    .zip(updated_body.iter())
                    .filter(|(old, new)| old != new)
                    .map(|(old, new)| format!("  {} → {}", old, new))
                    .collect();
                changes.push(format!(
                    "Rule '{}' ({}):\n{}",
                    record.value.name,
                    rkey,
                    clause_changes.join("\n")
                ));
            }
        }

        Ok(MigrationPreview {
            records_to_update: count,
            changes,
        })
    }

    async fn apply(&self, client: &AtprotoClient) -> Result<MigrationResult> {
        let user_predicates = Self::fetch_user_predicates(client).await?;
        let arities = Self::build_arity_map(&user_predicates);

        info!(
            derived_count = Self::derived_predicates_with_arity().len(),
            user_count = user_predicates.len(),
            total_predicates = arities.len(),
            "built arity map for migration"
        );

        let rules = client
            .list_all_records::<Rule>(RULE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        let mut updated = 0;
        let errors = Vec::new();

        for record in rules {
            let rkey = extract_rkey(&record.uri);
            let (new_body, changed) = Self::update_rule_body(&record.value.body, &arities);

            if changed {
                let mut rule = record.value;
                rule.body = new_body;

                client
                    .put_record(RULE_COLLECTION, &rkey, &rule)
                    .await
                    .map_err(|e| miette::miette!("{}", e))?;

                info!(rule = %rule.name, "updated rule body clauses");
                updated += 1;
            }
        }

        Ok(MigrationResult {
            records_updated: updated,
            errors,
        })
    }
}

// =============================================================================
// Migration Registry
// =============================================================================

/// Get all available migrations.
pub fn available_migrations() -> Vec<Box<dyn Migration>> {
    vec![
        Box::new(FactReferencesToUris),
        Box::new(DirectiveSupersedesToUris),
        Box::new(NoteRelatedFactsToUris),
        Box::new(LegacyIdentityToDirectives),
        Box::new(RulePredicateArityMigration),
    ]
}

// =============================================================================
// Command Handler
// =============================================================================

/// Run the migrate command with the given options.
pub async fn run_migrate_command(
    pds_url: &str,
    handle: &str,
    app_password: &str,
    migration_name: Option<&str>,
    list: bool,
    dry_run: bool,
    all: bool,
) -> Result<()> {
    let client = AtprotoClient::new(pds_url);
    client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    let migrations = available_migrations();

    if list {
        println!("Available migrations:\n");
        for m in &migrations {
            let needs = m.needs_migration(&client).await.unwrap_or(false);
            let status = if needs { "[PENDING]" } else { "[APPLIED]" };
            println!("  {} {}", status, m.name());
            println!("      {}\n", m.description());
        }
        return Ok(());
    }

    let to_run: Vec<_> = if all {
        // Run all pending migrations
        let mut pending = Vec::new();
        for m in migrations {
            if m.needs_migration(&client).await? {
                pending.push(m);
            }
        }
        pending
    } else if let Some(name) = migration_name {
        // Run specific migration
        let m = migrations
            .into_iter()
            .find(|m| m.name() == name)
            .ok_or_else(|| miette::miette!("Unknown migration: {}", name))?;
        vec![m]
    } else {
        return Err(miette::miette!(
            "Specify a migration name, --all, or --list"
        ));
    };

    if to_run.is_empty() {
        println!("No pending migrations to run.");
        return Ok(());
    }

    for m in to_run {
        println!("\n=== {} ===", m.name());
        println!("{}\n", m.description());

        if dry_run {
            let preview = m.preview(&client).await?;
            println!(
                "Dry-run: {} record(s) would be updated",
                preview.records_to_update
            );
            if !preview.changes.is_empty() {
                println!("\nChanges:");
                for change in &preview.changes {
                    println!("  - {}", change);
                }
            }
        } else {
            let result = m.apply(&client).await?;
            println!("Applied: {} record(s) updated", result.records_updated);
            for err in &result.errors {
                println!("  Warning: {}", err);
            }
        }
    }

    Ok(())
}

/// Run the legacy migrate-identity command.
///
/// This is kept for backwards compatibility with the old `winter migrate-identity` command.
pub async fn run(pds_url: &str, handle: &str, app_password: &str) -> Result<()> {
    run_migrate_command(
        pds_url,
        handle,
        app_password,
        Some("legacy-identity-to-directives"),
        false,
        false,
        false,
    )
    .await
}
