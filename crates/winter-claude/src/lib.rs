//! Rust SDK for Claude CLI integration.
//!
//! This crate provides a typed interface to the Claude CLI tool,
//! supporting streaming responses, tool calls, and MCP integration.

#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(missing_docs)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::collapsible_if)]

// Core modules (always available)
pub mod core;
pub mod runtime;

// Feature-gated modules
#[cfg(feature = "mcp")]
pub mod mcp;

#[cfg(feature = "cli")]
pub mod cli;

// Re-export core types for convenience
pub use crate::core::{
    ClaudeResponse, Config, Cost, Error, ExtractedToolCall, Message, MessageMeta, MessageType,
    ResponseMetadata, Result, Session, SessionId, SessionManager, StreamFormat, TokenUsage,
    ToolPermission,
};
// Re-export MCP types when feature is enabled
#[cfg(feature = "mcp")]
pub use crate::mcp::{McpConfig, McpServer};
// Re-export runtime types
pub use crate::runtime::{extract_tool_calls, Client, MessageStream, QueryBuilder};

/// Prelude module for convenient imports
pub mod prelude {
    pub use futures::StreamExt;

    pub use crate::{Client, Config, Error, Message, MessageType, Result, StreamFormat};
}
