//! Error types for the agent.

use thiserror::Error;

/// Errors that can occur in agent operations.
#[derive(Debug, Error)]
pub enum AgentError {
    /// ATProto error.
    #[error("ATProto error: {0}")]
    Atproto(#[from] winter_atproto::AtprotoError),

    /// Datalog error.
    #[error("Datalog error: {0}")]
    Datalog(#[from] winter_datalog::DatalogError),

    /// Identity not found.
    #[error("identity not found")]
    IdentityNotFound,

    /// Claude SDK error.
    #[error("Claude error: {0}")]
    Claude(#[from] winter_claude::Error),

    /// Operation timed out.
    #[error("operation timed out: {0}")]
    Timeout(String),

    /// Invalid configuration.
    #[error("invalid configuration: {0}")]
    Config(String),
}
