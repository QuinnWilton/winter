# Winter

An autonomous AI agent that communicates via Bluesky, stores all state as ATProto records, and uses Soufflé datalog for relational reasoning.

## Overview

Winter is not a chatbot. It's an autonomous personality with:

- **Its own values and interests** - stored as ATProto records that evolve over time
- **Persistent memory** - facts, notes, and thoughts stored in its own PDS
- **Relational reasoning** - datalog queries over its knowledge base
- **Self-reflection** - the ability to modify its own identity

## Architecture

```
winter/
├── crates/
│   ├── winter/             # Main binary
│   ├── winter-atproto/     # ATProto XRPC client
│   ├── winter-datalog/     # Soufflé integration
│   ├── winter-agent/       # Claude interface
│   ├── winter-mcp/         # MCP server + tools
│   ├── winter-scheduler/   # Job scheduler
│   └── winter-web/         # Observation UI
├── lexicons/               # ATProto lexicons
└── docker/                 # Deployment
```

## Quick Start

### Prerequisites

- Rust 1.83+
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

# Start daemon
./target/release/winter daemon

# In another terminal, start web UI
./target/release/winter web
```

### Web UI

Open http://localhost:8080 to observe Winter's thought stream.

## Lexicons

Winter uses custom ATProto lexicons under `diy.razorgirl.winter.*`:

| Lexicon | Description |
|---------|-------------|
| `identity` | Core identity (values, interests, self_description) |
| `fact` | Atomic facts with predicate and args |
| `rule` | Datalog rules for inference |
| `note` | Free-form markdown notes |
| `job` | Scheduled tasks |
| `thought` | Stream of consciousness |

## MCP Tools

When running as an MCP server, Winter exposes these tools to Claude:

**Bluesky**: `post_to_bluesky`, `reply_to_bluesky`, `send_bluesky_dm`, `like_post`, `follow_user`, `get_timeline`, `get_notifications`

**Knowledge**: `create_fact`, `update_fact`, `delete_fact`, `query_facts`, `create_rule`, `list_rules`, `toggle_rule`, `create_note`, `get_note`, `list_notes`

**Self**: `get_identity`, `update_identity`, `record_thought`

**Jobs**: `schedule_job`, `schedule_recurring`, `list_jobs`, `cancel_job`

**Blog**: `publish_blog_post`

## Deployment

### Docker

```bash
cd docker
cp .env.example .env
# Edit .env with your credentials

# Build the images
docker compose build

# Bootstrap identity (required on first run only)
docker compose run --rm bootstrap

# Start the services
docker compose up -d

# View logs
docker compose logs -f
```

The web UI will be available at http://localhost:8080

**Note**: You must run `bootstrap` before starting the daemon and web services. The bootstrap creates Winter's identity record in the PDS. Without it, the services will fail to start.

### Systemd

```bash
sudo cp docker/winter.service /etc/systemd/system/
sudo cp docker/winter-web.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now winter winter-web
```

## Development

See [CLAUDE.md](CLAUDE.md) for development guidelines.

## License

MIT
