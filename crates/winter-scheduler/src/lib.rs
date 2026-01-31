//! Durable job scheduler for Winter.
//!
//! This crate provides a persistent scheduler that:
//! - Stores job state as ATProto records
//! - Survives crashes and restarts
//! - Supports one-shot and recurring interval jobs
//! - Implements exponential backoff for failures

mod error;
mod scheduler;
mod types;

pub use error::SchedulerError;
pub use scheduler::{JobExecutor, Scheduler};
pub use types::{Job, JobSchedule, JobStatus};
