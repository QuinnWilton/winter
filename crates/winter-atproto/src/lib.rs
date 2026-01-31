//! ATProto XRPC client for Winter's local PDS.
//!
//! This crate provides a client for interacting with an ATProto PDS,
//! including record CRUD operations and authentication.
//!
//! ## Features
//!
//! - **HTTP Client**: XRPC client for record CRUD operations
//! - **CAR Parsing**: Parse CAR files from `getRepo`
//! - **Firehose**: WebSocket subscription to `subscribeRepos`
//! - **Cache**: Thread-safe in-memory cache for facts and rules
//! - **Sync**: Coordinator for CAR hydration with firehose subscription

pub mod cache;
pub mod car;
mod client;
mod error;
pub mod firehose;
mod records;
pub mod sync;
mod types;

pub use cache::{CacheUpdate, CachedRecord, FirehoseCommit, FirehoseOp, RepoCache, SyncState};
pub use car::{CarParseResult, parse_car};
pub use client::AtprotoClient;
pub use error::AtprotoError;
pub use firehose::{DEFAULT_FIREHOSE_URL, FirehoseClient};
pub use records::*;
pub use sync::{SyncCoordinator, SyncCoordinatorBuilder};
pub use types::*;
