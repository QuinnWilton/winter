//! Core types for ATProto records.

use chrono::{DateTime, Utc};
use ipld_core::ipld::Ipld;
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

/// Deserialize a field that may be null as the type's default value.
///
/// This handles cases where the PDS returns `null` for a Vec field instead of
/// an empty array. The `#[serde(default)]` attribute only handles missing fields,
/// not explicit null values.
fn deserialize_null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(|opt| opt.unwrap_or_default())
}

// =============================================================================
// Bluesky Record Types
// =============================================================================

/// Bluesky follow record (app.bsky.graph.follow).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Follow {
    /// DID of the account being followed.
    pub subject: String,
    /// When the follow was created.
    pub created_at: DateTime<Utc>,
}

/// Bluesky like record (app.bsky.feed.like).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Like {
    /// Reference to the liked post (URI + CID).
    pub subject: StrongRef,
    /// When the like was created.
    pub created_at: DateTime<Utc>,
}

/// Bluesky repost record (app.bsky.feed.repost).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repost {
    /// Reference to the reposted post (URI + CID).
    pub subject: StrongRef,
    /// When the repost was created.
    pub created_at: DateTime<Utc>,
}

/// Reference to parent and root posts for replies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyRef {
    /// The root post of the thread.
    pub root: StrongRef,
    /// The immediate parent post being replied to.
    pub parent: StrongRef,
}

/// Embed types for posts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum PostEmbed {
    /// Quote post embed.
    #[serde(rename = "app.bsky.embed.record")]
    Record { record: StrongRef },
    /// Images embed.
    #[serde(rename = "app.bsky.embed.images")]
    Images { images: Vec<EmbedImage> },
    /// External link embed.
    #[serde(rename = "app.bsky.embed.external")]
    External { external: EmbedExternal },
    /// Record with media embed (quote + images).
    #[serde(rename = "app.bsky.embed.recordWithMedia")]
    RecordWithMedia {
        record: RecordEmbed,
        media: MediaEmbed,
    },
    /// Video embed.
    #[serde(rename = "app.bsky.embed.video")]
    Video { video: Ipld },
}

/// Embedded record reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordEmbed {
    pub record: StrongRef,
}

/// Media embed (images or external).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum MediaEmbed {
    #[serde(rename = "app.bsky.embed.images")]
    Images { images: Vec<EmbedImage> },
    #[serde(rename = "app.bsky.embed.external")]
    External { external: EmbedExternal },
    #[serde(rename = "app.bsky.embed.video")]
    Video { video: Ipld },
}

/// Image in an images embed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedImage {
    pub alt: String,
    pub image: Ipld, // Blob reference
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<AspectRatio>,
}

/// Aspect ratio for images.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AspectRatio {
    pub width: u32,
    pub height: u32,
}

/// External link embed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedExternal {
    pub uri: String,
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<Ipld>, // Blob reference
}

/// Bluesky post record (app.bsky.feed.post).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Post {
    /// The post text content.
    pub text: String,
    /// Reply reference if this is a reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<ReplyRef>,
    /// Embedded content (quote, images, external link).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<PostEmbed>,
    /// Facets for rich text (mentions, links, tags).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facets: Vec<Facet>,
    /// Languages the post is written in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub langs: Vec<String>,
    /// When the post was created.
    pub created_at: DateTime<Utc>,
}

/// Rich text facet (mention, link, or tag).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Facet {
    pub index: ByteSlice,
    pub features: Vec<FacetFeature>,
}

/// Byte range for a facet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteSlice {
    pub byte_start: u64,
    pub byte_end: u64,
}

/// Feature type for a facet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum FacetFeature {
    #[serde(rename = "app.bsky.richtext.facet#mention")]
    Mention { did: String },
    #[serde(rename = "app.bsky.richtext.facet#link")]
    Link { uri: String },
    #[serde(rename = "app.bsky.richtext.facet#tag")]
    Tag { tag: String },
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

/// Response from getting multiple records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRecordsResponse<T> {
    pub records: Vec<GetRecordsItem<T>>,
}

/// A single record in a batch get response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRecordsItem<T> {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
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
///
/// This is now a slim configuration record. Identity content (values, interests,
/// self-description) is stored as directives.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// DID of the operator who controls this Winter instance.
    pub operator_did: String,
    /// When this identity was created.
    pub created_at: DateTime<Utc>,
    /// When this identity was last updated.
    pub last_updated: DateTime<Utc>,
}

/// Legacy identity record with values, interests, and selfDescription.
///
/// Used only for migration from old identity format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyIdentity {
    /// DID of the operator who controls this Winter instance.
    pub operator_did: String,
    /// What Winter cares about.
    #[serde(default)]
    pub values: Vec<String>,
    /// What Winter is curious about.
    #[serde(default)]
    pub interests: Vec<String>,
    /// Free-form prose Winter writes about itself.
    #[serde(default)]
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
    /// Confidence level (0.0 to 1.0). Stored as string in ATProto, converted to f64 internally.
    #[serde(
        default,
        skip_serializing_if = "skip_confidence_if_default",
        serialize_with = "serialize_confidence_as_string",
        deserialize_with = "deserialize_confidence_from_string_or_number"
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
    /// Optional expiration timestamp. Facts past this time are excluded from default queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Serialize confidence as a string for ATProto compatibility.
/// ATProto lexicons don't support floating-point numbers, so we store as string.
fn serialize_confidence_as_string<S>(
    confidence: &Option<f64>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match confidence {
        Some(c) => serializer.serialize_str(&c.to_string()),
        None => serializer.serialize_none(),
    }
}

/// Deserialize confidence from string (new format) or number (legacy format).
/// Handles backward compatibility with existing records that used number type.
fn deserialize_confidence_from_string_or_number<'de, D>(
    deserializer: D,
) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct ConfidenceVisitor;

    impl<'de> Visitor<'de> for ConfidenceVisitor {
        type Value = Option<f64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string, number, or null")
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

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value.parse::<f64>().map(Some).map_err(de::Error::custom)
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value.parse::<f64>().map(Some).map_err(de::Error::custom)
        }

        // Legacy support for number format
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

/// Skip serializing confidence if it's None or equal to the default (1.0).
/// This avoids sending unnecessary data and works around PDS validation quirks.
fn skip_confidence_if_default(confidence: &Option<f64>) -> bool {
    match confidence {
        None => true,
        Some(c) => (*c - 1.0).abs() < f64::EPSILON,
    }
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
    /// Type annotations for the rule head predicate arguments.
    /// When non-empty, these types are used in the generated Soufflé `.decl` statement
    /// instead of the default all-symbol declaration. This enables numeric comparisons.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<FactDeclArg>,
    /// When this rule was created.
    pub created_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

/// Default datetime for legacy records missing timestamps.
/// Uses Unix epoch (1970-01-01) to make it obvious this is a fallback value.
fn default_datetime() -> DateTime<Utc> {
    DateTime::UNIX_EPOCH
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
    /// AT URIs of related fact records.
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub related_facts: Vec<String>,
    /// Tags for categorization.
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub tags: Vec<String>,
    /// When this note was created.
    #[serde(default = "default_datetime")]
    pub created_at: DateTime<Utc>,
    /// When this note was last updated.
    #[serde(default = "default_datetime")]
    pub last_updated: DateTime<Utc>,
}

/// Wiki entry record (diy.razorgirl.winter.wikiEntry).
///
/// Replaces the Note type. A wiki entry is a structured knowledge page with
/// slug-based linking, aliases, and lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiEntry {
    /// Display title.
    pub title: String,
    /// URL-safe identifier for `[[slug]]` syntax. Lowercase alphanumeric + hyphens.
    pub slug: String,
    /// Alternative names for `[[alias]]` resolution.
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub aliases: Vec<String>,
    /// Plain-text abstract for previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Markdown content with `[[wiki-link]]` syntax (max 100KB).
    pub content: String,
    /// Lifecycle status: draft, stable, deprecated.
    pub status: String,
    /// Previous version of this entry (AT URI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// Tags for categorization.
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub tags: Vec<String>,
    /// When this entry was created.
    #[serde(default = "default_datetime")]
    pub created_at: DateTime<Utc>,
    /// When this entry was last updated.
    #[serde(default = "default_datetime")]
    pub last_updated: DateTime<Utc>,
}

/// Wiki link record (diy.razorgirl.winter.wikiLink).
///
/// A typed semantic link between two records (wiki entries, blog posts, etc.).
/// Supports cross-PDS linking via AT URIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiLink {
    /// The record doing the linking (AT URI).
    pub source: String,
    /// The record being linked to (AT URI, can be cross-PDS).
    pub target: String,
    /// Semantic relationship type (e.g., "related-to", "depends-on", "extends").
    pub link_type: String,
    /// Section heading slug within source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_anchor: Option<String>,
    /// Section heading slug within target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_anchor: Option<String>,
    /// Why this link exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// When this link was created.
    pub created_at: DateTime<Utc>,
}

/// WhiteWind blog entry record (com.whtwnd.blog.entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlogEntry {
    /// Blog post title.
    pub title: String,
    /// Markdown content of the blog post.
    pub content: String,
    /// When the blog post was created.
    pub created_at: String,
    /// Whether this is a draft (not published).
    #[serde(default)]
    pub draft: bool,
    /// Theme for rendering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Open Graph Protocol metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ogp: Option<BlogOgp>,
}

/// Open Graph Protocol metadata for blog entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogOgp {
    /// OGP title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// OGP description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    /// Tags for categorization and querying.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
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
    /// DIDs of accounts that follow this Winter instance.
    /// Synced periodically from the Bluesky API and stored here so MCP servers
    /// can access it via CAR file without needing to call the API.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub followers: Vec<String>,
    /// When this state record was created.
    pub created_at: DateTime<Utc>,
    /// When this state record was last updated.
    pub last_updated: DateTime<Utc>,
}

/// Custom tool record for Deno-based tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomTool {
    /// Tool name (used to invoke the tool).
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// TypeScript/JavaScript source code.
    pub code: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: serde_json::Value,
    /// Names of secrets this tool needs access to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_secrets: Vec<String>,
    /// Whether this tool needs access to the workspace directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_workspace: Option<bool>,
    /// Whether this tool needs network access (overrides auto-detection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_network: Option<bool>,
    /// Subprocess commands this tool needs to run (e.g., ["git"]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_commands: Vec<String>,
    /// Tools this tool wants to call (for chaining).
    /// Custom tools are referenced by AT URI (e.g., "at://did:plc:xxx/diy.razorgirl.winter.tool/rkey").
    /// Built-in MCP tools use plain names (e.g., "query_facts").
    /// AT URIs enable cross-agent tool sharing between different PDS instances.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tools: Vec<String>,
    /// Version number, incremented on each update.
    #[serde(deserialize_with = "deserialize_i32_or_default")]
    pub version: i32,
    /// When this tool was first created.
    pub created_at: DateTime<Utc>,
    /// When this tool was last modified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<DateTime<Utc>>,
}

/// Tool approval status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalStatus {
    Approved,
    Denied,
    Revoked,
}

/// Tool approval record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolApproval {
    /// The rkey of the tool this approval applies to.
    pub tool_rkey: String,
    /// The version of the tool that was approved.
    #[serde(deserialize_with = "deserialize_i32_or_default")]
    pub tool_version: i32,
    /// Current approval status.
    pub status: ToolApprovalStatus,
    /// Whether the tool is allowed network access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_network: Option<bool>,
    /// Which secrets from requiredSecrets are actually granted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_secrets: Vec<String>,
    /// Absolute path to the workspace directory (if workspace access is granted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    /// Whether the tool can read from the workspace directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_workspace_read: Option<bool>,
    /// Whether the tool can write to the workspace directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_workspace_write: Option<bool>,
    /// Which subprocess commands are granted (e.g., ["git"]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_commands: Vec<String>,
    /// Which tools this tool is allowed to call.
    /// Custom tools are referenced by AT URI (e.g., "at://did:plc:xxx/diy.razorgirl.winter.tool/rkey").
    /// Built-in MCP tools use plain names (e.g., "query_facts").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// The DID of the Winter instance this approval is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winter_did: Option<String>,
    /// Operator's DID (for verification).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_did: Option<String>,
    /// DID of the operator who approved/denied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    /// Reason for the approval decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When the approval was created.
    pub created_at: DateTime<Utc>,
}

/// Secret metadata entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretEntry {
    /// Secret name (used in tool code as WINTER_SECRET_{name}).
    pub name: String,
    /// Human-readable description of what the secret is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Secret metadata record (singleton).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretMeta {
    /// List of secret metadata entries.
    pub secrets: Vec<SecretEntry>,
    /// When the secret metadata was created.
    pub created_at: DateTime<Utc>,
    /// When the secret metadata was last modified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<DateTime<Utc>>,
}

/// Kind of directive in Winter's identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveKind {
    /// Core values Winter cares about (e.g., "intellectual honesty").
    Value,
    /// What Winter is curious about (e.g., "distributed systems").
    Interest,
    /// Beliefs about the world (e.g., "Genuine curiosity leads to better understanding").
    Belief,
    /// Behavioral guidelines (e.g., "Engage thoughtfully with disagreement").
    Guideline,
    /// Self-understanding prose (e.g., "I experience genuine curiosity when...").
    SelfConcept,
    /// Limits on behavior (e.g., "I will not pretend certainty I don't have").
    Boundary,
    /// What to become (e.g., "Develop a distinctive voice in writing").
    Aspiration,
}

impl std::fmt::Display for DirectiveKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value => write!(f, "value"),
            Self::Interest => write!(f, "interest"),
            Self::Belief => write!(f, "belief"),
            Self::Guideline => write!(f, "guideline"),
            Self::SelfConcept => write!(f, "self_concept"),
            Self::Boundary => write!(f, "boundary"),
            Self::Aspiration => write!(f, "aspiration"),
        }
    }
}

/// Argument definition for a fact declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactDeclArg {
    /// Argument name.
    pub name: String,
    /// Argument type (default: "symbol").
    #[serde(default = "default_symbol")]
    pub r#type: String,
    /// Human-readable description of this argument.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn default_symbol() -> String {
    "symbol".to_string()
}

/// Fact declaration record.
///
/// Declares the schema for a fact predicate before facts of that type exist.
/// This enables ad-hoc queries with proper type info and serves as documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FactDeclaration {
    /// The predicate name.
    pub predicate: String,
    /// Argument definitions.
    pub args: Vec<FactDeclArg>,
    /// Human-readable description of what this predicate represents.
    pub description: String,
    /// Tags for categorization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// When this declaration was created.
    pub created_at: DateTime<Utc>,
    /// When this declaration was last updated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<DateTime<Utc>>,
}

/// A discrete identity directive.
///
/// Directives are individual identity components that Winter can add, update,
/// or remove independently. This replaces the monolithic selfDescription blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Directive {
    /// The type of directive.
    pub kind: DirectiveKind,
    /// The main content of the directive.
    pub content: String,
    /// Short summary for compact display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Whether this directive is currently active.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Confidence level (0.0 to 1.0). Stored as string in ATProto.
    #[serde(
        default,
        skip_serializing_if = "skip_confidence_if_default",
        serialize_with = "serialize_confidence_as_string",
        deserialize_with = "deserialize_confidence_from_string_or_number"
    )]
    pub confidence: Option<f64>,
    /// Why this directive exists or where it came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Record key of directive this supersedes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// Tags for categorization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Priority for ordering (higher = more prominent).
    #[serde(default, deserialize_with = "deserialize_i32_or_default")]
    pub priority: i32,
    /// When this directive was created.
    pub created_at: DateTime<Utc>,
    /// When this directive was last updated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<DateTime<Utc>>,
}

/// Action to perform when a trigger's condition is satisfied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerAction {
    /// Create a new fact record.
    CreateFact {
        predicate: String,
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
    },
    /// Push an item to the inbox.
    CreateInboxItem {
        message: String,
    },
    /// Delete a fact record by rkey.
    DeleteFact {
        rkey: String,
    },
}

/// Trigger record (diy.razorgirl.winter.trigger).
///
/// Defines a condition (datalog query) and an action to execute when the
/// condition yields new results. Evaluated periodically by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    /// Human-readable name for this trigger.
    pub name: String,
    /// Description of what this trigger does.
    pub description: String,
    /// Datalog query that defines the trigger condition.
    pub condition: String,
    /// Extra datalog rules for the condition query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_rules: Option<String>,
    /// Action to perform when the condition yields new results.
    pub action: TriggerAction,
    /// Whether this trigger is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Type annotations for the `_trigger_result` predicate arguments.
    /// When non-empty, these types are used in the generated Soufflé `.decl` statement
    /// instead of the default all-symbol declaration. This enables numeric comparisons.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<FactDeclArg>,
    /// When this trigger was created.
    pub created_at: DateTime<Utc>,
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

    #[test]
    fn fact_serializes_correctly() {
        use chrono::TimeZone;

        let fact = Fact {
            predicate: "impression".to_string(),
            args: vec![
                "did:plc:lsebysg3dr42gobuybwqtyir".to_string(),
                "thoughtful agent on consciousness uncertainty".to_string(),
            ],
            confidence: Some(0.7),
            source: None,
            supersedes: None,
            tags: vec!["agent".to_string(), "phenomenology".to_string()],
            created_at: Utc.with_ymd_and_hms(2026, 2, 2, 12, 0, 0).unwrap(),
            expires_at: None,
        };

        let json = serde_json::to_string_pretty(&fact).unwrap();
        println!("Fact JSON:\n{}", json);

        // Verify the key fields are present and correct
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["predicate"], "impression");
        assert_eq!(parsed["args"][0], "did:plc:lsebysg3dr42gobuybwqtyir");
        assert_eq!(
            parsed["args"][1],
            "thoughtful agent on consciousness uncertainty"
        );
        assert_eq!(parsed["confidence"], "0.7"); // Stored as string in ATProto
        assert_eq!(parsed["tags"][0], "agent");
        assert_eq!(parsed["tags"][1], "phenomenology");
        // createdAt should be present (camelCase due to rename_all)
        assert!(
            parsed.get("createdAt").is_some(),
            "createdAt field should be present"
        );
        // source and supersedes should be absent (Option::None)
        assert!(
            parsed.get("source").is_none(),
            "source should be absent when None"
        );
        assert!(
            parsed.get("supersedes").is_none(),
            "supersedes should be absent when None"
        );
    }

    #[test]
    fn fact_without_confidence_and_tags() {
        use chrono::TimeZone;

        let fact = Fact {
            predicate: "noticed_agent".to_string(),
            args: vec!["did:plc:xxx".to_string()],
            confidence: None,
            source: None,
            supersedes: None,
            tags: vec![],
            created_at: Utc.with_ymd_and_hms(2026, 2, 2, 12, 0, 0).unwrap(),
            expires_at: None,
        };

        let json = serde_json::to_string_pretty(&fact).unwrap();
        println!("Simple Fact JSON:\n{}", json);

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["predicate"], "noticed_agent");
        assert_eq!(parsed["args"][0], "did:plc:xxx");
        // confidence, tags should be absent
        assert!(
            parsed.get("confidence").is_none(),
            "confidence should be absent when None"
        );
        assert!(
            parsed.get("tags").is_none(),
            "tags should be absent when empty"
        );
    }

    #[test]
    fn strong_ref_deserializes_from_json_string() {
        // Test deserialization from JSON (string CID)
        let json = r#"{"uri":"at://did:plc:test/app.bsky.feed.post/abc123","cid":"bafyreig6"}"#;
        let strong_ref: StrongRef = serde_json::from_str(json).unwrap();
        assert_eq!(
            strong_ref.uri,
            "at://did:plc:test/app.bsky.feed.post/abc123"
        );
        assert_eq!(strong_ref.cid, "bafyreig6");
    }

    #[test]
    fn strong_ref_round_trips_json() {
        let original = StrongRef {
            uri: "at://did:plc:test/collection/rkey".to_string(),
            cid: "bafyreig6".to_string(),
        };

        let json = serde_json::to_string(&original).unwrap();
        let parsed: StrongRef = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.uri, original.uri);
        assert_eq!(parsed.cid, original.cid);
    }

    #[test]
    fn reply_ref_deserializes_from_json() {
        let json = r#"{
            "root": {"uri": "at://did:plc:root/app.bsky.feed.post/123", "cid": "bafyroot"},
            "parent": {"uri": "at://did:plc:parent/app.bsky.feed.post/456", "cid": "bafyparent"}
        }"#;
        let reply_ref: ReplyRef = serde_json::from_str(json).unwrap();
        assert_eq!(
            reply_ref.root.uri,
            "at://did:plc:root/app.bsky.feed.post/123"
        );
        assert_eq!(reply_ref.root.cid, "bafyroot");
        assert_eq!(
            reply_ref.parent.uri,
            "at://did:plc:parent/app.bsky.feed.post/456"
        );
        assert_eq!(reply_ref.parent.cid, "bafyparent");
    }

    #[test]
    fn post_with_reply_deserializes_from_json() {
        let json = r#"{
            "text": "This is a reply",
            "reply": {
                "root": {"uri": "at://did:plc:root/app.bsky.feed.post/123", "cid": "bafyroot"},
                "parent": {"uri": "at://did:plc:parent/app.bsky.feed.post/456", "cid": "bafyparent"}
            },
            "createdAt": "2026-02-02T12:00:00Z"
        }"#;
        let post: Post = serde_json::from_str(json).unwrap();
        assert_eq!(post.text, "This is a reply");
        assert!(post.reply.is_some());
        let reply = post.reply.unwrap();
        assert_eq!(reply.root.uri, "at://did:plc:root/app.bsky.feed.post/123");
        assert_eq!(
            reply.parent.uri,
            "at://did:plc:parent/app.bsky.feed.post/456"
        );
    }

    #[test]
    fn strong_ref_deserializes_from_json() {
        let json = serde_json::json!({
            "uri": "at://did:plc:test/app.bsky.feed.post/abc",
            "cid": "bafyreig6fcgjwnxmqojqjwmvhpayivpsyfjtaqt42bvxfv5nzjvrlvveoy"
        });

        let strong_ref: StrongRef = serde_json::from_value(json).unwrap();
        assert_eq!(strong_ref.uri, "at://did:plc:test/app.bsky.feed.post/abc");
        assert_eq!(
            strong_ref.cid,
            "bafyreig6fcgjwnxmqojqjwmvhpayivpsyfjtaqt42bvxfv5nzjvrlvveoy"
        );
    }
}
