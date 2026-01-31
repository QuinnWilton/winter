//! Read-only observation web UI for Winter.
//!
//! This crate provides a web interface to observe Winter's:
//! - Stream of consciousness (thoughts)
//! - Facts and knowledge base
//! - Identity (values, interests, self_description)
//! - Scheduled jobs

mod error;
mod routes;
mod sse;
mod thought_stream;

pub use error::WebError;
pub use routes::create_router;
