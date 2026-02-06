//! Jetstream WebSocket client for real-time ATProto repository updates.
//!
//! Connects to a Jetstream instance (JSON WebSocket) instead of the binary
//! CBOR/CAR firehose. Events arrive as JSON with records already deserialized.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};

use crate::cache::{RepoCache, SyncState};
use crate::dispatch::{dispatch_create_or_update_json, dispatch_delete, is_tracked_collection};
use crate::{AtprotoError, IDENTITY_COLLECTION, IDENTITY_KEY, Identity};

/// Default Jetstream endpoint.
pub const DEFAULT_JETSTREAM_URL: &str = "wss://jetstream2.us-west.bsky.network/subscribe";

/// All collections we want Jetstream to send us events for.
const WANTED_COLLECTIONS: &[&str] = &[
    "diy.razorgirl.winter.identity",
    "diy.razorgirl.winter.state",
    "diy.razorgirl.winter.fact",
    "diy.razorgirl.winter.rule",
    "diy.razorgirl.winter.thought",
    "diy.razorgirl.winter.note",
    "diy.razorgirl.winter.job",
    "diy.razorgirl.winter.directive",
    "diy.razorgirl.winter.factDeclaration",
    "diy.razorgirl.winter.tool",
    "diy.razorgirl.winter.toolApproval",
    "diy.razorgirl.winter.wikiEntry",
    "diy.razorgirl.winter.wikiLink",
    "diy.razorgirl.winter.trigger",
    "app.bsky.feed.post",
    "app.bsky.feed.like",
    "app.bsky.feed.repost",
    "app.bsky.graph.follow",
    "com.whtwnd.blog.entry",
];

/// Callback for operator events (e.g., tool approvals).
pub type OperatorEventCallback =
    Arc<dyn Fn(OperatorEvent) + Send + Sync>;

/// Events from the operator's DID that the daemon cares about.
#[derive(Debug, Clone)]
pub enum OperatorEvent {
    /// Operator created/updated a tool approval record.
    ToolApproval {
        rkey: String,
        approval: crate::ToolApproval,
    },
}

/// Jetstream WebSocket client.
pub struct JetstreamClient {
    /// Cache to update with events.
    cache: Arc<RepoCache>,
    /// Jetstream WebSocket URL base (without query params).
    url: String,
    /// DIDs to watch: [winter_did, operator_did].
    wanted_dids: Vec<String>,
    /// Optional callback for operator events.
    operator_callback: Option<OperatorEventCallback>,
    /// The operator DID (if watching).
    operator_did: Option<String>,
}

impl JetstreamClient {
    /// Create a new Jetstream client watching a single DID.
    pub fn new(url: impl Into<String>, did: impl Into<String>, cache: Arc<RepoCache>) -> Self {
        let did = did.into();
        Self {
            cache,
            url: url.into(),
            wanted_dids: vec![did],
            operator_callback: None,
            operator_did: None,
        }
    }

    /// Add the operator DID to the watch list.
    pub fn with_operator_did(mut self, operator_did: impl Into<String>) -> Self {
        let did = operator_did.into();
        if !did.is_empty() && !self.wanted_dids.contains(&did) {
            self.wanted_dids.push(did.clone());
        }
        self.operator_did = Some(did);
        self
    }

    /// Set a callback for operator events (tool approvals, etc.).
    pub fn with_operator_callback(mut self, callback: OperatorEventCallback) -> Self {
        self.operator_callback = Some(callback);
        self
    }

    /// Build the full WebSocket URL with query parameters.
    fn build_url(&self, cursor: Option<i64>) -> String {
        let mut url = self.url.clone();

        // Add query params
        let mut first = !url.contains('?');

        // wantedDids
        for did in &self.wanted_dids {
            url.push(if first { '?' } else { '&' });
            first = false;
            url.push_str("wantedDids=");
            url.push_str(did);
        }

        // wantedCollections
        for col in WANTED_COLLECTIONS {
            url.push('&');
            url.push_str("wantedCollections=");
            url.push_str(col);
        }

        // Cursor (time_us)
        if let Some(cursor) = cursor {
            url.push('&');
            url.push_str("cursor=");
            url.push_str(&cursor.to_string());
        }

        url
    }

    /// Connect and start receiving events.
    ///
    /// Runs in a reconnection loop with exponential backoff.
    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<(), AtprotoError> {
        let mut backoff_secs = 1u64;
        let mut last_time_us: Option<i64> = None;

        loop {
            if *shutdown_rx.borrow() {
                info!("jetstream client shutting down");
                return Ok(());
            }

            // On reconnect, subtract 5 seconds from cursor for gapless playback
            let cursor = last_time_us.map(|t| t - 5_000_000);
            let url = self.build_url(cursor);

            info!(url = %url, dids = ?self.wanted_dids, "connecting to jetstream");

            match self
                .connect_and_process(&url, &mut shutdown_rx, &mut last_time_us)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    error!(error = %e, "jetstream connection error, reconnecting");

                    if self.cache.state() == SyncState::Live {
                        self.cache.set_state(SyncState::Syncing);
                        warn!("jetstream disconnected, cache set to Syncing");
                    }

                    // Wait with backoff
                    let wait = Duration::from_secs(backoff_secs);
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                return Ok(());
                            }
                        }
                        _ = tokio::time::sleep(wait) => {}
                    }

                    backoff_secs = (backoff_secs * 2).min(60);
                }
            }
        }
    }

    /// Connect and process messages until error or shutdown.
    async fn connect_and_process(
        &self,
        url: &str,
        shutdown_rx: &mut watch::Receiver<bool>,
        last_time_us: &mut Option<i64>,
    ) -> Result<(), AtprotoError> {
        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| AtprotoError::WebSocket(format!("connection failed: {}", e)))?;

        let (_, mut read) = ws_stream.split();

        info!("jetstream connected");

        // Reset backoff on successful connection (caller handles backoff state)
        // If we were Syncing, suppress broadcasts during catchup
        let was_syncing = self.cache.state() == SyncState::Syncing;
        if was_syncing {
            debug!("suppressing broadcasts during jetstream reconnection catchup");
            self.cache.set_suppress_broadcasts(true);

            // Schedule re-enabling broadcasts after 3 seconds
            let cache = Arc::clone(&self.cache);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if cache.broadcasts_suppressed() {
                    debug!("reconnection catchup complete, re-enabling broadcasts");
                    cache.set_suppress_broadcasts(false);
                    cache.set_state(SyncState::Live);
                }
            });
        }

        const READ_TIMEOUT: Duration = Duration::from_secs(300);

        loop {
            tokio::select! {
                biased;

                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("jetstream received shutdown signal");
                        // Re-enable broadcasts if suppressed
                        if self.cache.broadcasts_suppressed() {
                            self.cache.set_suppress_broadcasts(false);
                        }
                        return Ok(());
                    }
                }

                result = tokio::time::timeout(READ_TIMEOUT, read.next()) => {
                    match result {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            if let Err(e) = self.handle_message(&text, last_time_us) {
                                warn!(error = %e, "failed to handle jetstream message");
                            }
                        }
                        Ok(Some(Ok(Message::Ping(_)))) => {
                            // tungstenite auto-responds to pings
                            trace!("received ping");
                        }
                        Ok(Some(Ok(Message::Close(_)))) => {
                            info!("jetstream connection closed by server");
                            if self.cache.broadcasts_suppressed() {
                                self.cache.set_suppress_broadcasts(false);
                            }
                            return Err(AtprotoError::WebSocket("connection closed".to_string()));
                        }
                        Ok(Some(Ok(_))) => {}
                        Ok(Some(Err(e))) => {
                            if self.cache.broadcasts_suppressed() {
                                self.cache.set_suppress_broadcasts(false);
                            }
                            return Err(AtprotoError::WebSocket(format!("read error: {}", e)));
                        }
                        Ok(None) => {
                            if self.cache.broadcasts_suppressed() {
                                self.cache.set_suppress_broadcasts(false);
                            }
                            return Err(AtprotoError::WebSocket("stream ended".to_string()));
                        }
                        Err(_) => {
                            warn!("jetstream read timeout after {}s", READ_TIMEOUT.as_secs());
                            if self.cache.broadcasts_suppressed() {
                                self.cache.set_suppress_broadcasts(false);
                            }
                            return Err(AtprotoError::WebSocket("read timeout".to_string()));
                        }
                    }
                }
            }
        }
    }

    /// Handle a single Jetstream JSON message.
    fn handle_message(
        &self,
        text: &str,
        last_time_us: &mut Option<i64>,
    ) -> Result<(), AtprotoError> {
        let event: JetstreamEvent = serde_json::from_str(text).map_err(|e| {
            AtprotoError::Json(e)
        })?;

        // Update cursor
        *last_time_us = Some(event.time_us);

        match event.kind.as_str() {
            "commit" => {
                if let Some(commit) = event.commit {
                    self.handle_commit(&event.did, commit)?;
                }
            }
            "identity" | "account" => {
                trace!(kind = %event.kind, did = %event.did, "ignoring non-commit event");
            }
            _ => {
                trace!(kind = %event.kind, "ignoring unknown jetstream event");
            }
        }

        Ok(())
    }

    /// Handle a commit event from Jetstream.
    fn handle_commit(
        &self,
        did: &str,
        commit: JetstreamCommit,
    ) -> Result<(), AtprotoError> {
        let is_own = self.wanted_dids.first().map(|d| d.as_str()) == Some(did);
        let is_operator = self.operator_did.as_deref() == Some(did);

        if !is_own && !is_operator {
            return Ok(());
        }

        let collection = &commit.collection;
        let rkey = &commit.rkey;

        // Handle operator events (only tool approvals on operator's PDS)
        if is_operator && !is_own {
            if collection == crate::TOOL_APPROVAL_COLLECTION {
                if let Some(ref record) = commit.record {
                    if commit.operation == "create" || commit.operation == "update" {
                        if let Ok(approval) = serde_json::from_value::<crate::ToolApproval>(record.clone()) {
                            if let Some(ref callback) = self.operator_callback {
                                callback(OperatorEvent::ToolApproval {
                                    rkey: rkey.clone(),
                                    approval,
                                });
                            }
                        }
                    }
                }
            }
            return Ok(());
        }

        // Own DID events — update cache
        if !is_tracked_collection(collection) {
            return Ok(());
        }

        match commit.operation.as_str() {
            "create" | "update" => {
                if let Some(record) = commit.record {
                    let cid = commit.cid.as_deref().unwrap_or("unknown");

                    // Handle special collections (identity)
                    let handled =
                        dispatch_create_or_update_json(&self.cache, collection, rkey, cid, record.clone())?;

                    if !handled && collection == IDENTITY_COLLECTION && rkey == IDENTITY_KEY {
                        if let Ok(identity) = serde_json::from_value::<Identity>(record) {
                            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                                rt.block_on(self.cache.set_identity(identity, cid.to_string()));
                            }
                        }
                    }
                }
            }
            "delete" => {
                dispatch_delete(&self.cache, collection, rkey);
            }
            _ => {
                trace!(op = %commit.operation, "unknown jetstream operation");
            }
        }

        Ok(())
    }
}

// =============================================================================
// Jetstream JSON types
// =============================================================================

#[derive(Debug, Deserialize)]
struct JetstreamEvent {
    /// DID of the account.
    did: String,
    /// Microsecond timestamp.
    time_us: i64,
    /// Event type: "commit", "identity", "account".
    kind: String,
    /// Commit details (only for "commit" events).
    commit: Option<JetstreamCommit>,
}

#[derive(Debug, Deserialize)]
struct JetstreamCommit {
    /// Repository revision.
    #[allow(dead_code)]
    rev: String,
    /// Operation: "create", "update", "delete".
    operation: String,
    /// Collection NSID (e.g., "app.bsky.feed.post").
    collection: String,
    /// Record key.
    rkey: String,
    /// The record value (already deserialized JSON). Absent for deletes.
    record: Option<serde_json::Value>,
    /// Content ID.
    cid: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_jetstream_url() {
        assert_eq!(
            DEFAULT_JETSTREAM_URL,
            "wss://jetstream2.us-west.bsky.network/subscribe"
        );
    }

    #[test]
    fn test_build_url_no_cursor() {
        let cache = RepoCache::new();
        let client = JetstreamClient::new(DEFAULT_JETSTREAM_URL, "did:plc:test", cache);
        let url = client.build_url(None);
        assert!(url.contains("wantedDids=did:plc:test"));
        assert!(url.contains("wantedCollections=diy.razorgirl.winter.fact"));
        assert!(!url.contains("cursor="));
    }

    #[test]
    fn test_build_url_with_cursor() {
        let cache = RepoCache::new();
        let client = JetstreamClient::new(DEFAULT_JETSTREAM_URL, "did:plc:test", cache);
        let url = client.build_url(Some(1234567890));
        assert!(url.contains("cursor=1234567890"));
    }

    #[test]
    fn test_build_url_with_operator() {
        let cache = RepoCache::new();
        let client = JetstreamClient::new(DEFAULT_JETSTREAM_URL, "did:plc:winter", cache)
            .with_operator_did("did:plc:operator");
        let url = client.build_url(None);
        assert!(url.contains("wantedDids=did:plc:winter"));
        assert!(url.contains("wantedDids=did:plc:operator"));
    }

    #[test]
    fn test_build_url_operator_same_as_self() {
        let cache = RepoCache::new();
        let client = JetstreamClient::new(DEFAULT_JETSTREAM_URL, "did:plc:same", cache)
            .with_operator_did("did:plc:same");
        let url = client.build_url(None);
        // Should only appear once
        let count = url.matches("wantedDids=did:plc:same").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_parse_jetstream_commit_event() {
        let json = r#"{
            "did": "did:plc:test123",
            "time_us": 1706000000000000,
            "kind": "commit",
            "commit": {
                "rev": "3abc123",
                "operation": "create",
                "collection": "diy.razorgirl.winter.fact",
                "rkey": "3xyz789",
                "record": {"predicate": "likes", "args": ["coffee"]},
                "cid": "bafytest"
            }
        }"#;

        let event: JetstreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.did, "did:plc:test123");
        assert_eq!(event.kind, "commit");
        let commit = event.commit.unwrap();
        assert_eq!(commit.operation, "create");
        assert_eq!(commit.collection, "diy.razorgirl.winter.fact");
        assert_eq!(commit.rkey, "3xyz789");
        assert!(commit.record.is_some());
    }

    #[test]
    fn test_parse_jetstream_delete_event() {
        let json = r#"{
            "did": "did:plc:test123",
            "time_us": 1706000000000000,
            "kind": "commit",
            "commit": {
                "rev": "3abc123",
                "operation": "delete",
                "collection": "diy.razorgirl.winter.fact",
                "rkey": "3xyz789"
            }
        }"#;

        let event: JetstreamEvent = serde_json::from_str(json).unwrap();
        let commit = event.commit.unwrap();
        assert_eq!(commit.operation, "delete");
        assert!(commit.record.is_none());
        assert!(commit.cid.is_none());
    }

    #[test]
    fn test_parse_jetstream_identity_event() {
        let json = r#"{
            "did": "did:plc:test123",
            "time_us": 1706000000000000,
            "kind": "identity"
        }"#;

        let event: JetstreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.kind, "identity");
        assert!(event.commit.is_none());
    }
}
