//! Error types for the web UI.

use thiserror::Error;

/// Errors that can occur in the web UI.
#[derive(Debug, Error)]
pub enum WebError {
    /// ATProto error.
    #[error("ATProto error: {0}")]
    Atproto(#[from] winter_atproto::AtprotoError),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Template error.
    #[error("template error: {0}")]
    Template(String),
}
