//! Extract facts from ATProto records to TSV format.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use winter_atproto::{AtUri, Fact, ListRecordItem};

use crate::DatalogError;
use crate::cache::CachedFactData;

/// Result of extracting facts to TSV files.
pub struct ExtractResult {
    /// User-defined predicates (e.g., "follows", "interested_in").
    pub predicates: Vec<String>,
    /// Metadata relation names that were generated.
    pub meta_relations: Vec<&'static str>,
}

/// Extracts facts from ATProto records to TSV files for Soufflé.
pub struct FactExtractor;

impl FactExtractor {
    /// Extract facts to a directory of TSV files.
    ///
    /// Generates:
    /// - `{predicate}.facts` - current facts only (superseded facts excluded)
    /// - `_all_{predicate}.facts` - all facts with rkey prefix
    /// - `_fact.facts` - base relation (rkey, predicate, cid)
    /// - `_confidence.facts` - sparse, only non-1.0 values
    /// - `_source.facts` - sparse, only facts with source set
    /// - `_supersedes.facts` - (new_rkey, old_rkey) supersession links
    pub fn extract_to_dir(
        facts: &[ListRecordItem<Fact>],
        output_dir: &Path,
    ) -> Result<ExtractResult, DatalogError> {
        use std::fs::File;

        // Build CID -> rkey map for supersession lookups
        let cid_to_rkey: HashMap<&str, &str> = facts
            .iter()
            .map(|item| (item.cid.as_str(), AtUri::extract_rkey(&item.uri)))
            .collect();

        // Collect superseded CIDs (facts that have been replaced)
        let superseded_cids: HashSet<&str> = facts
            .iter()
            .filter_map(|item| item.value.supersedes.as_deref())
            .collect();

        // Track files for each predicate (current facts only)
        let mut current_files: HashMap<String, File> = HashMap::new();
        // Track files for _all_{predicate} (all facts with rkey)
        let mut all_files: HashMap<String, File> = HashMap::new();
        let mut predicates = Vec::new();

        // Metadata relation files
        let mut fact_file = File::create(output_dir.join("_fact.facts"))?;
        let mut confidence_file = File::create(output_dir.join("_confidence.facts"))?;
        let mut source_file = File::create(output_dir.join("_source.facts"))?;
        let mut supersedes_file = File::create(output_dir.join("_supersedes.facts"))?;
        let mut created_at_file = File::create(output_dir.join("_created_at.facts"))?;

        for item in facts {
            let fact = &item.value;
            let predicate = &fact.predicate;
            let rkey = AtUri::extract_rkey(&item.uri);
            let cid = &item.cid;
            let args = fact.args.join("\t");
            let is_current = !superseded_cids.contains(cid.as_str());

            // Ensure predicate files exist
            if !current_files.contains_key(predicate) {
                let current_path = output_dir.join(format!("{}.facts", predicate));
                let all_path = output_dir.join(format!("_all_{}.facts", predicate));
                current_files.insert(predicate.clone(), File::create(&current_path)?);
                all_files.insert(predicate.clone(), File::create(&all_path)?);
                predicates.push(predicate.clone());
            }

            // Write to current predicate file (only non-superseded facts)
            // Format: args..., rkey (rkey at end)
            if is_current {
                let file = current_files.get_mut(predicate).unwrap();
                writeln!(file, "{}\t{}", args, rkey)?;
            }

            // Write to _all_{predicate} file (all facts with rkey at end)
            // Format: args..., rkey (rkey at end, same as current)
            let all_file = all_files.get_mut(predicate).unwrap();
            writeln!(all_file, "{}\t{}", args, rkey)?;

            // Write to _fact.facts (rkey, predicate, cid)
            writeln!(fact_file, "{}\t{}\t{}", rkey, predicate, cid)?;

            // Write to _confidence.facts (sparse - only non-1.0 values)
            if let Some(conf) = fact.confidence
                && (conf - 1.0).abs() > f64::EPSILON
            {
                writeln!(confidence_file, "{}\t{}", rkey, conf)?;
            }

            // Write to _source.facts (sparse - only if set)
            if let Some(ref source) = fact.source {
                writeln!(source_file, "{}\t{}", rkey, source)?;
            }

            // Write to _supersedes.facts (new_rkey, old_rkey)
            if let Some(ref old_cid) = fact.supersedes
                && let Some(old_rkey) = cid_to_rkey.get(old_cid.as_str())
            {
                writeln!(supersedes_file, "{}\t{}", rkey, old_rkey)?;
            }

            // Write to _created_at.facts (dense - every fact)
            writeln!(
                created_at_file,
                "{}\t{}",
                rkey,
                fact.created_at.to_rfc3339()
            )?;
        }

        Ok(ExtractResult {
            predicates,
            meta_relations: vec![
                "_fact",
                "_confidence",
                "_source",
                "_supersedes",
                "_created_at",
            ],
        })
    }

    /// Generate input declarations for Soufflé based on facts.
    ///
    /// Returns a tuple of (declarations string, set of declared predicate names).
    pub fn generate_input_declarations(
        facts: &[ListRecordItem<Fact>],
    ) -> (String, HashSet<String>) {
        // Determine arity for each predicate
        let mut arities: HashMap<&str, usize> = HashMap::new();
        for item in facts {
            let fact = &item.value;
            arities
                .entry(&fact.predicate)
                .or_insert_with(|| fact.args.len());
        }

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
             .input _created_at\n\n",
        );
        declared_set.insert("_fact".to_string());
        declared_set.insert("_confidence".to_string());
        declared_set.insert("_source".to_string());
        declared_set.insert("_supersedes".to_string());
        declared_set.insert("_created_at".to_string());

        // User predicates (current facts only) and _all_{predicate} (all facts with rkey at end)
        for (predicate, arity) in arities {
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
            declared_set.insert(predicate.to_string());

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

    /// Generate input declarations from cached arities (no facts scan needed).
    ///
    /// This is an optimized version that uses pre-computed arities instead of
    /// scanning all facts.
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
             .input _created_at\n\n",
        );
        declared_set.insert("_fact".to_string());
        declared_set.insert("_confidence".to_string());
        declared_set.insert("_source".to_string());
        declared_set.insert("_supersedes".to_string());
        declared_set.insert("_created_at".to_string());

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

    /// Regenerate TSV files for a single predicate.
    ///
    /// This is an incremental update method that only regenerates the files
    /// for one predicate, avoiding full fact extraction.
    pub fn regenerate_predicate_files<'a>(
        output_dir: &Path,
        predicate: &str,
        facts: impl Iterator<Item = (&'a str, &'a CachedFactData)>,
    ) -> Result<(), DatalogError> {
        use std::fs::File;

        // Current facts file (non-superseded only)
        let current_path = output_dir.join(format!("{}.facts", predicate));
        let mut current_file = File::create(&current_path)?;

        // All facts file (with rkey prefix)
        let all_path = output_dir.join(format!("_all_{}.facts", predicate));
        let mut all_file = File::create(&all_path)?;

        for (rkey, data) in facts {
            if data.fact.predicate != predicate {
                continue;
            }

            let args = data.fact.args.join("\t");

            // Write to all file (always, rkey at end)
            writeln!(all_file, "{}\t{}", args, rkey)?;

            // Write to current file (only if not superseded, rkey at end)
            if !data.is_superseded {
                writeln!(current_file, "{}\t{}", args, rkey)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::tempdir;

    fn make_fact(predicate: &str, args: Vec<&str>) -> ListRecordItem<Fact> {
        make_fact_with_meta(predicate, args, None, None, None, "test-cid")
    }

    fn make_fact_with_meta(
        predicate: &str,
        args: Vec<&str>,
        confidence: Option<f64>,
        source: Option<&str>,
        supersedes: Option<&str>,
        cid: &str,
    ) -> ListRecordItem<Fact> {
        let rkey = format!("rkey-{}", cid);
        ListRecordItem {
            uri: format!("at://did:test/diy.razorgirl.winter.fact/{}", rkey),
            cid: cid.to_string(),
            value: Fact {
                predicate: predicate.to_string(),
                args: args.into_iter().map(String::from).collect(),
                confidence,
                source: source.map(String::from),
                supersedes: supersedes.map(String::from),
                tags: vec![],
                created_at: Utc::now(),
            },
        }
    }

    #[test]
    fn test_extract_to_dir_basic() {
        let dir = tempdir().unwrap();
        let facts = vec![
            make_fact("follows", vec!["did:a", "did:b"]),
            make_fact("follows", vec!["did:b", "did:c"]),
            make_fact("interested_in", vec!["did:a", "rust"]),
        ];

        let result = FactExtractor::extract_to_dir(&facts, dir.path()).unwrap();
        assert_eq!(result.predicates.len(), 2);
        assert!(result.predicates.contains(&"follows".to_string()));
        assert!(result.predicates.contains(&"interested_in".to_string()));
        assert_eq!(
            result.meta_relations,
            vec![
                "_fact",
                "_confidence",
                "_source",
                "_supersedes",
                "_created_at"
            ]
        );

        // Check current facts file
        let follows = std::fs::read_to_string(dir.path().join("follows.facts")).unwrap();
        assert!(follows.contains("did:a\tdid:b"));
        assert!(follows.contains("did:b\tdid:c"));

        // Check _all_ files exist
        assert!(dir.path().join("_all_follows.facts").exists());
        assert!(dir.path().join("_all_interested_in.facts").exists());

        // Check metadata files exist
        assert!(dir.path().join("_fact.facts").exists());
        assert!(dir.path().join("_confidence.facts").exists());
        assert!(dir.path().join("_source.facts").exists());
        assert!(dir.path().join("_supersedes.facts").exists());
    }

    #[test]
    fn test_superseded_facts_excluded_from_current() {
        let dir = tempdir().unwrap();

        // Old fact (will be superseded)
        let old_fact = make_fact_with_meta(
            "follows",
            vec!["did:a", "did:b"],
            Some(0.5),
            None,
            None,
            "cid-old",
        );

        // New fact supersedes old
        let new_fact = make_fact_with_meta(
            "follows",
            vec!["did:a", "did:c"],
            None,
            None,
            Some("cid-old"), // supersedes old fact
            "cid-new",
        );

        let facts = vec![old_fact, new_fact];
        let result = FactExtractor::extract_to_dir(&facts, dir.path()).unwrap();
        assert_eq!(result.predicates, vec!["follows"]);

        // Current file should only have the new fact
        let current = std::fs::read_to_string(dir.path().join("follows.facts")).unwrap();
        assert!(!current.contains("did:a\tdid:b")); // old fact excluded
        assert!(current.contains("did:a\tdid:c")); // new fact included

        // _all_ file should have both
        let all = std::fs::read_to_string(dir.path().join("_all_follows.facts")).unwrap();
        assert!(all.contains("did:a\tdid:b")); // old fact included
        assert!(all.contains("did:a\tdid:c")); // new fact included
    }

    #[test]
    fn test_confidence_sparse_output() {
        let dir = tempdir().unwrap();

        let facts = vec![
            // Default confidence (1.0) - should NOT appear in _confidence.facts
            make_fact_with_meta("follows", vec!["did:a", "did:b"], None, None, None, "cid1"),
            // Explicit 1.0 confidence - should NOT appear
            make_fact_with_meta(
                "follows",
                vec!["did:b", "did:c"],
                Some(1.0),
                None,
                None,
                "cid2",
            ),
            // Non-1.0 confidence - SHOULD appear
            make_fact_with_meta(
                "follows",
                vec!["did:c", "did:d"],
                Some(0.7),
                None,
                None,
                "cid3",
            ),
        ];

        FactExtractor::extract_to_dir(&facts, dir.path()).unwrap();

        let confidence = std::fs::read_to_string(dir.path().join("_confidence.facts")).unwrap();
        assert!(!confidence.contains("rkey-cid1")); // default 1.0
        assert!(!confidence.contains("rkey-cid2")); // explicit 1.0
        assert!(confidence.contains("rkey-cid3\t0.7")); // non-1.0
    }

    #[test]
    fn test_source_sparse_output() {
        let dir = tempdir().unwrap();

        let facts = vec![
            // No source - should NOT appear
            make_fact_with_meta("follows", vec!["did:a", "did:b"], None, None, None, "cid1"),
            // With source - SHOULD appear
            make_fact_with_meta(
                "follows",
                vec!["did:b", "did:c"],
                None,
                Some("source-cid-ref"),
                None,
                "cid2",
            ),
        ];

        FactExtractor::extract_to_dir(&facts, dir.path()).unwrap();

        let source = std::fs::read_to_string(dir.path().join("_source.facts")).unwrap();
        assert!(!source.contains("rkey-cid1"));
        assert!(source.contains("rkey-cid2\tsource-cid-ref"));
    }

    #[test]
    fn test_supersedes_relation() {
        let dir = tempdir().unwrap();

        let facts = vec![
            make_fact_with_meta(
                "follows",
                vec!["did:a", "did:b"],
                None,
                None,
                None,
                "cid-old",
            ),
            make_fact_with_meta(
                "follows",
                vec!["did:a", "did:c"],
                None,
                None,
                Some("cid-old"),
                "cid-new",
            ),
        ];

        FactExtractor::extract_to_dir(&facts, dir.path()).unwrap();

        let supersedes = std::fs::read_to_string(dir.path().join("_supersedes.facts")).unwrap();
        // new_rkey -> old_rkey
        assert!(supersedes.contains("rkey-cid-new\trkey-cid-old"));
    }

    #[test]
    fn test_fact_relation() {
        let dir = tempdir().unwrap();

        let facts = vec![
            make_fact_with_meta("follows", vec!["did:a", "did:b"], None, None, None, "cid1"),
            make_fact_with_meta(
                "interested_in",
                vec!["did:a", "rust"],
                None,
                None,
                None,
                "cid2",
            ),
        ];

        FactExtractor::extract_to_dir(&facts, dir.path()).unwrap();

        let fact_rel = std::fs::read_to_string(dir.path().join("_fact.facts")).unwrap();
        assert!(fact_rel.contains("rkey-cid1\tfollows\tcid1"));
        assert!(fact_rel.contains("rkey-cid2\tinterested_in\tcid2"));
    }

    #[test]
    fn test_generate_input_declarations() {
        let facts = vec![
            make_fact("follows", vec!["did:a", "did:b"]),
            make_fact("interested_in", vec!["did:a", "rust"]),
        ];

        let (decls, declared) = FactExtractor::generate_input_declarations(&facts);

        // Metadata relations
        assert!(decls.contains(".decl _fact(rkey: symbol, predicate: symbol, cid: symbol)"));
        assert!(decls.contains(".input _fact"));
        assert!(decls.contains(".decl _confidence(rkey: symbol, value: symbol)"));
        assert!(decls.contains(".decl _source(rkey: symbol, source_cid: symbol)"));
        assert!(decls.contains(".decl _supersedes(new_rkey: symbol, old_rkey: symbol)"));

        // User predicates (current, with rkey suffix)
        assert!(decls.contains(".decl follows(arg0: symbol, arg1: symbol, rkey: symbol)"));
        assert!(decls.contains(".input follows"));
        assert!(decls.contains(".decl interested_in(arg0: symbol, arg1: symbol, rkey: symbol)"));

        // _all_ predicates (with rkey at end, same format as current)
        assert!(decls.contains(".decl _all_follows(arg0: symbol, arg1: symbol, rkey: symbol)"));
        assert!(decls.contains(".input _all_follows"));
        assert!(
            decls.contains(".decl _all_interested_in(arg0: symbol, arg1: symbol, rkey: symbol)")
        );

        // _created_at declaration
        assert!(decls.contains(".decl _created_at(rkey: symbol, timestamp: symbol)"));
        assert!(decls.contains(".input _created_at"));

        // Check declared set
        assert!(declared.contains("_fact"));
        assert!(declared.contains("_confidence"));
        assert!(declared.contains("_source"));
        assert!(declared.contains("_supersedes"));
        assert!(declared.contains("_created_at"));
        assert!(declared.contains("follows"));
        assert!(declared.contains("_all_follows"));
        assert!(declared.contains("interested_in"));
        assert!(declared.contains("_all_interested_in"));
    }

    #[test]
    fn test_all_files_include_rkey_suffix() {
        let dir = tempdir().unwrap();

        let facts = vec![make_fact_with_meta(
            "follows",
            vec!["did:a", "did:b"],
            None,
            None,
            None,
            "my-cid",
        )];

        FactExtractor::extract_to_dir(&facts, dir.path()).unwrap();

        let all = std::fs::read_to_string(dir.path().join("_all_follows.facts")).unwrap();
        // Format: arg0\targ1\trkey (rkey at end)
        assert!(all.contains("did:a\tdid:b\trkey-my-cid"));
    }

    #[test]
    fn test_created_at_dense_output() {
        let dir = tempdir().unwrap();

        let facts = vec![
            make_fact_with_meta("follows", vec!["did:a", "did:b"], None, None, None, "cid1"),
            make_fact_with_meta("follows", vec!["did:b", "did:c"], None, None, None, "cid2"),
            make_fact_with_meta(
                "interested_in",
                vec!["did:a", "rust"],
                None,
                None,
                None,
                "cid3",
            ),
        ];

        FactExtractor::extract_to_dir(&facts, dir.path()).unwrap();

        let created_at = std::fs::read_to_string(dir.path().join("_created_at.facts")).unwrap();
        // Every fact should have an entry (dense relation)
        assert!(created_at.contains("rkey-cid1\t"));
        assert!(created_at.contains("rkey-cid2\t"));
        assert!(created_at.contains("rkey-cid3\t"));

        // Verify ISO8601 format (contains 'T' separator and ends with 'Z' or offset)
        for line in created_at.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            assert_eq!(parts.len(), 2);
            let timestamp = parts[1];
            assert!(
                timestamp.contains('T'),
                "timestamp should be ISO8601 format"
            );
        }
    }
}
