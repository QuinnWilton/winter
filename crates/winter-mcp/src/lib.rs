//! MCP (Model Context Protocol) server for Winter.
//!
//! This crate implements an MCP server that exposes Winter's tools
//! to Claude Code via JSON-RPC. It supports two transports:
//! - stdio: Traditional stdin/stdout for local development
//! - HTTP: Persistent server for production (reduces startup latency)

pub mod bluesky;
pub mod deno;
pub mod http;
pub mod protocol;
pub mod secrets;
pub mod server;
pub mod tools;

pub use bluesky::{BlueskyClient, BlueskyError};
pub use deno::{DenoError, DenoExecutor, DenoOutput, DenoPermissions};
pub use secrets::{SecretError, SecretManager};
pub use server::McpServer;
pub use tools::{InterruptionState, ToolMeta, ToolRegistry};
