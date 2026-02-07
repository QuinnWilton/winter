//! Compile rules to Soufflé `.dl` format.

use std::collections::HashMap;

use winter_atproto::Rule;

use crate::DatalogError;

/// Compiles Winter rules to Soufflé datalog format.
pub struct RuleCompiler;

impl RuleCompiler {
    /// Compile a single rule to Soufflé format.
    pub fn compile_rule(rule: &Rule) -> Result<String, DatalogError> {
        if !rule.enabled {
            return Ok(String::new());
        }

        // Validate the rule has head and body
        if rule.head.is_empty() {
            return Err(DatalogError::InvalidRule(
                "rule head cannot be empty".to_string(),
            ));
        }
        if rule.body.is_empty() {
            return Err(DatalogError::InvalidRule(
                "rule body cannot be empty".to_string(),
            ));
        }

        // Build the rule: head :- body, constraints.
        let mut rule_str = format!("{} :- {}", rule.head, rule.body.join(", "));

        if !rule.constraints.is_empty() {
            rule_str.push_str(", ");
            rule_str.push_str(&rule.constraints.join(", "));
        }

        rule_str.push('.');

        Ok(rule_str)
    }

    /// Compile all rules to Soufflé format.
    pub fn compile_rules(rules: &[Rule]) -> Result<String, DatalogError> {
        let mut output = String::new();

        // Sort by priority (lower = earlier)
        let mut sorted: Vec<_> = rules.iter().collect();
        sorted.sort_by_key(|r| r.priority);

        for rule in sorted {
            let compiled = Self::compile_rule(rule)?;
            if !compiled.is_empty() {
                output.push_str(&format!("// {}: {}\n", rule.name, rule.description));
                output.push_str(&compiled);
                output.push_str("\n\n");
            }
        }

        Ok(output)
    }

    /// Generate declarations for derived predicates (rule heads).
    ///
    /// This extracts predicates from rule heads and generates `.decl` statements
    /// for each. These must be emitted before rules are used, otherwise Soufflé
    /// will fail with undeclared relation errors.
    ///
    /// If `already_declared` is provided, predicates in that set are skipped to
    /// avoid redefinition errors (e.g., when a rule head uses the same predicate
    /// as an input fact).
    ///
    /// Returns a tuple of (declarations string, set of declared predicate names).
    pub fn generate_derived_declarations(
        rules: &[Rule],
        already_declared: Option<&std::collections::HashSet<String>>,
    ) -> (String, std::collections::HashSet<String>) {
        let mut predicates: HashMap<String, usize> = HashMap::new();

        for rule in rules {
            if !rule.enabled {
                continue;
            }

            if let Some((name, arity)) = Self::parse_head(&rule.head) {
                // Skip if already declared as an input predicate
                if let Some(declared) = already_declared
                    && declared.contains(&name)
                {
                    continue;
                }
                // Only insert if we haven't seen this predicate yet
                predicates.entry(name).or_insert(arity);
            }
        }

        let mut declarations = String::new();
        let mut declared_set = std::collections::HashSet::new();

        if !predicates.is_empty() {
            declarations.push_str("// Derived predicate declarations\n");
            for (predicate, arity) in predicates {
                let params: Vec<String> = (0..arity).map(|i| format!("arg{}: symbol", i)).collect();
                declarations.push_str(&format!(".decl {}({})\n", predicate, params.join(", ")));
                declared_set.insert(predicate);
            }
            declarations.push('\n');
        }

        (declarations, declared_set)
    }

    /// Parse head predicates from raw extra_rules text.
    ///
    /// Extracts predicate names and arities from rule heads like:
    /// "test(X) :- foo(X)." -> [("test", 1)]
    /// "bar(A, B) :- baz(A), qux(B)." -> [("bar", 2)]
    pub fn parse_extra_rules_heads(extra_rules: &str) -> Vec<(String, usize)> {
        let mut heads = Vec::new();

        for line in extra_rules.lines() {
            let line = line.trim();
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with("//") {
                continue;
            }

            // Find rule separator
            if let Some(sep_idx) = line.find(":-") {
                let head_part = line[..sep_idx].trim();
                if let Some((name, arity)) = Self::parse_head(head_part) {
                    heads.push((name, arity));
                }
            }
        }

        heads
    }

    /// Parse a rule head to extract predicate name and arity.
    /// e.g., "mutual_follow(X, Y)" -> Some(("mutual_follow", 2))
    pub fn parse_head(head: &str) -> Option<(String, usize)> {
        let paren_idx = head.find('(')?;
        let name = head[..paren_idx].trim().to_string();
        if name.is_empty() {
            return None;
        }

        let args_part = &head[paren_idx..];

        // Count arguments by counting commas + 1 (unless empty parens)
        let arity = if args_part.contains("()") || args_part.trim() == "()" {
            0
        } else if args_part.contains(',') {
            args_part.matches(',').count() + 1
        } else {
            1
        };

        Some((name, arity))
    }

    /// Compile a single rule to Soufflé format with optional comment.
    ///
    /// Unlike `compile_rule`, this includes the rule name as a comment.
    pub fn compile_single_rule(rule: &Rule) -> Result<String, DatalogError> {
        let compiled = Self::compile_rule(rule)?;
        if compiled.is_empty() {
            return Ok(String::new());
        }

        Ok(format!(
            "// {}: {}\n{}\n",
            rule.name, rule.description, compiled
        ))
    }

    /// Generate output declaration for a query predicate.
    ///
    /// If `already_declared` is provided and contains the predicate, only emits
    /// the `.output` directive (skips the `.decl` to avoid redefinition errors).
    pub fn generate_output_declaration(
        predicate: &str,
        arity: usize,
        already_declared: Option<&std::collections::HashSet<String>>,
    ) -> String {
        let needs_decl = already_declared
            .map(|set| !set.contains(predicate))
            .unwrap_or(true);

        if needs_decl {
            let params: Vec<String> = (0..arity).map(|i| format!("arg{}: symbol", i)).collect();
            format!(
                ".decl {}({})\n.output {}\n",
                predicate,
                params.join(", "),
                predicate
            )
        } else {
            format!(".output {}\n", predicate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_rule(name: &str, head: &str, body: Vec<&str>) -> Rule {
        Rule {
            name: name.to_string(),
            description: format!("{} rule", name),
            head: head.to_string(),
            body: body.into_iter().map(String::from).collect(),
            constraints: vec![],
            enabled: true,
            priority: 0,
            args: Vec::new(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_compile_rule() {
        let rule = make_rule(
            "mutual_follow",
            "mutual_follow(X, Y)",
            vec!["follows(X, Y)", "follows(Y, X)"],
        );

        let compiled = RuleCompiler::compile_rule(&rule).unwrap();
        assert_eq!(
            compiled,
            "mutual_follow(X, Y) :- follows(X, Y), follows(Y, X)."
        );
    }

    #[test]
    fn test_compile_rule_with_constraints() {
        let mut rule = make_rule(
            "recent_follow",
            "recent_follow(X, Y)",
            vec!["follows(X, Y)"],
        );
        rule.constraints = vec!["X != Y".to_string()];

        let compiled = RuleCompiler::compile_rule(&rule).unwrap();
        assert_eq!(compiled, "recent_follow(X, Y) :- follows(X, Y), X != Y.");
    }

    #[test]
    fn test_compile_rule_with_negation() {
        // Negation is supported via ! prefix in body literals
        let rule = make_rule(
            "introduce",
            "introduce(A, B, Topic)",
            vec![
                "follows(Me, A)",
                "follows(Me, B)",
                "interested_in(A, Topic)",
                "interested_in(B, Topic)",
                "!follows(A, B)",
            ],
        );

        let compiled = RuleCompiler::compile_rule(&rule).unwrap();
        assert!(compiled.contains("!follows(A, B)"));
        assert_eq!(
            compiled,
            "introduce(A, B, Topic) :- follows(Me, A), follows(Me, B), interested_in(A, Topic), interested_in(B, Topic), !follows(A, B)."
        );
    }

    #[test]
    fn test_disabled_rule() {
        let mut rule = make_rule("test", "test(X)", vec!["foo(X)"]);
        rule.enabled = false;

        let compiled = RuleCompiler::compile_rule(&rule).unwrap();
        assert!(compiled.is_empty());
    }

    #[test]
    fn test_parse_head_binary() {
        let result = RuleCompiler::parse_head("mutual_follow(X, Y)");
        assert_eq!(result, Some(("mutual_follow".to_string(), 2)));
    }

    #[test]
    fn test_parse_head_unary() {
        let result = RuleCompiler::parse_head("is_active(X)");
        assert_eq!(result, Some(("is_active".to_string(), 1)));
    }

    #[test]
    fn test_parse_head_ternary() {
        let result = RuleCompiler::parse_head("shared_interest(X, Y, T)");
        assert_eq!(result, Some(("shared_interest".to_string(), 3)));
    }

    #[test]
    fn test_parse_head_no_parens() {
        let result = RuleCompiler::parse_head("invalid");
        assert_eq!(result, None);
    }

    #[test]
    fn test_generate_derived_declarations() {
        let rules = vec![
            make_rule(
                "mutual_follow",
                "mutual_follow(X, Y)",
                vec!["follows(X, Y)", "follows(Y, X)"],
            ),
            make_rule(
                "shared_interest",
                "shared_interest(X, Y, T)",
                vec!["interested_in(X, T)", "interested_in(Y, T)"],
            ),
        ];

        let (decls, declared) = RuleCompiler::generate_derived_declarations(&rules, None);

        // Should contain declarations for both derived predicates
        assert!(decls.contains(".decl mutual_follow("));
        assert!(decls.contains(".decl shared_interest("));
        // Should not contain .input (these are derived, not input facts)
        assert!(!decls.contains(".input"));
        // Should track declared predicates
        assert!(declared.contains("mutual_follow"));
        assert!(declared.contains("shared_interest"));
    }

    #[test]
    fn test_generate_derived_declarations_skips_disabled() {
        let mut rule = make_rule("test", "test_pred(X)", vec!["foo(X)"]);
        rule.enabled = false;

        let (decls, declared) = RuleCompiler::generate_derived_declarations(&[rule], None);
        assert!(!decls.contains("test_pred"));
        assert!(declared.is_empty());
    }

    #[test]
    fn test_generate_derived_declarations_empty() {
        let (decls, declared) = RuleCompiler::generate_derived_declarations(&[], None);
        assert!(decls.is_empty());
        assert!(declared.is_empty());
    }

    #[test]
    fn test_generate_derived_declarations_skips_already_declared() {
        let rules = vec![
            make_rule(
                "agent_peer",
                "agent_peer(X, Y)",
                vec!["follows(X, Y)", "follows(Y, X)"],
            ),
            make_rule("new_derived", "new_derived(X)", vec!["some_fact(X)"]),
        ];

        // Simulate agent_peer already declared as an input predicate
        let mut already_declared = std::collections::HashSet::new();
        already_declared.insert("agent_peer".to_string());

        let (decls, declared) =
            RuleCompiler::generate_derived_declarations(&rules, Some(&already_declared));

        // Should NOT contain declaration for agent_peer (already declared)
        assert!(!decls.contains(".decl agent_peer("));
        assert!(!declared.contains("agent_peer"));

        // Should contain declaration for new_derived
        assert!(decls.contains(".decl new_derived("));
        assert!(declared.contains("new_derived"));
    }

    #[test]
    fn test_generate_output_declaration_with_already_declared() {
        let mut declared = std::collections::HashSet::new();
        declared.insert("mutual_follow".to_string());

        // When predicate is already declared, should only emit .output
        let output = RuleCompiler::generate_output_declaration("mutual_follow", 2, Some(&declared));
        assert!(!output.contains(".decl"));
        assert!(output.contains(".output mutual_follow"));

        // When predicate is not declared, should emit both .decl and .output
        let output = RuleCompiler::generate_output_declaration("new_pred", 2, Some(&declared));
        assert!(output.contains(".decl new_pred("));
        assert!(output.contains(".output new_pred"));
    }

    #[test]
    fn test_generate_output_declaration_without_declared_set() {
        // When no declared set provided, should always emit .decl
        let output = RuleCompiler::generate_output_declaration("test_pred", 1, None);
        assert!(output.contains(".decl test_pred("));
        assert!(output.contains(".output test_pred"));
    }

    #[test]
    fn test_parse_extra_rules_heads_single() {
        let heads = RuleCompiler::parse_extra_rules_heads("test(X) :- foo(X).");
        assert_eq!(heads, vec![("test".to_string(), 1)]);
    }

    #[test]
    fn test_parse_extra_rules_heads_multiple() {
        let heads = RuleCompiler::parse_extra_rules_heads(
            "test(X) :- foo(X).\nbar(A, B) :- baz(A), qux(B).",
        );
        assert_eq!(heads.len(), 2);
        assert!(heads.contains(&("test".to_string(), 1)));
        assert!(heads.contains(&("bar".to_string(), 2)));
    }

    #[test]
    fn test_parse_extra_rules_heads_with_comments() {
        let heads = RuleCompiler::parse_extra_rules_heads(
            "// This is a comment\ntest(X) :- foo(X).\n// Another comment",
        );
        assert_eq!(heads, vec![("test".to_string(), 1)]);
    }

    #[test]
    fn test_parse_extra_rules_heads_empty() {
        let heads = RuleCompiler::parse_extra_rules_heads("");
        assert!(heads.is_empty());
    }

    #[test]
    fn test_parse_extra_rules_heads_with_constant() {
        // The key case: rule with constant argument in body
        let heads = RuleCompiler::parse_extra_rules_heads(
            r#"filtered(X) :- category(X, "protocol_design")."#,
        );
        assert_eq!(heads, vec![("filtered".to_string(), 1)]);
    }
}
