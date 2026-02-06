//! ATProto XRPC client for Winter's local PDS.
//!
//! This crate provides a client for interacting with an ATProto PDS,
//! including record CRUD operations and authentication.
//!
//! ## Features
//!
//! - **HTTP Client**: XRPC client for record CRUD operations
//! - **Jetstream**: JSON WebSocket subscription for real-time updates
//! - **Cache**: Thread-safe in-memory cache for facts and rules
//! - **Sync**: Coordinator for list_all_records hydration with Jetstream subscription

pub mod cache;
pub mod car;
mod client;
pub mod dispatch;
mod error;
pub mod jetstream;
mod records;
pub mod sync;
mod types;
mod uri;

pub use cache::{CacheUpdate, CachedRecord, RepoCache, ScopeFilter, SyncState};
pub use car::{CarParseResult, parse_car};
pub use dispatch::{dispatch_create_or_update_json, dispatch_delete, extract_record_to_result, is_tracked_collection};
pub use client::{ApplyWritesResponse, AtprotoClient, CommitInfo, WriteOp, WriteResult};
pub use error::AtprotoError;
pub use jetstream::{DEFAULT_JETSTREAM_URL, JetstreamClient, OperatorEvent, OperatorEventCallback};
pub use records::*;
pub use sync::{SyncCoordinator, SyncCoordinatorBuilder};
pub use types::*;
pub use types::{FactDeclArg, FactDeclaration};
pub use uri::{AtUri, AtUriError};
