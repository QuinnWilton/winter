# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is **claude-sdk-rs**, a type-safe, async-first Rust SDK that wraps the Claude Code CLI to provide a programmatic API for interacting with Claude AI. The project transforms the CLI tool into a powerful library for Rust applications.

## Common Development Commands

```bash
# Build and test
cargo build                    # Build the crate
cargo test                     # Run all tests (unit + integration)
cargo fmt                      # Format code
cargo clippy                   # Run linter

# Build with specific features
cargo build --features cli     # Build with CLI binary
cargo build --features mcp     # Build with MCP support
cargo build --features sqlite  # Build with SQLite storage
cargo build --all-features     # Build with all features

# Run examples
cargo run --example basic_usage      # Simple usage example
cargo run --example streaming        # Streaming responses
cargo run --example session_management # Session handling
cargo run --example error_handling   # Error handling patterns
cargo run --example configuration    # Configuration options

# Run CLI (requires cli feature)
cargo run --features cli --bin claude-sdk-rs -- help

# Publishing
./scripts/publish.sh          # Publish crate to crates.io

# Testing with features
cargo test --features sqlite  # Test with SQLite support
cargo test --features mcp     # Test with MCP support
cargo test --all-features     # Test with all features enabled
```

## Architecture Overview

**Single Crate Structure**: The project is organized as a single crate with feature flags to enable optional functionality:
- **Core SDK** (default): Essential types, client, configuration, and runtime
- **CLI feature** (`cli`): Command-line interface and interactive tools
- **MCP feature** (`mcp`): Model Context Protocol support for tool integration
- **SQLite feature** (`sqlite`): Persistent session storage with SQLite
- **Analytics feature** (`analytics`): Usage metrics and performance tracking (requires `cli`)

**Key Design Patterns**:
- **Builder Pattern**: `Client::builder()` and `Config::builder()` for fluent configuration
- **Three Response Modes**: Simple text (`send()`), full metadata (`send_full()`), streaming (`stream()`)
- **Type-Safe Tool Integration**: Tool permissions as typed enums, format: `mcp__server__tool`
- **Session Management**: Persistent sessions with `SessionManager` and `SessionId` tracking

## Core Architecture Concepts

**Configuration System**: `Config` struct supports:
- Three streaming formats: `Text`, `Json`, `StreamJson`  
- Timeouts (default 30s), system prompts, model selection
- Tool permissions and MCP server configuration
- Builder pattern with validation

**Response Handling Flow**:
1. `Client` calls `execute_claude()` in `process.rs` to spawn CLI process
2. Response parsed based on `StreamFormat` in client
3. Text → simple string, Json → `ClaudeResponse` with metadata, StreamJson → parsed message stream
4. All responses wrapped in `Result<T>` with comprehensive error types

**Error Architecture**: `Error` enum in `src/core/error.rs`:
- `ProcessError` - CLI execution failures
- `SerializationError` - JSON parsing issues  
- `BinaryNotFound` - Claude CLI not installed
- `Timeout` - Operation timeouts
- All errors implement `std::error::Error` via `thiserror`

**Session Management**: 
- `SessionManager` creates/persists sessions
- `SessionId` tracks conversation continuity
- Sessions stored with metadata (timestamps, costs, token usage)

## Critical Dependencies

**External Requirements**:
- Claude Code CLI must be installed and authenticated
- Rust 1.70+ required
- Tokio async runtime for all operations

**Key External Crates**:
- `tokio` - Async runtime and process spawning
- `serde`/`serde_json` - Response serialization
- `reqwest` - HTTP client for runtime operations
- `which` - Binary detection for Claude CLI
- `thiserror` - Error handling
- `sqlx` - SQLite support (optional, with `sqlite` feature)
- `clap` - CLI argument parsing (optional, with `cli` feature)

## Testing Strategy

**Test Organization**:
- Unit tests in `src/` modules alongside the code
- Integration tests in `tests/` directory
- Examples serve as integration tests via `cargo run --example`
- Feature-specific tests use conditional compilation (`#[cfg(feature = "...")]`)

**Note on CLI Integration Tests**: Tests that require the real Claude CLI binary (process execution, authentication, network access) are not included in the test suite. Since this SDK wraps an external CLI tool, testing the actual CLI integration is the responsibility of downstream consumers like Winter, which can run integration tests in environments with the CLI installed and authenticated.

**Test Tools**:
- `proptest` - Property-based testing for core types
- `insta` - Snapshot testing for response parsing
- `wiremock` - HTTP mocking for runtime tests
- `dotenv` - Environment config for integration tests

## Development Notes

**Feature Flag Guidelines**: 
- Keep core functionality dependency-free
- Feature flags should be additive (no breaking changes)
- Document feature requirements in examples and docs

**Publishing**: The `publish.sh` script handles the publication process with retry logic and validation.

**Stream Format Behavior**:
- `Text`: Raw CLI output, no parsing
- `Json`: Single structured response with metadata  
- `StreamJson`: Multiple JSON messages, requires parsing lines

**Tool Integration**: Tools use format `mcp__server__tool` or `bash:command`. The `allowed_tools` config takes a `Vec<String>` of these identifiers.