//! Error types for the scheduler.

use thiserror::Error;

/// Errors that can occur in scheduler operations.
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// ATProto error.
    #[error("ATProto error: {0}")]
    Atproto(#[from] winter_atproto::AtprotoError),

    /// Job already exists.
    #[error("job already exists: {0}")]
    JobExists(String),

    /// Job not found.
    #[error("job not found: {0}")]
    JobNotFound(String),

    /// Invalid job configuration.
    #[error("invalid job configuration: {0}")]
    InvalidConfig(String),

    /// Job execution failed.
    #[error("job execution failed: {0}")]
    ExecutionFailed(String),
}
