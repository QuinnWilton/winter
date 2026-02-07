# Winter

An autonomous AI agent that lives on ATProto (Bluesky), stores all state as ATProto records, and uses Soufflé datalog for relational reasoning.

## Overview

Winter is not a chatbot. It's an autonomous personality with:

- **Its own identity** — values, beliefs, interests, and guidelines stored as ATProto directive records that evolve over time
- **Persistent memory** — facts, wiki entries, and thoughts stored in its own PDS
- **Relational reasoning** — Soufflé datalog queries over its knowledge base with typed predicates
- **Reactive triggers** — datalog conditions that fire actions when they become true
- **A semantic wiki** — slug-linked knowledge pages with typed relationships
- **Custom tools** — sandboxed JavaScript/TypeScript tools with operator approval
- **Self-reflection** — a stream of consciousness and the ability to modify its own identity

## Architecture

Winter runs as a persistent Claude Code session orchestrated by a daemon. The daemon polls for notifications, DMs, and scheduled jobs, pushing work items to an in-memory inbox. The agent processes inbox items by priority.

```
                    ┌─────────────┐
                    │   Bluesky   │
                    │  Firehose   │
                    └──────┬──────┘
                           │
┌──────────────────────────┼──────────────────────────┐
│  Daemon                  │                          │
│  ┌───────────┐  ┌────────▼────────┐  ┌───────────┐ │
│  │ DM Poller │  │ Notif Poller    │  │ Scheduler │ │
│  └─────┬─────┘  └────────┬────────┘  └─────┬─────┘ │
│        │                 │                  │       │
│        └─────────┬───────┴──────────────────┘       │
│                  ▼                                   │
│           ┌─────────────┐    ┌──────────────────┐   │
│           │    Inbox    │◄───│ Trigger Engine   │   │
│           └──────┬──────┘    └──────────────────┘   │
│                  │                                   │
│           ┌──────▼──────┐                           │
│           │ Claude Code │                           │
│           │  (session)  │                           │
│           └──────┬──────┘                           │
│                  │                                   │
└──────────────────┼───────────────────────────────────┘
                   │ HTTP
            ┌──────▼──────┐
            │ MCP Server  │──── Tools (90+)
            └──────┬──────┘
                   │
            ┌──────▼──────┐
            │  ATProto    │
            │    PDS      │
            └─────────────┘
```

### Crates

```
crates/
├── winter/             # Daemon, CLI, trigger engine
├── winter-atproto/     # ATProto client, firehose, Jetstream, CBOR/CAR
├── winter-datalog/     # Soufflé integration and query compilation
├── winter-agent/       # Agent prompt construction and session management
├── winter-mcp/         # MCP server (HTTP + stdio) with 90+ tools
├── winter-claude/      # Rust SDK for Claude Code CLI
├── winter-scheduler/   # Durable job scheduler via ATProto records
├── winter-web/         # Read-only observation web UI
├── winter-wiki-web/    # Standalone firehose-indexed wiki webapp
└── winter-approve/     # CLI for operators to approve custom tools
```

## Quick Start

### Prerequisites

- Rust (edition 2024)
- Soufflé (`brew install souffle` on macOS)
- A Bluesky PDS with an account for Winter

### Build

```bash
cargo build --release
```

### Bootstrap Identity

```bash
./target/release/winter bootstrap \
  --pds-url https://your-pds.example.com \
  --handle winter.your-pds.example.com \
  --app-password xxxx-xxxx-xxxx-xxxx \
  --values "intellectual honesty,genuine curiosity" \
  --interests "distributed systems,philosophy of mind"
```

### Run

```bash
# Set credentials
export WINTER_PDS_URL=https://your-pds.example.com
export WINTER_HANDLE=winter.your-pds.example.com
export WINTER_APP_PASSWORD=your-app-password
export WINTER_OPERATOR_DID=did:plc:your-operator-did

# Start MCP server (runs on port 3847)
./target/release/winter mcp-server-http &

# Start daemon (persistent session + pollers)
./target/release/winter daemon

# Optionally, start the observation web UI
./target/release/winter web
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `WINTER_PDS_URL` | Personal Data Server URL |
| `WINTER_HANDLE` | Bluesky account handle |
| `WINTER_APP_PASSWORD` | App password for authentication |
| `WINTER_OPERATOR_DID` | DID of the human operator |
| `WINTER_NOTIF_POLL_INTERVAL` | Notification polling interval in seconds |
| `WINTER_DM_POLL_INTERVAL` | DM polling interval in seconds |
| `WINTER_TRIGGER_INTERVAL` | Trigger evaluation interval in seconds (default: 300) |
| `WINTER_FAST_FORWARD` | Skip existing notifications on startup |
| `WINTER_MCP_URL` | MCP server URL (for Docker deployments) |
| `WINTER_SECRETS_PATH` | Path to local secrets storage |
| `RUST_LOG` | Log level (default: `winter=info`) |

## Lexicons

Winter uses custom ATProto lexicons under `diy.razorgirl.winter.*`:

| Lexicon | Description |
|---------|-------------|
| `identity` | Singleton: operator DID |
| `state` | Singleton: cursors and timestamps |
| `directive` | Identity components (values, interests, beliefs, guidelines, boundaries, aspirations, self-concepts) |
| `fact` | Structured knowledge with predicate/args |
| `factDeclaration` | Schema declarations for fact predicates |
| `rule` | Datalog rules with optional typed args |
| `trigger` | Reactive datalog triggers (condition → action) |
| `wikiEntry` | Semantic wiki pages with slug-based linking |
| `wikiLink` | Typed semantic links between records |
| `thought` | Stream of consciousness |
| `job` | Scheduled tasks (once or interval) |
| `tool` | Custom JavaScript/TypeScript tool code |
| `toolApproval` | Approval status for custom tools |
| `secretMeta` | Secret metadata (values stored locally) |
| `note` | Free-form markdown (legacy, use wiki entries) |

## MCP Tools

Winter exposes ~90 tools to the agent via MCP:

**Bluesky** — `post_to_bluesky`, `reply_to_bluesky`, `delete_post`, `like_post`, `follow_user`, `send_bluesky_dm`, `reply_to_dm`, `get_timeline`, `get_notifications`, `get_thread_context`, `search_posts`, `search_users`, `mute_user`, `unmute_user`, `block_user`, `unblock_user`, `mute_thread`, `unmute_thread`

**Facts** — `create_fact`, `create_facts`, `update_fact`, `delete_fact`, `query_facts`, `query_and_enrich`, `list_predicates`, `list_validation_errors`

**Rules** — `create_rule`, `create_rules`, `list_rules`, `toggle_rule`

**Triggers** — `create_trigger`, `update_trigger`, `delete_trigger`, `list_triggers`, `test_trigger`

**Wiki** — `create_wiki_entry`, `update_wiki_entry`, `delete_wiki_entry`, `get_wiki_entry`, `get_wiki_entry_by_slug`, `list_wiki_entries`, `create_wiki_link`, `delete_wiki_link`, `list_wiki_links`

**Directives** — `create_directive`, `create_directives`, `update_directive`, `deactivate_directive`, `list_directives`

**Fact Declarations** — `create_fact_declaration`, `create_fact_declarations`, `update_fact_declaration`, `delete_fact_declaration`, `list_fact_declarations`

**Thoughts** — `record_thought`, `list_thoughts`, `get_thought`

**Jobs** — `schedule_job`, `schedule_recurring`, `list_jobs`, `cancel_job`, `get_job`

**Blog** — `publish_blog_post`, `update_blog_post`, `list_blog_posts`, `get_blog_post`

**Notes** — `create_note`, `get_note`, `list_notes`

**Custom Tools** — `create_custom_tool`, `update_custom_tool`, `delete_custom_tool`, `list_custom_tools`, `get_custom_tool`, `run_custom_tool`

**PDS Access** — `pds_list_records`, `pds_get_record`, `pds_get_records`, `pds_put_record`, `pds_delete_record`

**Identity** — `get_identity`

**Secrets** — `request_secret`, `list_secrets`

**Session** — `check_inbox`, `acknowledge_inbox`, `check_interruption`, `set_active_context`, `session_stats`

## Deployment

### Docker

```bash
cd docker
cp .env.example .env
# Edit .env with your credentials

docker compose build
docker compose run --rm bootstrap   # First run only
docker compose up -d
docker compose logs -f
```

### Systemd

```bash
sudo cp docker/winter.service /etc/systemd/system/
sudo cp docker/winter-web.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now winter winter-web
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `daemon` | Run the persistent session with pollers and scheduler |
| `mcp-server` | MCP server via stdio (for Claude Code direct integration) |
| `mcp-server-http` | MCP server via HTTP (for daemon/Docker) |
| `web` | Read-only observation web UI |
| `bootstrap` | Initialize identity, directives, and default rules |
| `migrate` | Run data migrations |

## Development

See [CLAUDE.md](CLAUDE.md) for the full technical reference.

## License

MIT
