//! Predicate dependency analysis for lazy regeneration.
//!
//! Analyzes datalog rules and queries to determine which predicates are needed,
//! enabling lazy regeneration of only the required TSV files.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;

use winter_atproto::Rule;

/// Compiled regex for predicate extraction (cached).
fn predicate_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"([a-z_][a-z0-9_]*)\s*\(").unwrap())
}

/// Dependency graph for predicate relationships.
///
/// Tracks which predicates depend on which others, enabling computation
/// of the minimal set of predicates needed for a query.
#[derive(Debug)]
pub struct PredicateDependencyGraph {
    /// Predicate -> predicates it depends on (from rule bodies).
    dependencies: HashMap<String, HashSet<String>>,
    /// All predicates mentioned in rules.
    all_predicates: HashSet<String>,
}

impl PredicateDependencyGraph {
    /// Build a dependency graph from a set of rules.
    pub fn from_rules(rules: &[Rule]) -> Self {
        let mut dependencies: HashMap<String, HashSet<String>> = HashMap::new();
        let mut all_predicates = HashSet::new();

        for rule in rules {
            if !rule.enabled {
                continue;
            }

            // Parse the head to get the derived predicate
            if let Some(head_pred) = extract_predicate_name(&rule.head) {
                all_predicates.insert(head_pred.clone());

                // Parse the body (Vec<String>) to get dependencies
                for body_item in &rule.body {
                    let body_preds = extract_predicates_from_text(body_item);
                    for pred in &body_preds {
                        all_predicates.insert(pred.clone());
                    }
                    dependencies
                        .entry(head_pred.clone())
                        .or_default()
                        .extend(body_preds);
                }

                // Also parse constraints for additional predicates
                for constraint in &rule.constraints {
                    let constraint_preds = extract_predicates_from_text(constraint);
                    for pred in constraint_preds {
                        all_predicates.insert(pred);
                    }
                }
            }
        }

        Self {
            dependencies,
            all_predicates,
        }
    }

    /// Extract predicate names from a query string.
    ///
    /// Handles queries like:
    /// - `follows(X, Y, _)` -> {"follows"}
    /// - `mutual(X) :- follows(X, Y, _), is_followed_by(Y, X)` -> {"mutual", "follows", "is_followed_by"}
    pub fn extract_query_predicates(query: &str) -> HashSet<String> {
        extract_predicates_from_text(query)
    }

    /// Get all predicates required to evaluate a set of root predicates.
    ///
    /// Computes the transitive closure of dependencies.
    pub fn get_required_predicates(&self, roots: &HashSet<String>) -> HashSet<String> {
        let mut required = HashSet::new();
        let mut to_process: Vec<String> = roots.iter().cloned().collect();

        while let Some(pred) = to_process.pop() {
            if required.contains(&pred) {
                continue;
            }
            required.insert(pred.clone());

            // Add dependencies of this predicate
            if let Some(deps) = self.dependencies.get(&pred) {
                for dep in deps {
                    if !required.contains(dep) {
                        to_process.push(dep.clone());
                    }
                }
            }
        }

        required
    }

    /// Get all known predicates.
    pub fn all_predicates(&self) -> &HashSet<String> {
        &self.all_predicates
    }
}

/// Extract a predicate name from a rule head like `mutual(X)`.
fn extract_predicate_name(head: &str) -> Option<String> {
    let head = head.trim();
    if let Some(paren_idx) = head.find('(') {
        let name = head[..paren_idx].trim();
        if !name.is_empty() && is_valid_predicate_name(name) {
            return Some(name.to_string());
        }
    }
    None
}

/// Extract all predicate names from a text (rule body, query, etc).
fn extract_predicates_from_text(text: &str) -> HashSet<String> {
    let mut predicates = HashSet::new();

    for cap in predicate_regex().captures_iter(text) {
        let name = &cap[1];
        // Exclude Soufflé keywords and operators
        if is_valid_predicate_name(name) {
            predicates.insert(name.to_string());
        }
    }

    predicates
}

/// Check if a name is a valid predicate (not a Soufflé keyword).
fn is_valid_predicate_name(name: &str) -> bool {
    !matches!(
        name,
        "cat"
            | "ord"
            | "strlen"
            | "substr"
            | "to_string"
            | "to_number"
            | "match"
            | "contains"
            | "min"
            | "max"
            | "count"
            | "sum"
            | "mean"
            | "range"
            | "band"
            | "bor"
            | "bxor"
            | "bnot"
            | "bshl"
            | "bshr"
            | "bshru"
            | "land"
            | "lor"
            | "lxor"
            | "lnot"
    )
}

/// Metadata predicates that are always regenerated together.
pub const METADATA_PREDICATES: &[&str] = &[
    "_fact",
    "_confidence",
    "_source",
    "_supersedes",
    "_created_at",
    "_expires_at",
    "_validation_error",
];

/// Check if a predicate is a metadata predicate.
pub fn is_metadata_predicate(pred: &str) -> bool {
    METADATA_PREDICATES.contains(&pred)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_predicate_name() {
        assert_eq!(
            extract_predicate_name("mutual(X)"),
            Some("mutual".to_string())
        );
        assert_eq!(
            extract_predicate_name("follows(Self, Target, Rkey)"),
            Some("follows".to_string())
        );
        assert_eq!(
            extract_predicate_name("_all_follows(X, Y, R)"),
            Some("_all_follows".to_string())
        );
    }

    #[test]
    fn test_extract_predicates_from_text() {
        let preds = extract_predicates_from_text("follows(X, Y, _), is_followed_by(Y, X)");
        assert!(preds.contains("follows"));
        assert!(preds.contains("is_followed_by"));
        assert_eq!(preds.len(), 2);
    }

    #[test]
    fn test_extract_predicates_excludes_keywords() {
        let preds = extract_predicates_from_text("count(X), strlen(S), my_pred(A)");
        assert!(!preds.contains("count"));
        assert!(!preds.contains("strlen"));
        assert!(preds.contains("my_pred"));
    }

    #[test]
    fn test_get_required_predicates() {
        use chrono::Utc;

        let rules = vec![
            Rule {
                name: "mutual".to_string(),
                description: "Mutual follows".to_string(),
                head: "mutual(X)".to_string(),
                body: vec![
                    "follows(Self, X, _)".to_string(),
                    "is_followed_by(X, Self)".to_string(),
                ],
                constraints: vec![],
                enabled: true,
                priority: 0,
                created_at: Utc::now(),
            },
            Rule {
                name: "friend".to_string(),
                description: "Friends".to_string(),
                head: "friend(X)".to_string(),
                body: vec![
                    "mutual(X)".to_string(),
                    "liked(Self, P, _)".to_string(),
                    "posted(X, P, _)".to_string(),
                ],
                constraints: vec![],
                enabled: true,
                priority: 0,
                created_at: Utc::now(),
            },
        ];

        let graph = PredicateDependencyGraph::from_rules(&rules);

        // Query for mutual should need follows and is_followed_by
        let roots: HashSet<String> = ["mutual".to_string()].into_iter().collect();
        let required = graph.get_required_predicates(&roots);
        assert!(required.contains("mutual"));
        assert!(required.contains("follows"));
        assert!(required.contains("is_followed_by"));

        // Query for friend should need mutual, follows, is_followed_by, liked, posted
        let roots: HashSet<String> = ["friend".to_string()].into_iter().collect();
        let required = graph.get_required_predicates(&roots);
        assert!(required.contains("friend"));
        assert!(required.contains("mutual"));
        assert!(required.contains("follows"));
        assert!(required.contains("is_followed_by"));
        assert!(required.contains("liked"));
        assert!(required.contains("posted"));
    }

    #[test]
    fn test_extract_query_predicates() {
        let preds =
            PredicateDependencyGraph::extract_query_predicates("_validation_error(R, P, E)");
        assert!(preds.contains("_validation_error"));
        assert_eq!(preds.len(), 1);
    }
}
