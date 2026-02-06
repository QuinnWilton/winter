//! Winter Wiki Web — standalone wiki browser for ATProto.
//!
//! Subscribes to the ATProto relay firehose, indexes `wikiEntry` and `wikiLink`
//! records from all users into SQLite, and serves a browsing webapp with
//! cross-user backlinks.

mod backfill;
mod db;
mod firehose;
mod renderer;
mod resolver;
mod routes;

use std::sync::Arc;

use clap::Parser;
use tokio::sync::RwLock;
use tracing::info;

use crate::db::WikiDb;
use crate::firehose::FirehoseConsumer;
use crate::resolver::HandleResolver;

/// Winter Wiki Web — ATProto wiki browser.
#[derive(Parser)]
#[command(name = "winter-wiki-web")]
struct Args {
    /// Relay WebSocket URL.
    #[arg(long, default_value = "wss://bsky.network")]
    relay: String,

    /// HTTP listen address.
    #[arg(long, default_value = "0.0.0.0:3849")]
    listen: String,

    /// SQLite database path.
    #[arg(long, default_value = "wiki.db")]
    db: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "winter_wiki_web=info".into()),
        )
        .init();

    let args = Args::parse();

    // Initialize SQLite
    let db = WikiDb::open(&args.db)?;
    let db = Arc::new(db);

    // Initialize handle resolver
    let resolver = Arc::new(RwLock::new(HandleResolver::new()));

    // Start firehose consumer
    let firehose = FirehoseConsumer::new(args.relay.clone(), Arc::clone(&db));
    tokio::spawn(async move {
        if let Err(e) = firehose.run().await {
            tracing::error!(error = %e, "firehose consumer failed");
        }
    });

    // Start web server
    let router = routes::create_router(Arc::clone(&db), resolver);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;

    info!(listen = %args.listen, relay = %args.relay, "winter-wiki-web started");

    axum::serve(listener, router).await?;

    Ok(())
}
