//! Firehose client for real-time ATProto repository updates.
//!
//! Connects to com.atproto.sync.subscribeRepos and receives commit events.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use iroh_car::CarReader;
use serde::Deserialize;
use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};

use crate::cache::{FirehoseCommit, FirehoseOp, RepoCache, SyncState};
use crate::{
    AtprotoError, FACT_COLLECTION, Fact, IDENTITY_COLLECTION, IDENTITY_KEY, Identity,
    JOB_COLLECTION, Job, NOTE_COLLECTION, Note, RULE_COLLECTION, Rule, THOUGHT_COLLECTION, Thought,
};

/// Default firehose URL (Bluesky relay).
pub const DEFAULT_FIREHOSE_URL: &str = "wss://bsky.network";

/// Firehose client for subscribing to repository updates.
pub struct FirehoseClient {
    /// WebSocket URL for firehose.
    url: String,
    /// DID to filter commits for.
    did: String,
    /// Cache to update.
    cache: Arc<RepoCache>,
}

impl FirehoseClient {
    /// Create a new firehose client.
    pub fn new(url: impl Into<String>, did: impl Into<String>, cache: Arc<RepoCache>) -> Self {
        Self {
            url: url.into(),
            did: did.into(),
            cache,
        }
    }

    /// Connect and start receiving events.
    ///
    /// This runs in a loop, reconnecting on disconnection with exponential backoff.
    /// Events are either queued (during sync) or applied directly (when live).
    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<(), AtprotoError> {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);

        loop {
            if *shutdown_rx.borrow() {
                info!("firehose client shutting down");
                return Ok(());
            }

            match self.connect_and_process(&mut shutdown_rx).await {
                Ok(()) => {
                    // Clean shutdown
                    return Ok(());
                }
                Err(e) => {
                    error!(error = %e, "firehose connection error, reconnecting");

                    // Wait with backoff
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                return Ok(());
                            }
                        }
                        _ = tokio::time::sleep(backoff) => {}
                    }

                    // Increase backoff
                    backoff = std::cmp::min(backoff * 2, max_backoff);
                }
            }
        }
    }

    /// Connect to the firehose and process messages.
    async fn connect_and_process(
        &self,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) -> Result<(), AtprotoError> {
        let url = format!("{}/xrpc/com.atproto.sync.subscribeRepos", self.url);

        info!(url = %url, did = %self.did, "connecting to firehose");

        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| AtprotoError::WebSocket(format!("connection failed: {}", e)))?;

        let (mut _write, mut read) = ws_stream.split();

        info!("firehose connected");

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("firehose received shutdown signal");
                        return Ok(());
                    }
                }
                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            if let Err(e) = self.handle_message(&data).await {
                                warn!(error = %e, "failed to handle firehose message");
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("firehose connection closed by server");
                            return Err(AtprotoError::WebSocket("connection closed".to_string()));
                        }
                        Some(Ok(_)) => {
                            // Ignore other message types (text, ping, pong)
                        }
                        Some(Err(e)) => {
                            return Err(AtprotoError::WebSocket(format!("read error: {}", e)));
                        }
                        None => {
                            return Err(AtprotoError::WebSocket("stream ended".to_string()));
                        }
                    }
                }
            }
        }
    }

    /// Handle a firehose message.
    async fn handle_message(&self, data: &[u8]) -> Result<(), AtprotoError> {
        // Firehose messages are two CBOR values concatenated: header + payload
        // First decode the header to get the message type
        let (header, payload_offset) = decode_frame_header(data)?;

        // op=1 is a regular message, op=-1 is an error
        if header.op != 1 {
            if header.op == -1 {
                // Try to extract error details from the payload
                let payload = &data[payload_offset..];
                match serde_ipld_dagcbor::from_slice::<FirehoseError>(payload) {
                    Ok(err) => {
                        error!(
                            error_type = ?err.error,
                            message = ?err.message,
                            "firehose error frame received"
                        );
                    }
                    Err(_) => {
                        error!("firehose error frame received (could not decode error details)");
                    }
                }
            }
            return Ok(());
        }

        let payload = &data[payload_offset..];

        match header.t.as_deref() {
            Some("#commit") => {
                let commit: CommitEvent = serde_ipld_dagcbor::from_slice(payload).map_err(|e| {
                    AtprotoError::CborDecode(format!("failed to decode commit event: {}", e))
                })?;
                self.handle_commit(commit).await?;
            }
            Some("#identity") => {
                trace!("ignoring identity event");
            }
            Some("#account") => {
                trace!("ignoring account event");
            }
            Some("#handle") => {
                trace!("ignoring handle event");
            }
            Some("#tombstone") => {
                trace!("ignoring tombstone event");
            }
            Some("#info") => {
                let info: InfoEvent = serde_ipld_dagcbor::from_slice(payload).map_err(|e| {
                    AtprotoError::CborDecode(format!("failed to decode info event: {}", e))
                })?;
                debug!(name = ?info.name, message = ?info.message, "firehose info");
            }
            Some(t) => {
                trace!(message_type = %t, "ignoring unknown firehose event type");
            }
            None => {
                trace!("ignoring firehose frame with no type");
            }
        }

        Ok(())
    }

    /// Handle a commit event.
    async fn handle_commit(&self, commit: CommitEvent) -> Result<(), AtprotoError> {
        // Only process commits for our DID
        if commit.repo != self.did {
            return Ok(());
        }

        trace!(
            rev = %commit.rev,
            ops = commit.ops.len(),
            "received commit for our repo"
        );

        // Parse the CAR blocks
        let blocks = if let Some(ref blocks_data) = commit.blocks {
            parse_commit_blocks(blocks_data).await?
        } else {
            HashMap::new()
        };

        // Convert to FirehoseCommit
        let mut ops = Vec::new();

        for op in &commit.ops {
            let (collection, rkey) = match parse_record_path(&op.path) {
                Some((c, r)) => (c, r),
                None => {
                    warn!(path = %op.path, "malformed record path, skipping");
                    continue;
                }
            };

            // Only care about Winter collections
            if collection != FACT_COLLECTION
                && collection != RULE_COLLECTION
                && collection != THOUGHT_COLLECTION
                && collection != NOTE_COLLECTION
                && collection != JOB_COLLECTION
                && collection != IDENTITY_COLLECTION
            {
                continue;
            }

            match op.action.as_str() {
                "create" | "update" => {
                    if let Some(ref cid) = op.cid {
                        let cid_str = format_cid(cid);
                        if let Some(record_data) = blocks.get(&cid_str) {
                            ops.push(FirehoseOp::CreateOrUpdate {
                                collection: collection.to_string(),
                                rkey: rkey.to_string(),
                                cid: cid_str,
                                record: record_data.clone(),
                            });
                        }
                    }
                }
                "delete" => {
                    ops.push(FirehoseOp::Delete {
                        collection: collection.to_string(),
                        rkey: rkey.to_string(),
                    });
                }
                _ => {
                    trace!(action = %op.action, "unknown op action");
                }
            }
        }

        if ops.is_empty() {
            return Ok(());
        }

        let firehose_commit = FirehoseCommit {
            rev: commit.rev.clone(),
            ops,
        };

        // If we're still syncing, queue the commit; otherwise apply directly
        match self.cache.state() {
            SyncState::Disconnected | SyncState::Syncing => {
                debug!(rev = %commit.rev, "queuing commit during sync");
                self.cache.queue_commit(firehose_commit).await;
            }
            SyncState::Live => {
                debug!(rev = %commit.rev, "applying commit directly");
                apply_commit(&self.cache, &firehose_commit)?;
            }
        }

        Ok(())
    }
}

/// Parse CAR blocks from a commit.
async fn parse_commit_blocks(data: &[u8]) -> Result<HashMap<String, Vec<u8>>, AtprotoError> {
    let cursor = Cursor::new(data);
    let mut reader = CarReader::new(cursor)
        .await
        .map_err(|e| AtprotoError::CarParse(format!("failed to read commit CAR: {}", e)))?;

    let mut blocks = HashMap::new();

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

    Ok(blocks)
}

/// Format a CID for lookup.
fn format_cid(cid: &ipld_core::cid::Cid) -> String {
    cid.to_string()
}

/// Parse a record path into collection and rkey.
/// Returns None if the path is malformed (empty components).
fn parse_record_path(path: &str) -> Option<(&str, &str)> {
    let mut parts = path.split('/');
    let collection = parts.next().filter(|s| !s.is_empty())?;
    let rkey = parts.next().filter(|s| !s.is_empty())?;
    Some((collection, rkey))
}

/// Apply a commit to the cache.
pub fn apply_commit(cache: &RepoCache, commit: &FirehoseCommit) -> Result<(), AtprotoError> {
    for op in &commit.ops {
        match op {
            FirehoseOp::CreateOrUpdate {
                collection,
                rkey,
                cid,
                record,
            } => {
                if collection == FACT_COLLECTION {
                    let fact: Fact = serde_ipld_dagcbor::from_slice(record).map_err(|e| {
                        AtprotoError::CborDecode(format!("failed to decode fact: {}", e))
                    })?;
                    cache.upsert_fact(rkey.clone(), fact, cid.clone());
                } else if collection == RULE_COLLECTION {
                    let rule: Rule = serde_ipld_dagcbor::from_slice(record).map_err(|e| {
                        AtprotoError::CborDecode(format!("failed to decode rule: {}", e))
                    })?;
                    cache.upsert_rule(rkey.clone(), rule, cid.clone());
                } else if collection == THOUGHT_COLLECTION {
                    let thought: Thought = serde_ipld_dagcbor::from_slice(record).map_err(|e| {
                        AtprotoError::CborDecode(format!("failed to decode thought: {}", e))
                    })?;
                    cache.upsert_thought(rkey.clone(), thought, cid.clone());
                } else if collection == NOTE_COLLECTION {
                    let note: Note = serde_ipld_dagcbor::from_slice(record).map_err(|e| {
                        AtprotoError::CborDecode(format!("failed to decode note: {}", e))
                    })?;
                    cache.upsert_note(rkey.clone(), note, cid.clone());
                } else if collection == JOB_COLLECTION {
                    let job: Job = serde_ipld_dagcbor::from_slice(record).map_err(|e| {
                        AtprotoError::CborDecode(format!("failed to decode job: {}", e))
                    })?;
                    cache.upsert_job(rkey.clone(), job, cid.clone());
                } else if collection == IDENTITY_COLLECTION && rkey == IDENTITY_KEY {
                    let identity: Identity =
                        serde_ipld_dagcbor::from_slice(record).map_err(|e| {
                            AtprotoError::CborDecode(format!("failed to decode identity: {}", e))
                        })?;
                    // Use a blocking approach since this is sync code
                    if let Ok(rt) = tokio::runtime::Handle::try_current() {
                        rt.block_on(cache.set_identity(identity, cid.clone()));
                    }
                }
            }
            FirehoseOp::Delete { collection, rkey } => {
                if collection == FACT_COLLECTION {
                    cache.delete_fact(rkey);
                } else if collection == RULE_COLLECTION {
                    cache.delete_rule(rkey);
                } else if collection == THOUGHT_COLLECTION {
                    cache.delete_thought(rkey);
                } else if collection == NOTE_COLLECTION {
                    cache.delete_note(rkey);
                } else if collection == JOB_COLLECTION {
                    cache.delete_job(rkey);
                }
                // Identity is a singleton and typically not deleted
            }
        }
    }

    Ok(())
}

// Firehose frame header (first CBOR value in each message)

#[derive(Debug, Deserialize)]
struct FrameHeader {
    /// Operation: 1 = message, -1 = error
    op: i32,
    /// Message type (e.g., "#commit", "#identity")
    t: Option<String>,
}

/// Decode the frame header and return it along with the offset to the payload.
fn decode_frame_header(data: &[u8]) -> Result<(FrameHeader, usize), AtprotoError> {
    // We need to find where the header CBOR ends and payload begins.
    // Use ciborium which properly handles partial reads from a cursor.
    let mut cursor = Cursor::new(data);
    let header: FrameHeader = ciborium::from_reader(&mut cursor)
        .map_err(|e| AtprotoError::CborDecode(format!("failed to decode frame header: {}", e)))?;
    let offset = cursor.position() as usize;
    Ok((header, offset))
}

#[derive(Debug, Deserialize)]
struct CommitEvent {
    /// Sequence number.
    #[allow(dead_code)]
    seq: i64,
    /// DEPRECATED: Was used to indicate rebased commits.
    #[allow(dead_code)]
    #[serde(default)]
    rebase: bool,
    /// DEPRECATED: Was used to indicate oversized commits.
    #[allow(dead_code)]
    #[serde(rename = "tooBig", default)]
    too_big: bool,
    /// Repository DID.
    repo: String,
    /// Repo commit object CID.
    #[allow(dead_code)]
    commit: ipld_core::cid::Cid,
    /// Repository revision (TID format).
    rev: String,
    /// Revision of the previous commit (null for first commit).
    #[allow(dead_code)]
    since: Option<String>,
    /// CAR-encoded blocks.
    #[serde(with = "serde_bytes", default)]
    blocks: Option<Vec<u8>>,
    /// Operations in this commit.
    ops: Vec<RepoOp>,
    /// DEPRECATED: List of new blobs.
    #[allow(dead_code)]
    #[serde(default)]
    blobs: Vec<ipld_core::cid::Cid>,
    /// Timestamp of when the message was broadcast.
    #[allow(dead_code)]
    time: String,
    /// Previous data tree root CID (optional).
    #[allow(dead_code)]
    #[serde(rename = "prevData")]
    prev_data: Option<ipld_core::cid::Cid>,
}

#[derive(Debug, Deserialize)]
struct RepoOp {
    /// Action: "create", "update", or "delete".
    action: String,
    /// Path: "collection/rkey".
    path: String,
    /// CID of the record (for create/update).
    cid: Option<ipld_core::cid::Cid>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct InfoEvent {
    name: Option<String>,
    message: Option<String>,
}

/// Error payload from firehose (op=-1 frames).
#[derive(Debug, Deserialize)]
struct FirehoseError {
    /// Error type (e.g., "FutureCursor", "ConsumerTooSlow").
    error: Option<String>,
    /// Human-readable error message.
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    /// Helper to create a framed firehose message (header + payload).
    fn make_frame<T: Serialize>(op: i32, t: Option<&str>, payload: &T) -> Vec<u8> {
        #[derive(Serialize)]
        struct Header<'a> {
            op: i32,
            #[serde(skip_serializing_if = "Option::is_none")]
            t: Option<&'a str>,
        }

        let header = Header { op, t };
        let mut data = serde_ipld_dagcbor::to_vec(&header).unwrap();
        data.extend(serde_ipld_dagcbor::to_vec(payload).unwrap());
        data
    }

    #[test]
    fn test_decode_frame_header_commit() {
        #[derive(Serialize, Deserialize)]
        struct DummyPayload {
            seq: i64,
        }

        let frame = make_frame(1, Some("#commit"), &DummyPayload { seq: 12345 });
        let (header, offset) = decode_frame_header(&frame).unwrap();

        assert_eq!(header.op, 1);
        assert_eq!(header.t, Some("#commit".to_string()));
        assert!(offset > 0);
        assert!(offset < frame.len());

        // Verify payload can be decoded from offset
        let payload: DummyPayload = serde_ipld_dagcbor::from_slice(&frame[offset..]).unwrap();
        assert_eq!(payload.seq, 12345);
    }

    #[test]
    fn test_decode_frame_header_identity() {
        #[derive(Serialize)]
        struct IdentityPayload {
            did: String,
        }

        let frame = make_frame(
            1,
            Some("#identity"),
            &IdentityPayload {
                did: "did:plc:test123".to_string(),
            },
        );
        let (header, _) = decode_frame_header(&frame).unwrap();

        assert_eq!(header.op, 1);
        assert_eq!(header.t, Some("#identity".to_string()));
    }

    #[test]
    fn test_decode_frame_header_error() {
        #[derive(Serialize)]
        struct ErrorPayload {
            error: String,
            message: String,
        }

        let frame = make_frame(
            -1,
            None,
            &ErrorPayload {
                error: "FutureCursor".to_string(),
                message: "cursor is in the future".to_string(),
            },
        );
        let (header, _) = decode_frame_header(&frame).unwrap();

        assert_eq!(header.op, -1);
        assert_eq!(header.t, None);
    }

    #[test]
    fn test_decode_frame_header_info() {
        let frame = make_frame(
            1,
            Some("#info"),
            &InfoEvent {
                name: Some("OutdatedCursor".to_string()),
                message: Some("cursor is too old".to_string()),
            },
        );
        let (header, offset) = decode_frame_header(&frame).unwrap();

        assert_eq!(header.op, 1);
        assert_eq!(header.t, Some("#info".to_string()));

        let info: InfoEvent = serde_ipld_dagcbor::from_slice(&frame[offset..]).unwrap();
        assert_eq!(info.name, Some("OutdatedCursor".to_string()));
    }

    #[test]
    fn test_decode_frame_header_unknown_type() {
        #[derive(Serialize)]
        struct UnknownPayload {
            data: String,
        }

        let frame = make_frame(
            1,
            Some("#newEventType"),
            &UnknownPayload {
                data: "some data".to_string(),
            },
        );
        let (header, _) = decode_frame_header(&frame).unwrap();

        assert_eq!(header.op, 1);
        assert_eq!(header.t, Some("#newEventType".to_string()));
    }

    #[test]
    fn test_decode_frame_header_invalid_cbor() {
        let invalid_data = vec![0xFF, 0xFF, 0xFF];
        let result = decode_frame_header(&invalid_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_default_firehose_url() {
        assert_eq!(DEFAULT_FIREHOSE_URL, "wss://bsky.network");
    }

    #[test]
    fn test_parse_record_path_valid() {
        let result = parse_record_path("diy.razorgirl.winter.fact/3abc123");
        assert_eq!(result, Some(("diy.razorgirl.winter.fact", "3abc123")));
    }

    #[test]
    fn test_parse_record_path_empty_collection() {
        let result = parse_record_path("/3abc123");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_record_path_empty_rkey() {
        let result = parse_record_path("diy.razorgirl.winter.fact/");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_record_path_no_slash() {
        let result = parse_record_path("diy.razorgirl.winter.fact");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_record_path_empty_string() {
        let result = parse_record_path("");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_record_path_just_slash() {
        let result = parse_record_path("/");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_record_path_multiple_slashes() {
        // Only first two components matter
        let result = parse_record_path("collection/rkey/extra/parts");
        assert_eq!(result, Some(("collection", "rkey")));
    }
}
