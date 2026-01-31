//! Error types for the ATProto client.

use thiserror::Error;

/// Errors that can occur when interacting with ATProto.
#[derive(Debug, Error)]
pub enum AtprotoError {
    /// Authentication failed.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// HTTP request failed.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization/deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Record not found.
    #[error("record not found: {collection}/{rkey}")]
    NotFound { collection: String, rkey: String },

    /// Invalid response from server.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// Rate limited.
    #[error("rate limited{}", match (endpoint, retry_after_secs) {
        (Some(ep), Some(secs)) => format!(" on {} (retry after {}s)", ep, secs),
        (Some(ep), None) => format!(" on {}", ep),
        (None, Some(secs)) => format!(" (retry after {}s)", secs),
        (None, None) => String::new(),
    })]
    RateLimited {
        /// The endpoint that was rate limited (optional).
        endpoint: Option<String>,
        /// Seconds to wait before retrying (from Retry-After header, optional).
        retry_after_secs: Option<u64>,
    },

    /// XRPC error from server.
    #[error("XRPC error: {error} - {message}")]
    Xrpc { error: String, message: String },

    /// CAR parsing error.
    #[error("CAR parse error: {0}")]
    CarParse(String),

    /// CBOR decoding error.
    #[error("CBOR decode error: {0}")]
    CborDecode(String),

    /// WebSocket error.
    #[error("WebSocket error: {0}")]
    WebSocket(String),

    /// Sync error.
    #[error("sync error: {0}")]
    Sync(String),
}
