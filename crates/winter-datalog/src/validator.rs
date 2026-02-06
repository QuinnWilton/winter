//! Fact validation against predicate declarations.
//!
//! When an explicit fact declaration exists for a predicate, facts are validated
//! against that schema. Non-conforming facts are skipped from TSV output to prevent
//! Soufflé errors, and warnings are logged for investigation.

use std::collections::HashMap;
use std::fmt;

use winter_atproto::{Fact, FactDeclaration};

/// Error types for fact validation.
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// The fact has a different number of arguments than declared.
    ArityMismatch { expected: usize, actual: usize },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::ArityMismatch { expected, actual } => {
                write!(
                    f,
                    "arity mismatch: expected {} args, got {}",
                    expected, actual
                )
            }
        }
    }
}

/// Validate a fact against its declaration, if one exists.
///
/// Returns `None` if:
/// - No declaration exists for this predicate (permissive mode)
/// - The fact conforms to the declaration
///
/// Returns `Some(ValidationError)` if the fact does not conform to its declaration.
pub fn validate_fact_against_declaration(
    fact: &Fact,
    declarations_by_predicate: &HashMap<String, FactDeclaration>,
) -> Option<ValidationError> {
    let declaration = declarations_by_predicate.get(&fact.predicate)?;

    if fact.args.len() != declaration.args.len() {
        return Some(ValidationError::ArityMismatch {
            expected: declaration.args.len(),
            actual: fact.args.len(),
        });
    }

    None // Valid (or no declaration = permissive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use winter_atproto::FactDeclArg;

    fn make_declaration(predicate: &str, arg_count: usize) -> FactDeclaration {
        let args: Vec<FactDeclArg> = (0..arg_count)
            .map(|i| FactDeclArg {
                name: format!("arg{}", i),
                r#type: "symbol".to_string(),
                description: Some(format!("Argument {}", i)),
            })
            .collect();

        FactDeclaration {
            predicate: predicate.to_string(),
            args,
            description: "Test declaration".to_string(),
            tags: vec![],
            created_at: Utc::now(),
            last_updated: None,
        }
    }

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
    fn test_no_declaration_is_permissive() {
        let declarations = HashMap::new();
        let fact = make_fact("undeclared", vec!["a", "b", "c"]);

        let result = validate_fact_against_declaration(&fact, &declarations);
        assert!(result.is_none(), "should be permissive when no declaration");
    }

    #[test]
    fn test_matching_arity_passes() {
        let mut declarations = HashMap::new();
        declarations.insert("test_pred".to_string(), make_declaration("test_pred", 2));

        let fact = make_fact("test_pred", vec!["a", "b"]);
        let result = validate_fact_against_declaration(&fact, &declarations);
        assert!(result.is_none(), "matching arity should pass validation");
    }

    #[test]
    fn test_arity_mismatch_fails() {
        let mut declarations = HashMap::new();
        declarations.insert("test_pred".to_string(), make_declaration("test_pred", 2));

        let fact = make_fact("test_pred", vec!["a", "b", "c"]); // 3 args, expected 2
        let result = validate_fact_against_declaration(&fact, &declarations);

        assert!(result.is_some(), "arity mismatch should fail validation");
        match result.unwrap() {
            ValidationError::ArityMismatch { expected, actual } => {
                assert_eq!(expected, 2);
                assert_eq!(actual, 3);
            }
        }
    }

    #[test]
    fn test_too_few_args_fails() {
        let mut declarations = HashMap::new();
        declarations.insert("test_pred".to_string(), make_declaration("test_pred", 3));

        let fact = make_fact("test_pred", vec!["a"]); // 1 arg, expected 3
        let result = validate_fact_against_declaration(&fact, &declarations);

        assert!(result.is_some());
        match result.unwrap() {
            ValidationError::ArityMismatch { expected, actual } => {
                assert_eq!(expected, 3);
                assert_eq!(actual, 1);
            }
        }
    }

    #[test]
    fn test_error_display() {
        let error = ValidationError::ArityMismatch {
            expected: 2,
            actual: 5,
        };
        assert_eq!(
            format!("{}", error),
            "arity mismatch: expected 2 args, got 5"
        );
    }
}
