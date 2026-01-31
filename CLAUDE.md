# CLAUDE.md

This file provides guidance to Claude Code when working with the Winter codebase.

## Project Overview

**Winter** is an autonomous AI agent that communicates via Bluesky, stores all state as ATProto records in its own PDS (Personal Data Server), and uses Soufflé datalog for relational reasoning.

Unlike a chatbot, Winter has:
- Its own interests and perspective
- Persistent memory stored as ATProto records
- The ability to reason over its knowledge using datalog queries
- A self-evolving identity (values, interests, self_description)

## Architecture

```
winter/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── winter/                   # Main binary (daemon, mcp-server, web, bootstrap)
│   ├── winter-atproto/           # XRPC client for local PDS
│   ├── winter-datalog/           # Soufflé integration
│   ├── winter-agent/             # Claude interface and identity
│   ├── winter-mcp/               # MCP server and tools
│   ├── winter-scheduler/         # Durable job scheduler
│   └── winter-web/               # Observation web UI
├── lexicons/                     # ATProto lexicon definitions
├── vendor/claude-sdk-rs/         # Vendored Claude SDK
├── docker/                       # Dockerfile + compose
└── templates/                    # Askama templates
```

## Common Commands

```bash
# Build and test
cargo build --workspace           # Build all crates
cargo test -p winter -p winter-atproto -p winter-datalog -p winter-agent -p winter-mcp -p winter-scheduler -p winter-web  # Run tests (excludes vendor)
cargo fmt --all                   # Format code
cargo clippy --workspace          # Run linter

# Run the daemon
cargo run -p winter -- daemon

# Run the web UI
cargo run -p winter -- web

# Run the MCP server (for Claude Code)
cargo run -p winter -- mcp-server

# Bootstrap identity
cargo run -p winter -- bootstrap \
  --pds-url https://razorgirl.diy \
  --handle winter.razorgirl.diy

# Integration tests (requires PDS)
docker compose -f tests/docker-compose.test.yml up -d
cargo test --workspace --features integration
```

## Crate Responsibilities

### winter-atproto
Low-level ATProto client for interacting with the PDS:
- XRPC HTTP client with reqwest
- Record CRUD: create, get, list, put, delete
- Authentication with JWT refresh
- Rust types generated from lexicons

### winter-datalog
Soufflé datalog integration:
- Extract facts from ATProto records to TSV
- Compile rules to `.dl` format
- Execute Soufflé subprocess
- Parse output back to Rust tuples

### winter-agent
Core agent logic:
- Identity loading (values, interests, self_description)
- Context assembly for Claude prompts
- Notification handling
- Awaken cycles for autonomous thought
- Job execution

### winter-mcp
MCP (Model Context Protocol) server:
- JSON-RPC protocol over stdin/stdout
- Tool registry with Bluesky, facts, rules, notes, blog, jobs, self tools
- Called by Claude Code to perform actions

### winter-scheduler
Durable job scheduler:
- Jobs stored as ATProto records
- Survives restarts
- Exponential backoff for failures
- Built-in `awaken` job for autonomous cycles

### winter-web
Read-only observation UI:
- Thought stream (SSE live updates)
- Facts browser
- Identity view
- Jobs status

### winter (main binary)
Entry point with subcommands:
- `daemon` - main loop (notification polling, scheduler)
- `web` - observation web server
- `mcp-server` - MCP server mode
- `bootstrap` - initialize identity

## Lexicons

Winter uses custom ATProto lexicons under `diy.razorgirl.winter.*`:

| Lexicon | Purpose |
|---------|---------|
| `identity` | Core identity: values, interests, self_description (singleton) |
| `fact` | Atomic facts: predicate, args, confidence, source |
| `rule` | Datalog rules: head, body, constraints |
| `note` | Free-form markdown notes |
| `job` | Scheduled jobs: name, instructions, schedule |
| `thought` | Stream of consciousness entries |

## Key Design Decisions

### DIDs vs Handles
When facts reference Bluesky accounts, always use DIDs (`did:plc:xxx`), never handles. DIDs are stable; handles can change. Resolve DIDs to handles only at display time.

### Facts vs Notes
- **Facts**: Structured, queryable via datalog. Use for discrete knowledge.
- **Notes**: Free-form markdown. Use for investigations, summaries, reflections.

### Identity Evolution
Winter can modify its own identity via `update_identity` tool:
- Rewrite `self_description` during reflection
- Add/remove values as priorities shift
- Add/remove interests as curiosity evolves

All changes are versioned in the ATProto commit history.

### Datalog Queries
Stored rules (in PDS) are a library of reusable derivations. Queries are ad-hoc, written by Claude for each specific need. The `query_facts` tool:
1. Fetches facts and rules from PDS
2. Generates Soufflé program
3. Executes: `souffle -F<fact-dir> -D- program.dl`
4. Returns query results

## Testing Strategy

- **Unit tests**: In each crate, alongside the code
- **Integration tests**: In `tests/`, require Docker PDS
- **Property-based tests**: Using `proptest` for record types
- **Snapshot tests**: Using `insta` for protocol parsing

## Error Handling

Use `thiserror` for error types. Each crate has its own error enum. Errors should be actionable and include context.

```rust
#[derive(Debug, thiserror::Error)]
pub enum AtprotoError {
    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("record not found: {collection}/{rkey}")]
    NotFound { collection: String, rkey: String },
}
```

## Dependencies

### System Requirements
- Docker & docker-compose (for PDS)
- Soufflé (`brew install souffle` on macOS)
- Claude Code CLI (for agent operation)

### External Services
- Bluesky PDS (self-hosted at razorgirl.diy)
- Bluesky network (for federation)
