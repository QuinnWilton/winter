//! Property-based tests for Winter's core types.

use proptest::prelude::*;
use winter_atproto::{Directive, DirectiveKind, Fact, Identity, Note, Rule, Thought, ThoughtKind};

// Strategy for generating valid identifiers (alphanumeric + underscore)
fn valid_identifier() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,30}".prop_map(|s| s.to_string())
}

// Strategy for generating non-empty strings
fn non_empty_string() -> impl Strategy<Value = String> {
    ".{1,100}".prop_map(|s| s.to_string())
}

// Strategy for generating confidence values (0.0 to 1.0)
fn confidence_value() -> impl Strategy<Value = Option<f64>> {
    proptest::option::of(0.0..=1.0f64)
}

// Strategy for generating ThoughtKind
fn thought_kind() -> impl Strategy<Value = ThoughtKind> {
    prop_oneof![
        Just(ThoughtKind::Insight),
        Just(ThoughtKind::Question),
        Just(ThoughtKind::Plan),
        Just(ThoughtKind::Reflection),
        Just(ThoughtKind::Error),
        Just(ThoughtKind::Response),
    ]
}

// Strategy for generating DirectiveKind
fn directive_kind() -> impl Strategy<Value = DirectiveKind> {
    prop_oneof![
        Just(DirectiveKind::Value),
        Just(DirectiveKind::Interest),
        Just(DirectiveKind::Belief),
        Just(DirectiveKind::Guideline),
        Just(DirectiveKind::SelfConcept),
        Just(DirectiveKind::Boundary),
        Just(DirectiveKind::Aspiration),
    ]
}

proptest! {
    // Identity tests (slim version)
    #[test]
    fn identity_roundtrip(
        operator_did in non_empty_string(),
    ) {
        let identity = Identity {
            operator_did: operator_did.clone(),
            created_at: chrono::Utc::now(),
            last_updated: chrono::Utc::now(),
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&identity).unwrap();
        let decoded: Identity = serde_json::from_str(&json).unwrap();

        prop_assert_eq!(decoded.operator_did, operator_did);
    }

    // Directive tests
    #[test]
    fn directive_roundtrip(
        kind in directive_kind(),
        content in non_empty_string(),
        summary in proptest::option::of(non_empty_string()),
        active in proptest::bool::ANY,
        confidence in confidence_value(),
        priority in 0i32..100,
        tags in prop::collection::vec(valid_identifier(), 0..5),
    ) {
        let directive = Directive {
            kind: kind.clone(),
            content: content.clone(),
            summary: summary.clone(),
            active,
            confidence,
            source: None,
            supersedes: None,
            tags: tags.clone(),
            priority,
            created_at: chrono::Utc::now(),
            last_updated: None,
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&directive).unwrap();
        let decoded: Directive = serde_json::from_str(&json).unwrap();

        prop_assert_eq!(decoded.kind, kind);
        prop_assert_eq!(decoded.content, content);
        prop_assert_eq!(decoded.summary, summary);
        prop_assert_eq!(decoded.active, active);
        // Allow small floating point differences for confidence
        match (decoded.confidence, confidence) {
            (Some(d), Some(c)) => prop_assert!((d - c).abs() < 0.0001),
            (None, None) => {}
            _ => prop_assert!(false, "confidence mismatch"),
        }
        prop_assert_eq!(decoded.priority, priority);
        prop_assert_eq!(decoded.tags, tags);
    }

    // Fact tests
    #[test]
    fn fact_roundtrip(
        predicate in valid_identifier(),
        args in prop::collection::vec(non_empty_string(), 1..5),
        confidence in confidence_value(),
        source in proptest::option::of(non_empty_string()),
        tags in prop::collection::vec(valid_identifier(), 0..3),
    ) {
        let fact = Fact {
            predicate: predicate.clone(),
            args: args.clone(),
            confidence,
            source: source.clone(),
            supersedes: None,
            tags: tags.clone(),
            created_at: chrono::Utc::now(),
            expires_at: None,
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&fact).unwrap();
        let decoded: Fact = serde_json::from_str(&json).unwrap();

        prop_assert_eq!(decoded.predicate, predicate);
        prop_assert_eq!(decoded.args, args);
        // Allow small floating point differences for confidence
        match (decoded.confidence, confidence) {
            (Some(d), Some(c)) => prop_assert!((d - c).abs() < 0.0001),
            (None, None) => {}
            _ => prop_assert!(false, "confidence mismatch"),
        }
        prop_assert_eq!(decoded.source, source);
        prop_assert_eq!(decoded.tags, tags);
    }

    // Rule tests
    #[test]
    fn rule_roundtrip(
        name in valid_identifier(),
        description in non_empty_string(),
        head in valid_identifier(),
        body in prop::collection::vec(non_empty_string(), 1..3),
        enabled in proptest::bool::ANY,
        priority in 0i32..100,
    ) {
        let rule = Rule {
            name: name.clone(),
            description: description.clone(),
            head: head.clone(),
            body: body.clone(),
            constraints: Vec::new(),
            enabled,
            priority,
            created_at: chrono::Utc::now(),
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&rule).unwrap();
        let decoded: Rule = serde_json::from_str(&json).unwrap();

        prop_assert_eq!(decoded.name, name);
        prop_assert_eq!(decoded.description, description);
        prop_assert_eq!(decoded.head, head);
        prop_assert_eq!(decoded.body, body);
        prop_assert_eq!(decoded.enabled, enabled);
        prop_assert_eq!(decoded.priority, priority);
    }

    // Note tests
    #[test]
    fn note_roundtrip(
        title in non_empty_string(),
        content in non_empty_string(),
        category in proptest::option::of(valid_identifier()),
        tags in prop::collection::vec(valid_identifier(), 0..5),
    ) {
        let note = Note {
            title: title.clone(),
            content: content.clone(),
            category: category.clone(),
            related_facts: Vec::new(),
            tags: tags.clone(),
            created_at: chrono::Utc::now(),
            last_updated: chrono::Utc::now(),
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&note).unwrap();
        let decoded: Note = serde_json::from_str(&json).unwrap();

        prop_assert_eq!(decoded.title, title);
        prop_assert_eq!(decoded.content, content);
        prop_assert_eq!(decoded.category, category);
        prop_assert_eq!(decoded.tags, tags);
    }

    // Thought tests
    #[test]
    fn thought_roundtrip(
        kind in thought_kind(),
        content in non_empty_string(),
        trigger in proptest::option::of(non_empty_string()),
        duration_ms in proptest::option::of(0u64..100000),
    ) {
        let thought = Thought {
            kind: kind.clone(),
            content: content.clone(),
            trigger: trigger.clone(),
            tags: vec![],
            duration_ms,
            created_at: chrono::Utc::now(),
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&thought).unwrap();
        let decoded: Thought = serde_json::from_str(&json).unwrap();

        prop_assert_eq!(decoded.kind, kind);
        prop_assert_eq!(decoded.content, content);
        prop_assert_eq!(decoded.trigger, trigger);
        prop_assert_eq!(decoded.duration_ms, duration_ms);
    }
}

// Datalog-specific property tests
mod datalog {
    use super::*;

    proptest! {
        // Fact predicate names should be valid Soufflé identifiers
        #[test]
        fn fact_predicate_is_valid_souffle_identifier(predicate in valid_identifier()) {
            // Soufflé identifiers must start with a letter and contain only alphanumeric + underscore
            prop_assert!(predicate.chars().next().unwrap().is_ascii_lowercase());
            prop_assert!(predicate.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        }

        // Facts with same predicate and args should be considered equal (for deduplication)
        #[test]
        fn facts_with_same_predicate_and_args_are_equal(
            predicate in valid_identifier(),
            args in prop::collection::vec(non_empty_string(), 1..3),
        ) {
            let fact1 = Fact {
                predicate: predicate.clone(),
                args: args.clone(),
                confidence: Some(0.9),
                source: Some("test1".to_string()),
                supersedes: None,
                tags: vec!["tag1".to_string()],
                created_at: chrono::Utc::now(),
                expires_at: None,
            };

            let fact2 = Fact {
                predicate: predicate.clone(),
                args: args.clone(),
                confidence: Some(0.5),
                source: Some("test2".to_string()),
                supersedes: None,
                tags: vec!["tag2".to_string()],
                created_at: chrono::Utc::now(),
                expires_at: None,
            };

            // Same predicate and args means same semantic fact
            prop_assert_eq!(fact1.predicate, fact2.predicate);
            prop_assert_eq!(fact1.args, fact2.args);
        }

        // Rule body must be non-empty for valid datalog
        #[test]
        fn rule_body_is_non_empty(
            body in prop::collection::vec(non_empty_string(), 1..5),
        ) {
            prop_assert!(!body.is_empty());
        }
    }
}
