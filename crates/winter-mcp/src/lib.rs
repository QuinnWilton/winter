//! MCP (Model Context Protocol) server for Winter.
//!
//! This crate implements an MCP server that exposes Winter's tools
//! to Claude Code via JSON-RPC over stdin/stdout.

pub mod bluesky;
pub mod deno;
pub mod protocol;
pub mod secrets;
pub mod server;
pub mod tools;

pub use bluesky::{BlueskyClient, BlueskyError};
pub use deno::{DenoError, DenoExecutor, DenoOutput, DenoPermissions};
pub use secrets::{SecretError, SecretManager};
pub use server::McpServer;
