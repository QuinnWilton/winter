//! Bluesky integration for Winter MCP.
//!
//! Provides a client for interacting with the Bluesky AT Protocol,
//! including posting, replying, DMs, and timeline access.

mod client;
mod types;

pub use client::{BlueskyClient, BlueskyError};
pub use types::*;
