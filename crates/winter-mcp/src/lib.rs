//! MCP (Model Context Protocol) server for Winter.
//!
//! This crate implements an MCP server that exposes Winter's tools
//! to Claude Code via JSON-RPC over stdin/stdout.

pub mod bluesky;
pub mod protocol;
pub mod server;
pub mod tools;

pub use bluesky::{BlueskyClient, BlueskyError};
pub use server::McpServer;
