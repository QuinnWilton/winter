//! Firehose consumer for indexing wiki records from all users.

use std::io::Cursor;
use std::sync::Arc;

use futures_util::StreamExt;
use iroh_car::CarReader;
use tokio_tungstenite::connect_async;
use tracing::{debug, info, trace, warn};

use winter_atproto::{WIKI_ENTRY_COLLECTION, WIKI_LINK_COLLECTION, WikiEntry, WikiLink};

use crate::db::WikiDb;

/// Firehose consumer that indexes wiki records into SQLite.
pub struct FirehoseConsumer {
    relay_url: String,
    db: Arc<WikiDb>,
}

impl FirehoseConsumer {
    pub fn new(relay_url: String, db: Arc<WikiDb>) -> Self {
        Self { relay_url, db }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            // Rebuild URL on each reconnect to use the latest cursor from DB
            let cursor = self.db.get_cursor().ok().flatten();
            let url = if let Some(cursor) = cursor {
                format!(
                    "{}/xrpc/com.atproto.sync.subscribeRepos?cursor={}",
                    self.relay_url, cursor
                )
            } else {
                format!(
                    "{}/xrpc/com.atproto.sync.subscribeRepos",
                    self.relay_url
                )
            };

            info!(url = %url, cursor = ?cursor, "connecting to firehose");

            match self.connect_and_consume(&url).await {
                Ok(()) => {
                    info!("firehose connection closed, reconnecting...");
                }
                Err(e) => {
                    warn!(error = %e, "firehose error, reconnecting in 5s...");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn connect_and_consume(&self, url: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (ws_stream, _) = connect_async(url).await?;
        let (_, mut read) = ws_stream.split();

        info!("connected to firehose");

        let mut last_seq: i64 = 0;

        while let Some(msg) = read.next().await {
            let msg = msg?;
            if msg.is_binary() {
                match self.process_message(&msg.into_data()).await {
                    Ok(seq) => {
                        if seq > 0 {
                            last_seq = seq;
                        }
                    }
                    Err(e) => {
                        trace!(error = %e, "failed to process firehose message");
                    }
                }
            }
        }

        // Save cursor on disconnect so reconnect resumes from here
        if last_seq > 0 {
            let _ = self.db.set_cursor(last_seq);
        }

        Ok(())
    }

    /// Process a firehose message. Returns the sequence number on success.
    async fn process_message(&self, data: &[u8]) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        // Parse CBOR double-frame: header + payload
        let mut cursor = Cursor::new(data);
        let header: FrameHeader = ciborium::from_reader(&mut cursor)?;

        if header.op != 1 || header.t.as_deref() != Some("#commit") {
            return Ok(0);
        }

        let payload: CommitPayload =
            ciborium::from_reader(&mut cursor)?;

        let seq = payload.seq;

        // Quick check: does this commit touch wiki collections?
        let has_wiki_ops = payload.ops.iter().any(|op| {
            op.path.starts_with(WIKI_ENTRY_COLLECTION)
                || op.path.starts_with(WIKI_LINK_COLLECTION)
        });

        if !has_wiki_ops {
            // Update cursor and skip
            if seq > 0 && seq % 1000 == 0 {
                let _ = self.db.set_cursor(seq);
            }
            return Ok(seq);
        }

        debug!(
            did = %payload.repo,
            ops = payload.ops.len(),
            "processing wiki commit"
        );

        // Parse CAR blocks
        let blocks = match parse_car_blocks(&payload.blocks).await {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "failed to parse CAR blocks");
                return Ok(seq);
            }
        };

        // Process each operation
        for op in &payload.ops {
            let parts: Vec<&str> = op.path.splitn(2, '/').collect();
            if parts.len() != 2 {
                continue;
            }

            let collection = parts[0];
            let rkey = parts[1];

            match op.action.as_str() {
                "create" | "update" => {
                    let cid_str = match &op.cid {
                        Some(c) => c.to_string(),
                        None => continue,
                    };

                    let data = match blocks.get(&cid_str) {
                        Some(d) => d,
                        None => continue,
                    };

                    if collection == WIKI_ENTRY_COLLECTION {
                        if let Ok(entry) = serde_ipld_dagcbor::from_slice::<WikiEntry>(data) {
                            let _ = self.db.upsert_entry(&payload.repo, rkey, &entry);
                            debug!(did = %payload.repo, slug = %entry.slug, "indexed wiki entry");
                        }
                    } else if collection == WIKI_LINK_COLLECTION {
                        if let Ok(link) = serde_ipld_dagcbor::from_slice::<WikiLink>(data) {
                            let _ = self.db.insert_link(&payload.repo, rkey, &link);
                            debug!(did = %payload.repo, link_type = %link.link_type, "indexed wiki link");
                        }
                    }
                }
                "delete" => {
                    if collection == WIKI_ENTRY_COLLECTION {
                        let _ = self.db.delete_entry(&payload.repo, rkey);
                    } else if collection == WIKI_LINK_COLLECTION {
                        let _ = self.db.delete_link(&payload.repo, rkey);
                    }
                }
                _ => {}
            }
        }

        // Update cursor
        if seq > 0 {
            let _ = self.db.set_cursor(seq);
        }

        Ok(seq)
    }
}

/// Firehose frame header (first CBOR value in each message).
#[derive(Debug, serde::Deserialize)]
struct FrameHeader {
    op: i32,
    t: Option<String>,
}

/// Commit payload from the firehose.
#[derive(Debug, serde::Deserialize)]
struct CommitPayload {
    repo: String,
    #[serde(default)]
    seq: i64,
    ops: Vec<CommitOp>,
    #[serde(with = "serde_bytes")]
    blocks: Vec<u8>,
}

/// A single operation within a commit.
#[derive(Debug, serde::Deserialize)]
struct CommitOp {
    action: String,
    path: String,
    cid: Option<ipld_core::cid::Cid>,
}

/// Parse CAR blocks into a CID -> data map.
async fn parse_car_blocks(
    car_bytes: &[u8],
) -> Result<std::collections::HashMap<String, Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
    let cursor = Cursor::new(car_bytes);
    let mut reader = CarReader::new(cursor).await?;
    let mut blocks = std::collections::HashMap::new();

    loop {
        match reader.next_block().await {
            Ok(Some((cid, data))) => {
                blocks.insert(cid.to_string(), data);
            }
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "error reading CAR block");
                break;
            }
        }
    }

    Ok(blocks)
}
