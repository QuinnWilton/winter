//! Trigger evaluation engine.
//!
//! Periodically evaluates datalog conditions from trigger records and executes
//! actions when new result tuples appear. Deduplicates across evaluation cycles
//! so each unique result tuple fires at most once.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use winter_atproto::{AtprotoClient, Fact, RepoCache, Tid, TriggerAction};
use winter_datalog::DatalogCache;

/// Maximum actions per trigger per evaluation cycle.
const MAX_ACTIONS_PER_TRIGGER: usize = 50;

/// Engine that evaluates trigger conditions via datalog and executes actions
/// when new result tuples appear.
pub struct TriggerEngine {
    cache: Arc<RepoCache>,
    datalog: Arc<DatalogCache>,
    atproto: Arc<AtprotoClient>,
    mcp_base_url: String,
    http: reqwest::Client,
    /// Deduplication state: trigger rkey -> set of result tuples seen.
    last_fired: RwLock<HashMap<String, HashSet<Vec<String>>>>,
}

impl TriggerEngine {
    /// Create a new trigger engine.
    pub fn new(
        cache: Arc<RepoCache>,
        datalog: Arc<DatalogCache>,
        atproto: Arc<AtprotoClient>,
        mcp_base_url: String,
    ) -> Self {
        Self {
            cache,
            datalog,
            atproto,
            mcp_base_url,
            http: reqwest::Client::new(),
            last_fired: RwLock::new(HashMap::new()),
        }
    }

    /// Evaluate all enabled triggers.
    ///
    /// For each enabled trigger, runs the condition query via datalog,
    /// compares results against previously seen tuples, and executes
    /// the trigger action for each new tuple. Tuples that no longer
    /// appear in results are removed from the deduplication set.
    pub async fn evaluate_all(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let triggers = self.cache.list_triggers();

        if triggers.is_empty() {
            debug!("no triggers to evaluate");
            return Ok(());
        }

        debug!(count = triggers.len(), "evaluating triggers");

        for (rkey, cached_trigger) in &triggers {
            let trigger = &cached_trigger.value;

            if !trigger.enabled {
                continue;
            }

            // Build query from condition body
            let (query, rules) = Self::build_trigger_query(
                &trigger.condition,
                trigger.condition_rules.as_deref(),
            );

            // Run the condition query
            let results = match self
                .datalog
                .execute_query(
                    &query,
                    rules.as_deref(),
                )
                .await
            {
                Ok(results) => results,
                Err(e) => {
                    error!(
                        trigger_name = %trigger.name,
                        trigger_rkey = %rkey,
                        error = %e,
                        "failed to evaluate trigger condition"
                    );
                    continue;
                }
            };

            // Build the current result set for comparison
            let current_tuples: HashSet<Vec<String>> = results.into_iter().collect();

            // Get or create the last_fired entry for this trigger
            let mut last_fired = self.last_fired.write().await;
            let previous_tuples = last_fired
                .entry(rkey.clone())
                .or_insert_with(HashSet::new);

            // Find new tuples (in current but not in previous)
            let new_tuples: Vec<Vec<String>> = current_tuples
                .iter()
                .filter(|t| !previous_tuples.contains(*t))
                .cloned()
                .collect();

            // Remove stale tuples (in previous but not in current)
            previous_tuples.retain(|t| current_tuples.contains(t));

            // Drop the lock before executing actions
            drop(last_fired);

            if new_tuples.is_empty() {
                continue;
            }

            let capped = new_tuples.len() > MAX_ACTIONS_PER_TRIGGER;
            let to_process = if capped {
                warn!(
                    trigger_name = %trigger.name,
                    trigger_rkey = %rkey,
                    total = new_tuples.len(),
                    cap = MAX_ACTIONS_PER_TRIGGER,
                    "action cap reached, processing only first {} of {} new tuples",
                    MAX_ACTIONS_PER_TRIGGER,
                    new_tuples.len()
                );
                &new_tuples[..MAX_ACTIONS_PER_TRIGGER]
            } else {
                &new_tuples[..]
            };

            info!(
                trigger_name = %trigger.name,
                trigger_rkey = %rkey,
                new_tuples = to_process.len(),
                "executing trigger actions"
            );

            for tuple in to_process {
                match self
                    .execute_action(&trigger.name, &trigger.action, tuple)
                    .await
                {
                    Ok(()) => {
                        // Only add to last_fired on success
                        let mut last_fired = self.last_fired.write().await;
                        last_fired
                            .entry(rkey.clone())
                            .or_insert_with(HashSet::new)
                            .insert(tuple.clone());
                    }
                    Err(e) => {
                        error!(
                            trigger_name = %trigger.name,
                            trigger_rkey = %rkey,
                            error = %e,
                            "failed to execute trigger action"
                        );
                    }
                }
            }
        }

        // Clean up last_fired entries for triggers that no longer exist
        let active_rkeys: HashSet<&String> = triggers.iter().map(|(rkey, _)| rkey).collect();
        let mut last_fired = self.last_fired.write().await;
        last_fired.retain(|rkey, _| active_rkeys.contains(rkey));

        Ok(())
    }

    /// Execute a single trigger action with variable substitution from the tuple.
    async fn execute_action(
        &self,
        trigger_name: &str,
        action: &TriggerAction,
        tuple: &[String],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match action {
            TriggerAction::CreateFact {
                predicate,
                args,
                tags,
            } => {
                let substituted_args: Vec<String> = args
                    .iter()
                    .map(|arg| Self::substitute_variables(arg, tuple))
                    .collect();

                let fact = Fact {
                    predicate: predicate.clone(),
                    args: substituted_args,
                    confidence: None,
                    source: Some(format!("trigger:{}", trigger_name)),
                    supersedes: None,
                    tags: tags.clone(),
                    created_at: Utc::now(),
                    expires_at: None,
                };

                let rkey = Tid::now().to_string();
                self.atproto
                    .create_record("diy.razorgirl.winter.fact", Some(&rkey), &fact)
                    .await?;

                info!(
                    trigger_name = %trigger_name,
                    predicate = %predicate,
                    rkey = %rkey,
                    "created fact from trigger"
                );
            }

            TriggerAction::CreateInboxItem { message } => {
                let substituted_message = Self::substitute_variables(message, tuple);
                let full_message = format!("[trigger:{}] {}", trigger_name, substituted_message);

                let url = format!("{}/inbox", self.mcp_base_url);
                let body = serde_json::json!({
                    "message": full_message,
                    "priority": 50
                });

                let response = self.http.post(&url).json(&body).send().await?;

                if !response.status().is_success() {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    return Err(format!(
                        "inbox POST failed ({}): {}",
                        status, text
                    )
                    .into());
                }

                info!(
                    trigger_name = %trigger_name,
                    "created inbox item from trigger"
                );
            }

            TriggerAction::DeleteFact { rkey } => {
                let substituted_rkey = Self::substitute_variables(rkey, tuple);

                self.atproto
                    .delete_record("diy.razorgirl.winter.fact", &substituted_rkey)
                    .await?;

                info!(
                    trigger_name = %trigger_name,
                    rkey = %substituted_rkey,
                    "deleted fact from trigger"
                );
            }
        }

        Ok(())
    }

    /// Build a query and extra_rules for a trigger condition.
    ///
    /// Trigger conditions are rule bodies (conjunctions of literals) like
    /// `follows_me(X, _), !has_impression(X)`. These can't be passed directly
    /// as queries because `execute_query` expects a single predicate.
    ///
    /// This wraps the condition into a rule:
    ///   `_trigger_result(X) :- follows_me(X, _), !has_impression(X).`
    /// and queries `_trigger_result(X)`.
    fn build_trigger_query(condition: &str, condition_rules: Option<&str>) -> (String, Option<String>) {
        // Extract unique uppercase variables from the condition, preserving first-seen order
        let vars = Self::extract_variables(condition);

        let query = if vars.is_empty() {
            "_trigger_result()".to_string()
        } else {
            format!("_trigger_result({})", vars.join(", "))
        };

        // Build the wrapper rule
        let condition_trimmed = condition.trim().trim_end_matches('.');
        let wrapper_rule = if vars.is_empty() {
            format!("_trigger_result() :- {}.", condition_trimmed)
        } else {
            format!(
                "_trigger_result({}) :- {}.",
                vars.join(", "),
                condition_trimmed
            )
        };

        // Combine with any existing condition_rules
        let rules = match condition_rules {
            Some(existing) => format!("{}\n{}", existing, wrapper_rule),
            None => wrapper_rule,
        };

        (query, Some(rules))
    }

    /// Extract unique uppercase variable names from a datalog condition body,
    /// preserving first-seen order. Skips `_` (anonymous variable).
    fn extract_variables(condition: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut vars = Vec::new();

        // Split on typical datalog delimiters, then check each token
        for token in condition.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if token.is_empty() || token == "_" {
                continue;
            }
            if let Some(first) = token.chars().next() {
                if first.is_uppercase()
                    && token.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !seen.contains(token)
                {
                    seen.insert(token.to_string());
                    vars.push(token.to_string());
                }
            }
        }

        vars
    }

    /// Replace `$0`, `$1`, etc. in a template with values from the tuple.
    ///
    /// Out-of-range `$N` references are left as literals.
    fn substitute_variables(template: &str, tuple: &[String]) -> String {
        let mut result = template.to_string();

        // Replace from highest index to lowest to avoid $1 replacing part of $10
        for i in (0..tuple.len()).rev() {
            let placeholder = format!("${}", i);
            result = result.replace(&placeholder, &tuple[i]);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_variables_basic() {
        let tuple = vec!["alice".to_string(), "bob".to_string()];
        let result = TriggerEngine::substitute_variables("$0 follows $1", &tuple);
        assert_eq!(result, "alice follows bob");
    }

    #[test]
    fn test_substitute_variables_repeated() {
        let tuple = vec!["hello".to_string()];
        let result = TriggerEngine::substitute_variables("$0 and $0 again", &tuple);
        assert_eq!(result, "hello and hello again");
    }

    #[test]
    fn test_substitute_variables_out_of_range() {
        let tuple = vec!["only".to_string()];
        let result = TriggerEngine::substitute_variables("$0 and $1 and $2", &tuple);
        assert_eq!(result, "only and $1 and $2");
    }

    #[test]
    fn test_substitute_variables_empty_tuple() {
        let tuple: Vec<String> = vec![];
        let result = TriggerEngine::substitute_variables("no vars here", &tuple);
        assert_eq!(result, "no vars here");
    }

    #[test]
    fn test_substitute_variables_no_placeholders() {
        let tuple = vec!["unused".to_string()];
        let result = TriggerEngine::substitute_variables("plain text", &tuple);
        assert_eq!(result, "plain text");
    }

    #[test]
    fn test_substitute_variables_double_digit_index() {
        let tuple: Vec<String> = (0..11).map(|i| format!("val{}", i)).collect();
        let result = TriggerEngine::substitute_variables("$0 $10", &tuple);
        // $10 should be replaced with "val10", $0 with "val0"
        assert_eq!(result, "val0 val10");
    }

    #[test]
    fn test_extract_variables_basic() {
        let vars = TriggerEngine::extract_variables("follows_me(X, _), !has_impression(X)");
        assert_eq!(vars, vec!["X"]);
    }

    #[test]
    fn test_extract_variables_multiple() {
        let vars = TriggerEngine::extract_variables("follows(Self, X, _), is_followed_by(X, Self)");
        assert_eq!(vars, vec!["Self", "X"]);
    }

    #[test]
    fn test_extract_variables_with_strings() {
        let vars = TriggerEngine::extract_variables(r#"fact_tag(R, "social", _), _fact(R, P, _)"#);
        assert_eq!(vars, vec!["R", "P"]);
    }

    #[test]
    fn test_extract_variables_none() {
        let vars = TriggerEngine::extract_variables(r#"has_fact("hello", "world", _)"#);
        assert!(vars.is_empty());
    }

    #[test]
    fn test_build_trigger_query_conjunction() {
        let (query, rules) = TriggerEngine::build_trigger_query(
            "follows_me(X, _), !has_impression(X)",
            None,
        );
        assert_eq!(query, "_trigger_result(X)");
        let rules = rules.unwrap();
        assert!(rules.contains("_trigger_result(X) :- follows_me(X, _), !has_impression(X)."));
    }

    #[test]
    fn test_build_trigger_query_with_condition_rules() {
        let (query, rules) = TriggerEngine::build_trigger_query(
            "mutual(X)",
            Some("mutual(X) :- follows(Self, X, _), is_followed_by(X, Self)."),
        );
        assert_eq!(query, "_trigger_result(X)");
        let rules = rules.unwrap();
        assert!(rules.contains("mutual(X) :- follows(Self, X, _), is_followed_by(X, Self)."));
        assert!(rules.contains("_trigger_result(X) :- mutual(X)."));
    }

    #[test]
    fn test_build_trigger_query_nullary() {
        let (query, rules) = TriggerEngine::build_trigger_query(
            r#"has_fact("stale_check", _, _)"#,
            None,
        );
        assert_eq!(query, "_trigger_result()");
        let rules = rules.unwrap();
        assert!(rules.contains("_trigger_result() :- has_fact(\"stale_check\", _, _)."));
    }
}
