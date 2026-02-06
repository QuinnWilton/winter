//! CAR file parsing for ATProto repositories.
//!
//! Parses CAR v1 files and extracts records from the ATProto MST structure.

use std::collections::HashMap;
use std::io::Cursor;

use ipld_core::cid::Cid;
use iroh_car::CarReader;
use serde::de::DeserializeOwned;
use tracing::{debug, trace, warn};

use crate::dispatch::extract_record_to_result;
use crate::{
    AtprotoError, BlogEntry, CustomTool, DaemonState, Directive, Fact, FactDeclaration, Follow,
    IDENTITY_COLLECTION, IDENTITY_KEY, Identity, Job, Like, Note, Post, Repost, Rule,
    STATE_COLLECTION, STATE_KEY, Thought, ToolApproval, WikiEntry, WikiLink,
};

/// Result of parsing a CAR file.
#[derive(Debug, Default)]
pub struct CarParseResult {
    /// Facts extracted from the repo, keyed by rkey.
    pub facts: HashMap<String, (Fact, String)>,
    /// Rules extracted from the repo, keyed by rkey.
    pub rules: HashMap<String, (Rule, String)>,
    /// Thoughts extracted from the repo, keyed by rkey.
    pub thoughts: HashMap<String, (Thought, String)>,
    /// Notes extracted from the repo, keyed by rkey.
    pub notes: HashMap<String, (Note, String)>,
    /// Jobs extracted from the repo, keyed by rkey.
    pub jobs: HashMap<String, (Job, String)>,
    /// Identity (singleton), if present.
    pub identity: Option<(Identity, String)>,
    /// Daemon state (singleton), if present.
    pub daemon_state: Option<(DaemonState, String)>,
    /// The repo revision from the commit.
    pub rev: Option<String>,
    // =========================================================================
    // Bluesky records (for derived facts)
    // =========================================================================
    /// Follows extracted from the repo, keyed by rkey.
    pub follows: HashMap<String, (Follow, String)>,
    /// Likes extracted from the repo, keyed by rkey.
    pub likes: HashMap<String, (Like, String)>,
    /// Reposts extracted from the repo, keyed by rkey.
    pub reposts: HashMap<String, (Repost, String)>,
    /// Posts extracted from the repo, keyed by rkey.
    pub posts: HashMap<String, (Post, String)>,
    // =========================================================================
    // Winter records (for derived facts)
    // =========================================================================
    /// Directives extracted from the repo, keyed by rkey.
    pub directives: HashMap<String, (Directive, String)>,
    /// Fact declarations extracted from the repo, keyed by rkey.
    pub declarations: HashMap<String, (FactDeclaration, String)>,
    /// Custom tools extracted from the repo, keyed by rkey.
    pub tools: HashMap<String, (CustomTool, String)>,
    /// Tool approvals extracted from the repo, keyed by rkey.
    pub tool_approvals: HashMap<String, (ToolApproval, String)>,
    /// Blog entries extracted from the repo, keyed by rkey.
    pub blog_entries: HashMap<String, (BlogEntry, String)>,
    /// Wiki entries extracted from the repo, keyed by rkey.
    pub wiki_entries: HashMap<String, (WikiEntry, String)>,
    /// Wiki links extracted from the repo, keyed by rkey.
    pub wiki_links: HashMap<String, (WikiLink, String)>,
}

/// Parse a CAR file and extract Winter facts and rules.
///
/// The CAR file contains:
/// 1. A header with roots
/// 2. Blocks keyed by CID
/// 3. The first root is typically the signed commit
/// 4. The commit references an MST root
/// 5. MST nodes contain records organized by collection/rkey
pub async fn parse_car(car_bytes: &[u8]) -> Result<CarParseResult, AtprotoError> {
    let mut result = CarParseResult::default();

    // Parse CAR file
    let cursor = Cursor::new(car_bytes);
    let mut reader = CarReader::new(cursor)
        .await
        .map_err(|e| AtprotoError::CarParse(format!("failed to read CAR header: {}", e)))?;

    // Collect all blocks by CID
    let mut blocks: HashMap<String, Vec<u8>> = HashMap::new();
    let roots = reader.header().roots().to_vec();

    debug!(roots = ?roots, "CAR file has roots");

    loop {
        match reader.next_block().await {
            Ok(Some((cid, data))) => {
                blocks.insert(cid.to_string(), data);
            }
            Ok(None) => break,
            Err(e) => {
                return Err(AtprotoError::CarParse(format!(
                    "failed to read block: {}",
                    e
                )));
            }
        }
    }

    debug!(block_count = blocks.len(), "parsed CAR blocks");

    if roots.is_empty() {
        return Err(AtprotoError::CarParse("CAR file has no roots".to_string()));
    }

    // Parse the commit (first root)
    let commit_cid = roots[0].to_string();
    let commit_data = blocks
        .get(&commit_cid)
        .ok_or_else(|| AtprotoError::CarParse("commit block not found".to_string()))?;

    let commit: Commit = parse_cbor(commit_data)?;
    result.rev = Some(commit.rev.clone());
    debug!(rev = %commit.rev, "parsed commit");

    // Parse the MST starting from data root
    parse_mst_node(&commit.data.to_string(), &blocks, "", &mut result)?;

    debug!(
        facts = result.facts.len(),
        rules = result.rules.len(),
        thoughts = result.thoughts.len(),
        notes = result.notes.len(),
        jobs = result.jobs.len(),
        follows = result.follows.len(),
        likes = result.likes.len(),
        reposts = result.reposts.len(),
        posts = result.posts.len(),
        directives = result.directives.len(),
        tools = result.tools.len(),
        tool_approvals = result.tool_approvals.len(),
        blog_entries = result.blog_entries.len(),
        wiki_entries = result.wiki_entries.len(),
        wiki_links = result.wiki_links.len(),
        has_identity = result.identity.is_some(),
        has_daemon_state = result.daemon_state.is_some(),
        "extracted records from CAR"
    );

    Ok(result)
}

/// ATProto signed commit structure (repo format v3).
///
/// Per ATProto spec: https://atproto.com/specs/repository
#[derive(Debug, serde::Deserialize)]
struct Commit {
    /// DID of the repo (required).
    #[allow(dead_code)]
    did: String,
    /// Repo format version (required, must be 3).
    #[allow(dead_code)]
    version: u32,
    /// The data MST root CID (required).
    data: Cid,
    /// Repository revision in TID format (required).
    rev: String,
    /// Previous commit CID (nullable, virtually always null in v3).
    #[allow(dead_code)]
    prev: Option<Cid>,
    /// Cryptographic signature as raw bytes (required).
    #[allow(dead_code)]
    #[serde(with = "serde_bytes")]
    sig: Vec<u8>,
}

/// ATProto MST node structure (NodeData).
///
/// Per ATProto spec: https://atproto.com/specs/repository
/// MST nodes contain a list of entries and optional left pointer.
#[derive(Debug, serde::Deserialize)]
struct MstNode {
    /// Left subtree CID (nullable) - link to subtree with lexically earlier keys.
    #[serde(rename = "l")]
    left: Option<Cid>,
    /// Entries in this node (required, ordered list).
    #[serde(rename = "e", default)]
    entries: Vec<MstEntry>,
}

/// An entry in an MST node (TreeEntry).
///
/// Per ATProto spec: https://atproto.com/specs/repository
#[derive(Debug, serde::Deserialize)]
struct MstEntry {
    /// Prefix length - count of bytes shared with previous TreeEntry (required).
    /// First entry in a node must have p=0.
    /// Uses flexible deserialization to handle various CBOR integer encodings.
    #[serde(rename = "p", default, deserialize_with = "deserialize_usize")]
    prefix_len: usize,
    /// Key suffix - remaining key bytes after prefix (required).
    #[serde(rename = "k")]
    key_suffix: serde_bytes::ByteBuf,
    /// Value CID - link to record data (required per spec, but internal nodes may omit).
    #[serde(rename = "v")]
    value: Option<Cid>,
    /// Right subtree CID (nullable) - link to subtree between this and next entry.
    #[serde(rename = "t")]
    tree: Option<Cid>,
}

/// Deserialize a usize from any CBOR integer type.
fn deserialize_usize<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct UsizeVisitor;

    impl<'de> Visitor<'de> for UsizeVisitor {
        type Value = usize;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a non-negative integer")
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value >= 0 {
                Ok(value as usize)
            } else {
                Ok(0) // Treat negative as 0 for prefix_len
            }
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(value as usize)
        }
    }

    deserializer.deserialize_any(UsizeVisitor)
}

/// Parse a CBOR-encoded value.
fn parse_cbor<T: DeserializeOwned>(data: &[u8]) -> Result<T, AtprotoError> {
    // Use serde_ipld_dagcbor for proper CBOR parsing
    serde_ipld_dagcbor::from_slice(data).map_err(|e| AtprotoError::CborDecode(format!("{}", e)))
}

/// Recursively parse an MST node and extract records.
fn parse_mst_node(
    cid: &str,
    blocks: &HashMap<String, Vec<u8>>,
    key_prefix: &str,
    result: &mut CarParseResult,
) -> Result<(), AtprotoError> {
    let data = match blocks.get(cid) {
        Some(d) => d,
        None => {
            warn!(cid = %cid, "MST node block not found");
            return Ok(());
        }
    };

    let node: MstNode = parse_cbor(data)?;

    trace!(
        cid = %cid,
        entries = node.entries.len(),
        has_left = node.left.is_some(),
        "parsing MST node"
    );

    // Process left subtree first
    if let Some(ref left) = node.left {
        parse_mst_node(&left.to_string(), blocks, key_prefix, result)?;
    }

    // Process entries
    let mut prev_key = key_prefix.to_string();

    for entry in &node.entries {
        // Validate key suffix length
        const MAX_KEY_SUFFIX_LEN: usize = 512;
        if entry.key_suffix.len() > MAX_KEY_SUFFIX_LEN {
            warn!(
                suffix_len = entry.key_suffix.len(),
                max = MAX_KEY_SUFFIX_LEN,
                "key suffix too long, skipping entry"
            );
            continue;
        }

        // Build the full key from prefix and suffix
        let key_suffix = String::from_utf8_lossy(&entry.key_suffix);

        // Per ATProto MST spec:
        // - prefix_len is count of bytes shared with PREVIOUS entry in this node
        // - First entry in any node must have prefix_len=0
        // - When prefix_len=0, the key is just key_suffix (no shared prefix)
        let full_key = if entry.prefix_len > 0 {
            if entry.prefix_len > prev_key.len() {
                warn!(
                    prefix_len = entry.prefix_len,
                    prev_key_len = prev_key.len(),
                    "prefix_len exceeds prev_key length, using key_suffix only"
                );
                key_suffix.to_string()
            } else {
                format!("{}{}", &prev_key[..entry.prefix_len], key_suffix)
            }
        } else {
            // prefix_len=0 means no shared prefix - key is just the suffix
            key_suffix.to_string()
        };

        trace!(key = %full_key, "processing MST entry");

        // If this entry has a value, it's a record
        if let Some(ref value_cid) = entry.value {
            extract_record(&full_key, &value_cid.to_string(), blocks, result)?;
        }

        // Process right subtree
        if let Some(ref tree) = entry.tree {
            parse_mst_node(&tree.to_string(), blocks, &full_key, result)?;
        }

        prev_key = full_key;
    }

    Ok(())
}

/// Extract a record from the MST.
/// Key format: "collection/rkey"
fn extract_record(
    key: &str,
    value_cid: &str,
    blocks: &HashMap<String, Vec<u8>>,
    result: &mut CarParseResult,
) -> Result<(), AtprotoError> {
    // Parse collection and rkey from the key
    let parts: Vec<&str> = key.splitn(2, '/').collect();
    if parts.len() != 2 {
        trace!(key = %key, "skipping non-record key");
        return Ok(());
    }

    let collection = parts[0];
    let rkey = parts[1];

    let data = match blocks.get(value_cid) {
        Some(d) => d,
        None => {
            warn!(cid = %value_cid, key = %key, "record block not found");
            return Ok(());
        }
    };

    // Use the dispatch macro for most record types
    let handled = extract_record_to_result(collection, rkey, value_cid, data, result);

    // Handle special cases (singletons with key checks)
    if !handled {
        match collection {
            IDENTITY_COLLECTION => {
                if rkey == IDENTITY_KEY {
                    match parse_cbor::<Identity>(data) {
                        Ok(identity) => {
                            trace!("extracted identity");
                            result.identity = Some((identity, value_cid.to_string()));
                        }
                        Err(e) => {
                            warn!(rkey = %rkey, error = %e, "failed to parse identity");
                        }
                    }
                }
            }
            STATE_COLLECTION => {
                if rkey == STATE_KEY {
                    match parse_cbor::<DaemonState>(data) {
                        Ok(state) => {
                            trace!(followers = state.followers.len(), "extracted daemon state");
                            result.daemon_state = Some((state, value_cid.to_string()));
                        }
                        Err(e) => {
                            warn!(rkey = %rkey, error = %e, "failed to parse daemon state");
                        }
                    }
                }
            }
            _ => {
                trace!(collection = %collection, rkey = %rkey, "skipping unknown collection");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_car_parse_result_default() {
        let result = CarParseResult::default();
        assert!(result.facts.is_empty());
        assert!(result.rules.is_empty());
        assert!(result.rev.is_none());
    }

    #[test]
    fn test_cid_display() {
        // Test that Cid properly displays as a string
        // Using a known valid CIDv1 (dag-cbor, sha2-256)
        let cid_str = "bafyreib2rxk3rybloqtpwxev6skqdgvlfp2ewvqkdmvfrb4fhqnjdqftsu";
        let cid: Cid = cid_str.parse().unwrap();
        assert_eq!(cid.to_string(), cid_str);
    }

    #[test]
    fn test_extract_record_invalid_key_format() {
        let blocks = HashMap::new();
        let mut result = CarParseResult::default();

        // Key without slash should be skipped
        let outcome = extract_record("no-slash-here", "somecid", &blocks, &mut result);
        assert!(outcome.is_ok());
        assert!(result.facts.is_empty());
        assert!(result.rules.is_empty());
    }

    #[test]
    fn test_extract_record_missing_block() {
        let blocks = HashMap::new();
        let mut result = CarParseResult::default();

        // Block not found should be skipped (not error)
        let outcome = extract_record(
            "diy.razorgirl.winter.fact/rkey123",
            "missing-cid",
            &blocks,
            &mut result,
        );
        assert!(outcome.is_ok());
        assert!(result.facts.is_empty());
    }

    #[test]
    fn test_extract_record_unknown_collection() {
        let mut blocks = HashMap::new();
        blocks.insert("cid123".to_string(), vec![0u8; 10]);
        let mut result = CarParseResult::default();

        // Unknown collection should be skipped
        let outcome = extract_record("app.bsky.feed.post/rkey123", "cid123", &blocks, &mut result);
        assert!(outcome.is_ok());
        assert!(result.facts.is_empty());
        assert!(result.rules.is_empty());
    }

    #[tokio::test]
    async fn test_parse_car_empty_data() {
        let result = parse_car(&[]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_parse_car_invalid_data() {
        let result = parse_car(&[0, 1, 2, 3, 4, 5]).await;
        assert!(result.is_err());
    }
}
