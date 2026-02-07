//! Error types for the datalog integration.

use thiserror::Error;

/// Errors that can occur in datalog operations.
#[derive(Debug, Error)]
pub enum DatalogError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// ATProto error.
    #[error("ATProto error: {0}")]
    Atproto(#[from] winter_atproto::AtprotoError),

    /// Soufflé execution failed.
    #[error("Soufflé execution failed: {0}")]
    Execution(String),

    /// Invalid rule syntax.
    #[error("invalid rule syntax: {0}")]
    InvalidRule(String),

    /// Parse error.
    #[error("parse error: {0}")]
    Parse(String),

    /// Soufflé not found.
    #[error("Soufflé not found in PATH")]
    SouffleNotFound,

    /// Timeout.
    #[error("query timed out after {0}ms")]
    Timeout(u64),

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(String),
}
