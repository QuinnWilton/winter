//! Integration tests for Winter.
//!
//! These tests require a running PDS (set PDS_URL, HANDLE, APP_PASSWORD env vars)
//! or use mocked responses for unit-like integration testing.

use serde_json::json;
use winter_atproto::{Directive, DirectiveKind, Fact, Identity, Thought, ThoughtKind};

// Helper to create a test fact
fn test_fact(predicate: &str, args: &[&str]) -> Fact {
    Fact {
        predicate: predicate.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        confidence: None,
        source: Some("test".to_string()),
        supersedes: None,
        tags: vec![],
        created_at: chrono::Utc::now(),
        expires_at: None,
    }
}

// Helper to create a test identity (slim version)
fn test_identity() -> Identity {
    Identity {
        operator_did: "did:plc:test".to_string(),
        created_at: chrono::Utc::now(),
        last_updated: chrono::Utc::now(),
    }
}

// Helper to create test directives
fn test_directives() -> Vec<Directive> {
    vec![
        Directive {
            kind: DirectiveKind::SelfConcept,
            content: "A test agent exploring the world.".to_string(),
            summary: None,
            active: true,
            confidence: None,
            source: None,
            supersedes: None,
            tags: vec![],
            priority: 0,
            created_at: chrono::Utc::now(),
            last_updated: None,
        },
        Directive {
            kind: DirectiveKind::Value,
            content: "curiosity".to_string(),
            summary: None,
            active: true,
            confidence: None,
            source: None,
            supersedes: None,
            tags: vec![],
            priority: 0,
            created_at: chrono::Utc::now(),
            last_updated: None,
        },
        Directive {
            kind: DirectiveKind::Value,
            content: "honesty".to_string(),
            summary: None,
            active: true,
            confidence: None,
            source: None,
            supersedes: None,
            tags: vec![],
            priority: 0,
            created_at: chrono::Utc::now(),
            last_updated: None,
        },
        Directive {
            kind: DirectiveKind::Interest,
            content: "distributed systems".to_string(),
            summary: None,
            active: true,
            confidence: None,
            source: None,
            supersedes: None,
            tags: vec![],
            priority: 0,
            created_at: chrono::Utc::now(),
            last_updated: None,
        },
    ]
}

mod fact_serialization {
    use super::*;

    #[test]
    fn fact_serializes_to_json() {
        let fact = test_fact("follows", &["did:plc:abc", "did:plc:xyz"]);
        let json = serde_json::to_value(&fact).unwrap();

        assert_eq!(json["predicate"], "follows");
        assert_eq!(json["args"], json!(["did:plc:abc", "did:plc:xyz"]));
        // confidence is None (not serialized), so it should be absent
        assert!(json.get("confidence").is_none() || json["confidence"].is_null());
    }

    #[test]
    fn fact_deserializes_from_json() {
        let json = json!({
            "predicate": "interested_in",
            "args": ["did:plc:abc", "rust"],
            "confidence": 0.8,
            "source": "observation",
            "tags": ["topic"],
            "createdAt": "2026-01-29T00:00:00Z"
        });

        let fact: Fact = serde_json::from_value(json).unwrap();
        assert_eq!(fact.predicate, "interested_in");
        assert_eq!(fact.args, vec!["did:plc:abc", "rust"]);
        assert!((fact.confidence.unwrap() - 0.8).abs() < 0.0001);
    }

    #[test]
    fn fact_handles_optional_fields() {
        let json = json!({
            "predicate": "test",
            "args": ["a"],
            "createdAt": "2026-01-29T00:00:00Z"
        });

        let fact: Fact = serde_json::from_value(json).unwrap();
        // Confidence is None when not provided (lexicon default is 1.0)
        assert_eq!(fact.confidence, None);
        assert_eq!(fact.source, None);
        assert!(fact.tags.is_empty());
    }

    #[test]
    fn fact_deserializes_integer_confidence() {
        // CBOR stores integers differently from floats, so we need to handle both
        let json = json!({
            "predicate": "test",
            "args": ["a"],
            "confidence": 1,
            "createdAt": "2026-01-29T00:00:00Z"
        });

        let fact: Fact = serde_json::from_value(json).unwrap();
        assert_eq!(fact.confidence, Some(1.0));
    }
}

mod identity_serialization {
    use super::*;

    #[test]
    fn identity_roundtrip() {
        let identity = test_identity();
        let json = serde_json::to_string(&identity).unwrap();
        let decoded: Identity = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.operator_did, identity.operator_did);
    }
}

mod directive_serialization {
    use super::*;

    #[test]
    fn directive_roundtrip() {
        let directives = test_directives();
        for directive in &directives {
            let json = serde_json::to_string(directive).unwrap();
            let decoded: Directive = serde_json::from_str(&json).unwrap();

            assert_eq!(decoded.kind, directive.kind);
            assert_eq!(decoded.content, directive.content);
            assert_eq!(decoded.active, directive.active);
        }
    }

    #[test]
    fn directive_kind_serializes_correctly() {
        let kinds = vec![
            (DirectiveKind::Value, "value"),
            (DirectiveKind::Interest, "interest"),
            (DirectiveKind::Belief, "belief"),
            (DirectiveKind::Guideline, "guideline"),
            (DirectiveKind::SelfConcept, "self_concept"),
            (DirectiveKind::Boundary, "boundary"),
            (DirectiveKind::Aspiration, "aspiration"),
        ];

        for (kind, expected) in kinds {
            let directive = Directive {
                kind: kind.clone(),
                content: "test".to_string(),
                summary: None,
                active: true,
                confidence: None,
                source: None,
                supersedes: None,
                tags: vec![],
                priority: 0,
                created_at: chrono::Utc::now(),
                last_updated: None,
            };

            let json = serde_json::to_value(&directive).unwrap();
            assert_eq!(json["kind"], expected);
        }
    }
}

mod thought_serialization {
    use super::*;

    #[test]
    fn thought_kind_serializes_correctly() {
        let kinds = vec![
            (ThoughtKind::Insight, "insight"),
            (ThoughtKind::Question, "question"),
            (ThoughtKind::Plan, "plan"),
            (ThoughtKind::Reflection, "reflection"),
            (ThoughtKind::Error, "error"),
            (ThoughtKind::Response, "response"),
        ];

        for (kind, expected) in kinds {
            let thought = Thought {
                kind,
                content: "test".to_string(),
                trigger: None,
                tags: vec![],
                duration_ms: None,
                created_at: chrono::Utc::now(),
            };

            let json = serde_json::to_value(&thought).unwrap();
            assert_eq!(json["kind"], expected);
        }
    }
}

mod agent_context {
    use super::*;
    use winter_agent::{AgentContext, ContextTrigger, PromptBuilder};

    #[test]
    fn context_builds_with_identity() {
        let identity = test_identity();
        let context = AgentContext::new(identity.clone());

        assert_eq!(context.identity.operator_did, identity.operator_did);
        assert!(context.trigger.is_none());
        assert!(context.recent_thoughts.is_empty());
        assert!(context.directives.is_empty());
    }

    #[test]
    fn context_includes_directives() {
        let identity = test_identity();
        let directives = test_directives();
        let context = AgentContext::new(identity).with_directives(directives.clone());

        assert_eq!(context.directives.len(), directives.len());
    }

    #[test]
    fn context_includes_trigger() {
        let identity = test_identity();
        let context =
            AgentContext::new(identity).with_trigger(ContextTrigger::PersistentSession);
        assert!(context.trigger.is_some());
    }

    #[test]
    fn prompt_builder_includes_directives() {
        let identity = test_identity();
        let directives = test_directives();
        let context = AgentContext::new(identity).with_directives(directives);
        let prompt = PromptBuilder::build(&context);

        assert!(prompt.contains("A test agent exploring the world."));
        assert!(prompt.contains("curiosity"));
        assert!(prompt.contains("distributed systems"));
    }

    #[test]
    fn prompt_builder_includes_persistent_session_trigger() {
        let identity = test_identity();
        let directives = test_directives();

        let context = AgentContext::new(identity)
            .with_directives(directives)
            .with_trigger(ContextTrigger::PersistentSession);
        let prompt = PromptBuilder::build(&context);

        assert!(prompt.contains("Persistent Session"));
        assert!(prompt.contains("check_inbox"));
        assert!(prompt.contains("acknowledge_inbox"));
    }
}

#[cfg(feature = "integration")]
mod pds_integration {
    //! Tests that require a running PDS.
    //!
    //! Run with: cargo test --features integration
    //! Requires: PDS_URL, HANDLE, APP_PASSWORD environment variables

    use super::*;
    use winter_atproto::AtprotoClient;

    fn get_test_client() -> Option<AtprotoClient> {
        let pds_url = std::env::var("PDS_URL").ok()?;
        let handle = std::env::var("HANDLE").ok()?;
        let app_password = std::env::var("APP_PASSWORD").ok()?;

        let client = AtprotoClient::new(&pds_url);
        // Note: actual login would need to be async
        Some(client)
    }

    #[tokio::test]
    #[ignore = "requires PDS"]
    async fn can_connect_to_pds() {
        if let Some(client) = get_test_client() {
            // Would test connection here
            let _ = client;
        }
    }
}

#[cfg(feature = "integration")]
mod datalog_integration {
    //! Tests for datalog integration with Soufflé.

    use winter_datalog::{DatalogError, SouffleExecutor};

    #[tokio::test]
    async fn souffle_executor_runs_simple_program() {
        let executor = SouffleExecutor::new();

        // Simple datalog program
        let program = r#"
.decl edge(x:symbol, y:symbol)
.decl path(x:symbol, y:symbol)
.output path

edge("a", "b").
edge("b", "c").

path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
"#;

        let temp_dir = tempfile::tempdir().unwrap();
        let result = executor.execute(program, temp_dir.path()).await;

        match result {
            Ok(output) => {
                assert!(output.contains("a\tb"));
                assert!(output.contains("a\tc"));
                assert!(output.contains("b\tc"));
            }
            Err(DatalogError::SouffleNotFound) => {
                // Skip if Soufflé not installed
                eprintln!("Skipping: Soufflé not installed");
            }
            Err(e) => panic!("Unexpected error: {}", e),
        }
    }
}
