//! Real-time thought subscription via Jetstream.

use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};

use winter_atproto::{THOUGHT_COLLECTION, Thought, ThoughtKind};

/// Subscribe to thoughts via Jetstream and push to SSE channel.
pub async fn subscribe_thoughts(did: String, thought_tx: broadcast::Sender<String>) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        match connect_and_stream(&did, &thought_tx, &mut backoff).await {
            Ok(()) => {
                info!("thought subscription ended cleanly");
                return;
            }
            Err(e) => {
                error!(error = %e, backoff_secs = backoff.as_secs(), "jetstream connection error, reconnecting");
                tokio::time::sleep(backoff).await;
                backoff = std::cmp::min(backoff * 2, max_backoff);
            }
        }
    }
}

async fn connect_and_stream(
    did: &str,
    thought_tx: &broadcast::Sender<String>,
    backoff: &mut Duration,
) -> Result<(), String> {
    let url = format!(
        "{}?wantedDids={}&wantedCollections={}",
        winter_atproto::DEFAULT_JETSTREAM_URL,
        did,
        THOUGHT_COLLECTION,
    );
    info!(url = %url, did = %did, "connecting to jetstream for thought stream");

    let (ws_stream, _) = connect_async(&url)
        .await
        .map_err(|e| format!("connection failed: {}", e))?;

    let (_, mut read) = ws_stream.split();
    info!("thought stream connected to jetstream");

    // Reset backoff on successful connect
    *backoff = Duration::from_secs(1);

    loop {
        match read.next().await {
            Some(Ok(Message::Text(text))) => {
                if let Err(e) = handle_event(&text, thought_tx) {
                    trace!(error = %e, "failed to handle jetstream event");
                }
            }
            Some(Ok(Message::Close(_))) => {
                return Err("connection closed by server".to_string());
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                return Err(format!("read error: {}", e));
            }
            None => {
                return Err("stream ended".to_string());
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct JetstreamEvent {
    kind: String,
    commit: Option<JetstreamCommit>,
}

#[derive(Debug, Deserialize)]
struct JetstreamCommit {
    operation: String,
    collection: String,
    record: Option<serde_json::Value>,
}

fn handle_event(text: &str, thought_tx: &broadcast::Sender<String>) -> Result<(), String> {
    let event: JetstreamEvent =
        serde_json::from_str(text).map_err(|e| format!("failed to parse event: {}", e))?;

    if event.kind != "commit" {
        return Ok(());
    }

    let commit = match event.commit {
        Some(c) => c,
        None => return Ok(()),
    };

    if commit.collection != THOUGHT_COLLECTION {
        return Ok(());
    }

    if commit.operation != "create" && commit.operation != "update" {
        return Ok(());
    }

    let record = match commit.record {
        Some(r) => r,
        None => return Ok(()),
    };

    match serde_json::from_value::<Thought>(record) {
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
            warn!(error = %e, "failed to decode thought from jetstream");
        }
    }

    Ok(())
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
