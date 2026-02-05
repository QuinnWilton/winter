//! Real-time thought subscription via firehose.

use std::collections::HashMap;
use std::io::Cursor;
use std::time::Duration;

use futures_util::StreamExt;
use iroh_car::CarReader;
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};

use winter_atproto::{THOUGHT_COLLECTION, Thought, ThoughtKind};

/// Subscribe to thoughts via firehose and push to SSE channel.
pub async fn subscribe_thoughts(
    firehose_url: String,
    did: String,
    thought_tx: broadcast::Sender<String>,
) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        match connect_and_stream(&firehose_url, &did, &thought_tx, &mut backoff).await {
            Ok(()) => {
                // Clean shutdown (shouldn't happen normally)
                info!("thought subscription ended cleanly");
                return;
            }
            Err(e) => {
                error!(error = %e, backoff_secs = backoff.as_secs(), "firehose connection error, reconnecting");
                tokio::time::sleep(backoff).await;
                backoff = std::cmp::min(backoff * 2, max_backoff);
            }
        }
    }
}

async fn connect_and_stream(
    firehose_url: &str,
    did: &str,
    thought_tx: &broadcast::Sender<String>,
    backoff: &mut Duration,
) -> Result<(), String> {
    let url = format!("{}/xrpc/com.atproto.sync.subscribeRepos", firehose_url);
    info!(url = %url, did = %did, "connecting to firehose for thought stream");

    let (ws_stream, _) = connect_async(&url)
        .await
        .map_err(|e| format!("connection failed: {}", e))?;

    let (_, mut read) = ws_stream.split();
    info!("thought stream connected to firehose");

    // Reset backoff on successful connect
    *backoff = Duration::from_secs(1);

    loop {
        match read.next().await {
            Some(Ok(Message::Binary(data))) => {
                if let Err(e) = handle_message(&data, did, thought_tx).await {
                    trace!(error = %e, "failed to handle firehose message");
                }
            }
            Some(Ok(Message::Close(_))) => {
                return Err("connection closed by server".to_string());
            }
            Some(Ok(_)) => {
                // Ignore other message types
            }
            Some(Err(e)) => {
                return Err(format!("read error: {}", e));
            }
            None => {
                return Err("stream ended".to_string());
            }
        }
    }
}

async fn handle_message(
    data: &[u8],
    did: &str,
    thought_tx: &broadcast::Sender<String>,
) -> Result<(), String> {
    // Decode frame header
    let (header, payload_offset) = decode_frame_header(data)?;

    if header.op != 1 {
        return Ok(()); // Not a regular message
    }

    let payload = &data[payload_offset..];

    match header.t.as_deref() {
        Some("#commit") => {
            let commit: CommitEvent =
                serde_ipld_dagcbor::from_slice(payload).map_err(|e| e.to_string())?;

            // Only process commits for our DID
            if commit.repo != did {
                return Ok(());
            }

            // Check if any ops are for thought collection
            let has_thoughts = commit
                .ops
                .iter()
                .any(|op| op.path.starts_with(THOUGHT_COLLECTION));

            if !has_thoughts {
                return Ok(());
            }

            debug!(rev = %commit.rev, "received commit with thoughts");

            // Parse CAR blocks
            let blocks = if let Some(ref blocks_data) = commit.blocks {
                parse_commit_blocks(blocks_data).await?
            } else {
                HashMap::new()
            };

            // Process thought ops
            for op in &commit.ops {
                if !op.path.starts_with(THOUGHT_COLLECTION) {
                    continue;
                }

                if op.action != "create" && op.action != "update" {
                    continue;
                }

                if let Some(ref cid) = op.cid {
                    let cid_str = cid.to_string();
                    if let Some(record_data) = blocks.get(&cid_str) {
                        match serde_ipld_dagcbor::from_slice::<Thought>(record_data) {
                            Ok(thought) => {
                                let thought_json = serde_json::json!({
                                    "kind": thought_kind_to_string(&thought.kind),
                                    "content": thought.content,
                                    "created_at": thought.created_at.to_rfc3339(),
                                    "trigger": thought.trigger,
                                    "duration_ms": thought.duration_ms,
                                    "tags": thought.tags,
                                });

                                if let Err(e) = thought_tx.send(thought_json.to_string()) {
                                    debug!(error = %e, "no SSE subscribers");
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "failed to decode thought from firehose");
                            }
                        }
                    }
                }
            }
        }
        _ => {
            // Ignore non-commit events
        }
    }

    Ok(())
}

async fn parse_commit_blocks(data: &[u8]) -> Result<HashMap<String, Vec<u8>>, String> {
    let cursor = Cursor::new(data);
    let mut reader = CarReader::new(cursor)
        .await
        .map_err(|e| format!("failed to read CAR: {}", e))?;

    let mut blocks = HashMap::new();

    loop {
        match reader.next_block().await {
            Ok(Some((cid, data))) => {
                blocks.insert(cid.to_string(), data);
            }
            Ok(None) => break,
            Err(e) => {
                return Err(format!("failed to read block: {}", e));
            }
        }
    }

    Ok(blocks)
}

#[derive(Debug, Deserialize)]
struct FrameHeader {
    op: i32,
    t: Option<String>,
}

fn decode_frame_header(data: &[u8]) -> Result<(FrameHeader, usize), String> {
    let mut cursor = Cursor::new(data);
    let header: FrameHeader = ciborium::from_reader(&mut cursor)
        .map_err(|e| format!("failed to decode header: {}", e))?;
    let offset = cursor.position() as usize;
    Ok((header, offset))
}

#[derive(Debug, Deserialize)]
struct CommitEvent {
    #[allow(dead_code)]
    seq: i64,
    repo: String,
    rev: String,
    #[serde(with = "serde_bytes", default)]
    blocks: Option<Vec<u8>>,
    ops: Vec<RepoOp>,
}

#[derive(Debug, Deserialize)]
struct RepoOp {
    action: String,
    path: String,
    cid: Option<ipld_core::cid::Cid>,
}

/// Convert ThoughtKind to snake_case string for CSS classes.
fn thought_kind_to_string(kind: &ThoughtKind) -> &'static str {
    match kind {
        ThoughtKind::Insight => "insight",
        ThoughtKind::Question => "question",
        ThoughtKind::Plan => "plan",
        ThoughtKind::Reflection => "reflection",
        ThoughtKind::Error => "error",
        ThoughtKind::Response => "response",
        ThoughtKind::ToolCall => "tool_call",
    }
}
