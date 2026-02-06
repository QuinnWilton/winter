//! Firehose client for real-time ATProto repository updates.
//!
//! Connects to com.atproto.sync.subscribeRepos and receives commit events.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use backoff::ExponentialBackoff;
use backoff::backoff::Backoff;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use iroh_car::CarReader;
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};

use crate::cache::{FirehoseCommit, FirehoseOp, RepoCache, SyncState};
use crate::dispatch::{dispatch_create_or_update, dispatch_delete, is_tracked_collection};
use crate::{AtprotoError, IDENTITY_COLLECTION, IDENTITY_KEY, Identity};

/// Default firehose URL (Bluesky relay).
/// Prefer [`resolve_firehose_url`] to subscribe directly to the account's PDS,
/// which only emits commits for accounts hosted there (far less traffic).
pub const DEFAULT_FIREHOSE_URL: &str = "wss://bsky.network";

/// Derive a firehose WebSocket URL from a PDS HTTP URL.
///
/// Converts `https://pds.example.com` → `wss://pds.example.com`.
/// A PDS firehose only emits commits for accounts hosted on that PDS,
/// so this avoids the global relay's full-network traffic.
pub fn firehose_url_for_pds(pds_url: &str) -> String {
    pds_url
        .replace("https://", "wss://")
        .replace("http://", "ws://")
}

/// Resolve the firehose URL for a DID by looking up its DID document.
///
/// The DID document contains the actual PDS service endpoint (e.g.,
/// `https://puffball.us-east.host.bsky.network`), which may differ from the
/// login URL (`https://bsky.social`). The PDS firehose only emits commits
/// for accounts hosted on that PDS, avoiding the full-network relay.
///
/// Falls back to converting `fallback_pds_url` if DID resolution fails.
pub async fn resolve_firehose_url(did: &str, fallback_pds_url: &str) -> String {
    match resolve_pds_for_did(did).await {
        Some(pds_url) => {
            let url = firehose_url_for_pds(&pds_url);
            tracing::info!(did = %did, pds = %pds_url, firehose = %url, "resolved PDS firehose from DID document");
            url
        }
        None => {
            let url = firehose_url_for_pds(fallback_pds_url);
            tracing::warn!(
                did = %did,
                fallback = %url,
                "failed to resolve PDS from DID document, falling back to login URL"
            );
            url
        }
    }
}

/// Resolve the PDS service endpoint from a DID document.
///
/// Supports `did:plc:` (via plc.directory) and `did:web:` (via .well-known).
async fn resolve_pds_for_did(did: &str) -> Option<String> {
    let doc_url = if did.starts_with("did:plc:") {
        format!("https://plc.directory/{}", did)
    } else if did.starts_with("did:web:") {
        let domain = did.strip_prefix("did:web:")?;
        format!("https://{}/.well-known/did.json", domain)
    } else {
        return None;
    };

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let response = http.get(&doc_url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    let doc: serde_json::Value = response.json().await.ok()?;
    let services = doc.get("service")?.as_array()?;
    for service in services {
        let service_type = service.get("type")?.as_str()?;
        if service_type == "AtprotoPersonalDataServer" {
            return service
                .get("serviceEndpoint")
                .and_then(|v| v.as_str())
                .map(|s| s.trim_end_matches('/').to_string());
        }
    }

    None
}

/// Channel buffer size for messages between reader and processor tasks.
/// Large enough to absorb processing bursts without blocking the reader.
const PROCESSOR_CHANNEL_SIZE: usize = 1000;

/// Duration to wait after reconnection before re-enabling broadcasts.
/// During this window, firehose events are applied to the cache but broadcasts
/// are suppressed to prevent channel lag that triggers expensive TSV regeneration.
const RECONNECTION_CATCHUP_DURATION: Duration = Duration::from_secs(3);

/// Message sent from reader to processor task.
enum ProcessorMessage {
    /// Binary WebSocket message to process.
    Binary(Vec<u8>),
    /// Reader encountered an error.
    ReaderError(AtprotoError),
    /// Reader is shutting down (clean exit).
    Shutdown,
}

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
        let mut backoff = ExponentialBackoff {
            initial_interval: Duration::from_secs(1),
            max_interval: Duration::from_secs(60),
            max_elapsed_time: None, // Retry forever
            ..Default::default()
        };

        loop {
            if *shutdown_rx.borrow() {
                info!("firehose client shutting down");
                return Ok(());
            }

            match self
                .connect_and_process(&mut shutdown_rx, &mut backoff)
                .await
            {
                Ok(()) => {
                    // Clean shutdown
                    return Ok(());
                }
                Err(e) => {
                    error!(error = %e, "firehose connection error, reconnecting");

                    // Set state to Syncing so tools fall back to HTTP during reconnection
                    // This prevents serving stale data during the gap
                    if self.cache.state() == SyncState::Live {
                        self.cache.set_state(SyncState::Syncing);
                        warn!("firehose disconnected, cache set to Syncing until reconnection");
                    }

                    // Get next backoff duration (always Some since max_elapsed_time is None)
                    let wait_duration = backoff.next_backoff().unwrap_or(Duration::from_secs(60));

                    // Wait with backoff
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                return Ok(());
                            }
                        }
                        _ = tokio::time::sleep(wait_duration) => {}
                    }
                }
            }
        }
    }

    /// Reader task: reads WebSocket messages and immediately responds to pings.
    ///
    /// This task is lightweight and never blocks on message processing. Binary messages
    /// are forwarded to the processor task via a channel. If the channel is full,
    /// messages are dropped (cursor-based reconnection will replay them).
    async fn reader_task(
        mut read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        mut write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
        msg_tx: mpsc::Sender<ProcessorMessage>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), AtprotoError> {
        const READ_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

        loop {
            tokio::select! {
                biased;

                // Check for shutdown first (highest priority)
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("firehose reader received shutdown signal");
                        // Best-effort send; processor may already be gone
                        let _ = msg_tx.send(ProcessorMessage::Shutdown).await;
                        return Ok(());
                    }
                }

                // Read from WebSocket
                result = timeout(READ_TIMEOUT, read.next()) => {
                    match result {
                        Ok(Some(Ok(Message::Ping(data)))) => {
                            // Respond to pings immediately - this is the critical path
                            trace!("received ping, sending pong");
                            if let Err(e) = write.send(Message::Pong(data)).await {
                                warn!(error = %e, "failed to send pong");
                                let err = AtprotoError::WebSocket(format!("pong failed: {}", e));
                                let _ = msg_tx.send(ProcessorMessage::ReaderError(
                                    AtprotoError::WebSocket(format!("pong failed: {}", e))
                                )).await;
                                return Err(err);
                            }
                        }
                        Ok(Some(Ok(Message::Binary(data)))) => {
                            // Forward binary messages to processor (non-blocking)
                            match msg_tx.try_send(ProcessorMessage::Binary(data)) {
                                Ok(()) => {}
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    // Channel full - drop message, cursor will replay on reconnect
                                    warn!("processor channel full, dropping firehose message");
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    // Processor has shut down
                                    debug!("processor channel closed, reader exiting");
                                    return Ok(());
                                }
                            }
                        }
                        Ok(Some(Ok(Message::Close(_)))) => {
                            info!("firehose connection closed by server");
                            let err = AtprotoError::WebSocket("connection closed".to_string());
                            let _ = msg_tx.send(ProcessorMessage::ReaderError(
                                AtprotoError::WebSocket("connection closed".to_string())
                            )).await;
                            return Err(err);
                        }
                        Ok(Some(Ok(_))) => {
                            // Ignore other message types (text, pong)
                        }
                        Ok(Some(Err(e))) => {
                            let err = AtprotoError::WebSocket(format!("read error: {}", e));
                            let _ = msg_tx.send(ProcessorMessage::ReaderError(
                                AtprotoError::WebSocket(format!("read error: {}", e))
                            )).await;
                            return Err(err);
                        }
                        Ok(None) => {
                            let err = AtprotoError::WebSocket("stream ended".to_string());
                            let _ = msg_tx.send(ProcessorMessage::ReaderError(
                                AtprotoError::WebSocket("stream ended".to_string())
                            )).await;
                            return Err(err);
                        }
                        Err(_) => {
                            warn!("firehose read timeout after {}s - connection may be stale", READ_TIMEOUT.as_secs());
                            let err = AtprotoError::WebSocket("read timeout".to_string());
                            let _ = msg_tx.send(ProcessorMessage::ReaderError(
                                AtprotoError::WebSocket("read timeout".to_string())
                            )).await;
                            return Err(err);
                        }
                    }
                }
            }
        }
    }

    /// Processor task: receives messages from the reader and handles them.
    ///
    /// This task can take time to process messages (CAR parsing, cache updates)
    /// without affecting ping/pong latency.
    ///
    /// If `catchup_until` is provided, broadcasts are suppressed until that instant,
    /// then re-enabled with a `Synchronized` event to trigger full TSV regeneration.
    async fn processor_task(
        &self,
        mut msg_rx: mpsc::Receiver<ProcessorMessage>,
        catchup_until: Option<Instant>,
    ) -> Result<(), AtprotoError> {
        let mut catchup_complete = catchup_until.is_none();

        loop {
            // Check if catchup period has elapsed
            if !catchup_complete
                && let Some(until) = catchup_until
                && Instant::now() >= until
            {
                debug!("reconnection catchup complete, re-enabling broadcasts");
                self.cache.set_suppress_broadcasts(false);
                // Send Synchronized to trigger populate_from_repo_cache
                self.cache.set_state(SyncState::Live);
                catchup_complete = true;
            }

            match msg_rx.recv().await {
                Some(ProcessorMessage::Binary(data)) => {
                    if let Err(e) = self.handle_message(&data).await {
                        warn!(error = %e, "failed to handle firehose message");
                    }
                }
                Some(ProcessorMessage::ReaderError(e)) => {
                    // Reader encountered an error, propagate it
                    // Make sure broadcasts are re-enabled before returning
                    if !catchup_complete {
                        self.cache.set_suppress_broadcasts(false);
                    }
                    return Err(e);
                }
                Some(ProcessorMessage::Shutdown) => {
                    // Clean shutdown requested
                    // Make sure broadcasts are re-enabled before returning
                    if !catchup_complete {
                        self.cache.set_suppress_broadcasts(false);
                    }
                    info!("firehose processor received shutdown");
                    return Ok(());
                }
                None => {
                    // Channel closed, reader must have exited
                    // Make sure broadcasts are re-enabled before returning
                    if !catchup_complete {
                        self.cache.set_suppress_broadcasts(false);
                    }
                    debug!("processor channel closed, exiting");
                    return Ok(());
                }
            }
        }
    }

    /// Connect to the firehose and process messages.
    async fn connect_and_process(
        &self,
        shutdown_rx: &mut watch::Receiver<bool>,
        backoff: &mut ExponentialBackoff,
    ) -> Result<(), AtprotoError> {
        // Use cursor-based reconnection if we have a known sequence number
        let cursor = self.cache.firehose_seq();
        let url = if cursor > 0 {
            format!(
                "{}/xrpc/com.atproto.sync.subscribeRepos?cursor={}",
                self.url, cursor
            )
        } else {
            format!("{}/xrpc/com.atproto.sync.subscribeRepos", self.url)
        };

        info!(url = %url, did = %self.did, cursor = cursor, "connecting to firehose");

        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| AtprotoError::WebSocket(format!("connection failed: {}", e)))?;

        let (write, read) = ws_stream.split();

        info!("firehose connected");

        // Reset backoff on successful connection
        backoff.reset();

        // Determine if we need to suppress broadcasts during reconnection catchup.
        // If we were in Syncing state due to a disconnection and have a cursor,
        // the firehose will replay missed events. Suppress broadcasts during this
        // replay to prevent channel lag that triggers expensive TSV regeneration.
        let was_syncing = self.cache.state() == SyncState::Syncing;
        let catchup_until = if was_syncing && cursor > 0 {
            debug!("suppressing broadcasts during reconnection replay");
            self.cache.set_suppress_broadcasts(true);
            // Don't set state to Live yet - processor will do it after catchup
            info!(
                "firehose reconnected with cursor (seq={}), entering catchup mode",
                cursor
            );
            Some(Instant::now() + RECONNECTION_CATCHUP_DURATION)
        } else if was_syncing {
            // No cursor means we may have missed events, go Live immediately
            warn!("firehose reconnected without cursor, cache may have missed events");
            self.cache.set_state(SyncState::Live);
            None
        } else {
            // Not syncing (e.g., initial connection during SyncCoordinator startup)
            // Don't change state, let SyncCoordinator manage it
            None
        };

        // Create channel for reader -> processor communication
        let (msg_tx, msg_rx) = mpsc::channel(PROCESSOR_CHANNEL_SIZE);

        // Spawn reader task (handles pings immediately, forwards messages to processor)
        let reader_shutdown_rx = shutdown_rx.clone();
        let reader_handle = tokio::spawn(async move {
            Self::reader_task(read, write, msg_tx, reader_shutdown_rx).await
        });

        // Run processor in current task (handles message processing)
        let processor_result = self.processor_task(msg_rx, catchup_until).await;

        // Wait for reader to finish (it will exit when channel closes or on error)
        let reader_result = reader_handle.await;

        // Determine final result: prefer processor error, then reader error
        match processor_result {
            Ok(()) => {
                // Processor exited cleanly, check reader
                match reader_result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(AtprotoError::WebSocket(format!(
                        "reader task panicked: {}",
                        e
                    ))),
                }
            }
            Err(e) => Err(e),
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

                        // Handle cursor-related errors that require a full re-sync
                        if let Some(ref error_type) = err.error
                            && (error_type == "FutureCursor" || error_type == "ConsumerTooSlow")
                        {
                            warn!(
                                error_type = %error_type,
                                "cursor is invalid/stale, will trigger full re-sync"
                            );
                            // Reset cursor so next connection starts fresh
                            self.cache.reset_firehose_seq();
                            // Return error to trigger reconnection
                            return Err(AtprotoError::WebSocket(format!(
                                "cursor invalid: {}",
                                error_type
                            )));
                        }
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

            // Only care about Winter collections and Bluesky collections we track
            if !is_tracked_collection(collection) {
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
            seq: commit.seq,
            rev: commit.rev.clone(),
            ops,
        };

        // Update the sequence number tracking
        self.cache.update_firehose_seq(commit.seq);

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

// is_tracked_collection is now generated by the dispatch macro

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
                apply_create_or_update(cache, collection, rkey, cid, record)?;
            }
            FirehoseOp::Delete { collection, rkey } => {
                apply_delete(cache, collection, rkey);
            }
        }
    }

    Ok(())
}

/// Apply a create or update operation to the cache.
fn apply_create_or_update(
    cache: &RepoCache,
    collection: &str,
    rkey: &str,
    cid: &str,
    record: &[u8],
) -> Result<(), AtprotoError> {
    // Use the dispatch macro for most record types
    let handled = dispatch_create_or_update(cache, collection, rkey, cid, record)?;

    // Handle special cases (identity) that need async or singleton logic
    if !handled && collection == IDENTITY_COLLECTION && rkey == IDENTITY_KEY {
        let identity: Identity = serde_ipld_dagcbor::from_slice(record)
            .map_err(|e| AtprotoError::CborDecode(format!("failed to decode identity: {}", e)))?;
        // Use a blocking approach since this is sync code
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.block_on(cache.set_identity(identity, cid.to_string()));
        }
    }

    Ok(())
}

/// Apply a delete operation to the cache.
fn apply_delete(cache: &RepoCache, collection: &str, rkey: &str) {
    // Use the dispatch macro for all record types
    // Special collections (identity, state) are handled as no-ops in dispatch
    dispatch_delete(cache, collection, rkey);
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
    fn test_firehose_url_for_pds() {
        assert_eq!(
            firehose_url_for_pds("https://pds.example.com"),
            "wss://pds.example.com"
        );
        assert_eq!(
            firehose_url_for_pds("https://bsky.social"),
            "wss://bsky.social"
        );
        assert_eq!(
            firehose_url_for_pds("http://localhost:2583"),
            "ws://localhost:2583"
        );
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
