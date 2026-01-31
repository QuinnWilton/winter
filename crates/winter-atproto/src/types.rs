//! Core types for ATProto records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter for TID clock identifier to ensure uniqueness within a process.
static TID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A TID (timestamp-based ID) used as record keys.
///
/// Per ATProto spec: https://atproto.com/specs/record-key
/// TIDs are 13 characters of base32-sortable encoding containing:
/// - 53 bits of microsecond timestamp
/// - 10 bits of clock identifier (for collision prevention)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tid(pub String);

impl Tid {
    /// Generate a new TID based on current time with a unique clock identifier.
    ///
    /// The TID format is 63 bits total:
    /// - Upper 53 bits: microseconds since Unix epoch
    /// - Lower 10 bits: clock identifier (atomic counter for uniqueness)
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};

        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;

        // Use an atomic counter for the clock ID bits to ensure uniqueness within process.
        // Counter wraps at 1024 (10 bits), which handles up to 1024 TIDs per microsecond.
        let clock_id = TID_COUNTER.fetch_add(1, Ordering::Relaxed) & 0x3FF;

        // Combine: timestamp in upper 53 bits, clock ID in lower 10 bits
        // Total: 63 bits (fits in 13 base32 characters = 65 bits)
        let combined = (micros << 10) | clock_id;

        Self::from_u64(combined)
    }

    /// Create a TID from a raw 63-bit value.
    fn from_u64(val: u64) -> Self {
        // Base32-sortable encoding (uses digits 2-7 and a-z)
        const CHARSET: &[u8] = b"234567abcdefghijklmnopqrstuvwxyz";
        let mut tid = String::with_capacity(13);

        let mut v = val;
        for _ in 0..13 {
            tid.push(CHARSET[(v & 0x1f) as usize] as char);
            v >>= 5;
        }

        Self(tid.chars().rev().collect())
    }
}

impl std::fmt::Display for Tid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Tid {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Tid {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// A strong reference to a record (URI + CID).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrongRef {
    pub uri: String,
    pub cid: String,
}

/// Session information from authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub refresh_jwt: String,
}

/// Response from creating a record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRecordResponse {
    pub uri: String,
    pub cid: String,
}

/// Response from getting a record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRecordResponse<T> {
    pub uri: String,
    pub cid: Option<String>,
    pub value: T,
}

/// Response from listing records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRecordsResponse<T> {
    pub records: Vec<ListRecordItem<T>>,
    pub cursor: Option<String>,
}

/// A single record in a list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRecordItem<T> {
    pub uri: String,
    pub cid: String,
    pub value: T,
}

/// Winter identity record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// DID of the operator who controls this Winter instance.
    pub operator_did: String,
    /// What Winter cares about.
    pub values: Vec<String>,
    /// What Winter is curious about.
    pub interests: Vec<String>,
    /// Free-form prose Winter writes about itself.
    pub self_description: String,
    /// When this identity was created.
    pub created_at: DateTime<Utc>,
    /// When this identity was last updated.
    pub last_updated: DateTime<Utc>,
}

/// Atomic fact record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    /// Predicate name (e.g., "follows", "interested_in").
    pub predicate: String,
    /// Arguments to the predicate.
    pub args: Vec<String>,
    /// Confidence level (0.0 to 1.0). When None, uses lexicon default of 1.0.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_confidence"
    )]
    pub confidence: Option<f64>,
    /// CID reference to source (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// CID of fact this supersedes (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// Tags for categorization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// When this fact was created.
    pub created_at: DateTime<Utc>,
}

/// Deserialize confidence from either integer or float.
/// CBOR distinguishes between integers and floats, so we need to handle both.
fn deserialize_confidence<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct ConfidenceVisitor;

    impl<'de> Visitor<'de> for ConfidenceVisitor {
        type Value = Option<f64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a number (integer or float) or null")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value as f64))
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value as f64))
        }

        fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(ConfidenceVisitor)
        }
    }

    deserializer.deserialize_option(ConfidenceVisitor)
}

/// Deserialize an optional u64 from any integer type.
/// CBOR encodes integers with smallest representation, so we need flexibility.
fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct OptionalU64Visitor;

    impl<'de> Visitor<'de> for OptionalU64Visitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an integer or null")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value >= 0 {
                Ok(Some(value as u64))
            } else {
                Err(de::Error::custom("negative value for u64"))
            }
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(OptionalU64Visitor)
        }
    }

    deserializer.deserialize_option(OptionalU64Visitor)
}

/// Deserialize a u64 from any integer type, defaulting to 0.
fn deserialize_u64_or_default<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct U64Visitor;

    impl<'de> Visitor<'de> for U64Visitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an integer")
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value >= 0 {
                Ok(value as u64)
            } else {
                Ok(0) // Default for negative
            }
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(value)
        }
    }

    deserializer.deserialize_any(U64Visitor)
}

/// Deserialize a u32 from any integer type, defaulting to 0.
fn deserialize_u32_or_default<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct U32Visitor;

    impl<'de> Visitor<'de> for U32Visitor {
        type Value = u32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an integer")
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value >= 0 && value <= u32::MAX as i64 {
                Ok(value as u32)
            } else {
                Ok(0)
            }
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value <= u32::MAX as u64 {
                Ok(value as u32)
            } else {
                Ok(u32::MAX)
            }
        }
    }

    deserializer.deserialize_any(U32Visitor)
}

/// Deserialize an i32 from any integer type, defaulting to 0.
fn deserialize_i32_or_default<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct I32Visitor;

    impl<'de> Visitor<'de> for I32Visitor {
        type Value = i32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an integer")
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value >= i32::MIN as i64 && value <= i32::MAX as i64 {
                Ok(value as i32)
            } else {
                Ok(0)
            }
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value <= i32::MAX as u64 {
                Ok(value as i32)
            } else {
                Ok(i32::MAX)
            }
        }
    }

    deserializer.deserialize_any(I32Visitor)
}

/// Datalog rule record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    /// Rule name for reference.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Rule head (derived predicate).
    pub head: String,
    /// Rule body (conditions).
    pub body: Vec<String>,
    /// Additional constraints.
    #[serde(default)]
    pub constraints: Vec<String>,
    /// Whether this rule is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority for rule ordering (flexible integer deserialization for CBOR).
    #[serde(default, deserialize_with = "deserialize_i32_or_default")]
    pub priority: i32,
    /// When this rule was created.
    pub created_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

/// Free-form note record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// Note title.
    pub title: String,
    /// Markdown content (max 50KB).
    pub content: String,
    /// Category for organization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// CID references to related facts.
    #[serde(default)]
    pub related_facts: Vec<String>,
    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// When this note was created.
    pub created_at: DateTime<Utc>,
    /// When this note was last updated.
    pub last_updated: DateTime<Utc>,
}

/// Scheduled job record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// Job name.
    pub name: String,
    /// Instructions for the agent.
    pub instructions: String,
    /// Schedule configuration.
    pub schedule: JobSchedule,
    /// Current status.
    #[serde(default)]
    pub status: JobStatus,
    /// When this job last ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<DateTime<Utc>>,
    /// When this job should next run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run: Option<DateTime<Utc>>,
    /// Number of consecutive failures (flexible integer deserialization for CBOR).
    #[serde(default, deserialize_with = "deserialize_u32_or_default")]
    pub failure_count: u32,
    /// When this job was created.
    pub created_at: DateTime<Utc>,
}

/// Job schedule configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobSchedule {
    /// Run once at a specific time.
    Once { at: DateTime<Utc> },
    /// Run at regular intervals (flexible integer deserialization for CBOR).
    Interval {
        #[serde(deserialize_with = "deserialize_u64_or_default")]
        seconds: u64,
    },
}

/// Job status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed {
        error: String,
    },
}

/// Thought record (stream of consciousness).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thought {
    /// Kind of thought.
    pub kind: ThoughtKind,
    /// Content of the thought.
    pub content: String,
    /// What triggered this thought.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// How long this thought took to generate (ms, flexible integer deserialization for CBOR).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub duration_ms: Option<u64>,
    /// When this thought was recorded.
    pub created_at: DateTime<Utc>,
}

/// Kind of thought in Winter's stream of consciousness.
///
/// The web UI displays each kind with a distinct color for transparency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThoughtKind {
    /// Something noticed, perceived, or understood.
    /// Use for external observations (daemon receiving notifications) or
    /// internal realizations (Claude noticing patterns, making deductions).
    /// Aliases: "observation", "inference" (legacy lexicon values).
    #[serde(alias = "observation", alias = "inference")]
    Insight,

    /// An unresolved question or uncertainty.
    /// Use when something is unclear and needs investigation.
    Question,

    /// An intention or course of action.
    /// Use when deciding what to do next.
    Plan,

    /// Self-reflection on identity, values, or behavior.
    /// Use during introspection about who Winter is.
    Reflection,

    /// A failure or problem encountered.
    /// Created by daemon on errors, or by Claude to record problems.
    Error,

    /// The outcome of processing a trigger.
    /// Created by daemon after handling a notification.
    /// Not available via MCP (daemon-only).
    Response,

    /// Record of a tool being called.
    /// Used for transparency about what actions Winter takes.
    ToolCall,
}

/// Daemon state record (singleton).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonState {
    /// Last seen notification indexed_at timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_cursor: Option<String>,
    /// Last seen DM sent_at timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dm_cursor: Option<String>,
    /// When this state record was created.
    pub created_at: DateTime<Utc>,
    /// When this state record was last updated.
    pub last_updated: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn tid_now_generates_13_chars() {
        let tid = Tid::now();
        assert_eq!(tid.0.len(), 13, "TID should be 13 characters");
    }

    #[test]
    fn tid_now_uses_valid_base32_charset() {
        // Base32-sortable charset: digits 2-7 and letters a-z
        let valid_chars: HashSet<char> = "234567abcdefghijklmnopqrstuvwxyz".chars().collect();

        for _ in 0..100 {
            let tid = Tid::now();
            for c in tid.0.chars() {
                assert!(
                    valid_chars.contains(&c),
                    "TID contains invalid character '{}': {}",
                    c,
                    tid.0
                );
            }
        }
    }

    #[test]
    fn tid_now_generates_unique_values() {
        // Generate many TIDs rapidly and check for uniqueness
        let tids: Vec<Tid> = (0..1000).map(|_| Tid::now()).collect();
        let unique: HashSet<_> = tids.iter().map(|t| &t.0).collect();

        // Allow some collisions due to same-microsecond generation
        // but most should be unique
        assert!(
            unique.len() > 900,
            "Expected >90% unique TIDs, got {} out of 1000",
            unique.len()
        );
    }

    #[test]
    fn tid_from_u64_produces_sortable_output() {
        // TIDs should be lexicographically sortable by time
        let tid1 = Tid::from_u64(1000);
        let tid2 = Tid::from_u64(2000);
        let tid3 = Tid::from_u64(3000);

        assert!(tid1.0 < tid2.0, "tid1 should sort before tid2");
        assert!(tid2.0 < tid3.0, "tid2 should sort before tid3");
    }

    #[test]
    fn tid_from_u64_is_deterministic() {
        // Same input should produce same output
        let tid1 = Tid::from_u64(12345678901234);
        let tid2 = Tid::from_u64(12345678901234);

        assert_eq!(tid1.0, tid2.0);
    }

    #[test]
    fn tid_from_u64_zero() {
        let tid = Tid::from_u64(0);
        assert_eq!(tid.0.len(), 13);
        // All zeros should encode to all '2's (first char in base32-sortable)
        assert_eq!(tid.0, "2222222222222");
    }

    #[test]
    fn tid_from_u64_max_value() {
        // Test with maximum 63-bit value (preserves ATProto spec constraint)
        let max_63_bit = (1u64 << 63) - 1;
        let tid = Tid::from_u64(max_63_bit);
        assert_eq!(tid.0.len(), 13);
    }

    #[test]
    fn tid_display_matches_inner_string() {
        let tid = Tid::now();
        assert_eq!(format!("{}", tid), tid.0);
    }

    #[test]
    fn tid_from_string_preserves_value() {
        let original = "3abc4def5ghi6";
        let tid = Tid::from(original.to_string());
        assert_eq!(tid.0, original);
    }

    #[test]
    fn tid_from_str_preserves_value() {
        let original = "3abc4def5ghi6";
        let tid = Tid::from(original);
        assert_eq!(tid.0, original);
    }

    #[test]
    fn tid_serializes_as_string() {
        let tid = Tid::from("3abc4def5ghi6");
        let json = serde_json::to_string(&tid).unwrap();
        assert_eq!(json, "\"3abc4def5ghi6\"");
    }

    #[test]
    fn tid_deserializes_from_string() {
        let json = "\"3abc4def5ghi6\"";
        let tid: Tid = serde_json::from_str(json).unwrap();
        assert_eq!(tid.0, "3abc4def5ghi6");
    }

    // Metamorphic test: later TIDs should always sort after earlier ones
    #[test]
    fn tid_ordering_is_chronological() {
        let mut prev = Tid::now();

        // Sleep briefly to ensure different microsecond
        std::thread::sleep(std::time::Duration::from_micros(10));

        for _ in 0..10 {
            let curr = Tid::now();
            // Later TID should sort >= previous (equal if same microsecond)
            assert!(
                curr.0 >= prev.0,
                "TIDs should be chronologically sortable: {} should be >= {}",
                curr.0,
                prev.0
            );
            prev = curr;
            std::thread::sleep(std::time::Duration::from_micros(10));
        }
    }
}
