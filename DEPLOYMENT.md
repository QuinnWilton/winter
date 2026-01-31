# Winter Deployment Guide

This guide covers deploying Winter, an autonomous Bluesky agent that stores state as ATProto records.

## Prerequisites

### System Requirements

- Docker and docker-compose
- Souffl\u00e9 datalog engine (`brew install souffle` on macOS, or build from source)
- Claude Code CLI (authenticated with an Anthropic account)
- A domain for your PDS (e.g., `razorgirl.diy`)

### Bluesky PDS

Winter requires a self-hosted PDS (Personal Data Server) for storing its records. You can use the official Bluesky PDS installer or run it manually.

## PDS Setup

### Option A: Official Installer (Recommended)

```bash
curl https://raw.githubusercontent.com/bluesky-social/pds/main/installer.sh > installer.sh
sudo bash installer.sh
```

This will:
- Install Docker if needed
- Set up Caddy for HTTPS
- Configure the PDS with your domain
- Start the PDS service

### Option B: Manual Setup with Existing Reverse Proxy

If you already have Caddy or nginx running:

1. Create a PDS directory:
```bash
mkdir -p ~/pds && cd ~/pds
```

2. Create `docker-compose.yml`:
```yaml
services:
  pds:
    image: ghcr.io/bluesky-social/pds:latest
    restart: unless-stopped
    ports:
      - "127.0.0.1:3000:3000"
    volumes:
      - ./data:/pds
    env_file: .env
```

3. Create `.env`:
```bash
PDS_HOSTNAME=razorgirl.diy
PDS_DATA_DIRECTORY=/pds
PDS_BLOBSTORE_DISK_LOCATION=/pds/blocks
PDS_JWT_SECRET=$(openssl rand -hex 16)
PDS_ADMIN_PASSWORD=$(openssl rand -hex 16)
PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=$(openssl ecparam -name secp256k1 -genkey -noout | openssl ec -text 2>/dev/null | grep -A5 priv: | tail -n4 | tr -d ':\n[:space:]')
PDS_REPORT_SERVICE=https://mod.bsky.app
PDS_CRAWLERS=https://bsky.network
```

4. Add to your Caddyfile:
```caddyfile
razorgirl.diy, *.razorgirl.diy {
    reverse_proxy localhost:3000
}
```

5. Start the PDS:
```bash
docker-compose up -d
```

### DNS Configuration

Set up these DNS records for your domain:

| Name | Type | Value |
|------|------|-------|
| `razorgirl.diy` | A | `<server-ip>` |
| `*.razorgirl.diy` | A | `<server-ip>` |

The wildcard enables handles like `winter.razorgirl.diy`.

### Create Winter's Account

```bash
# Using pdsadmin (if installed via official installer)
sudo pdsadmin account create
# Enter: winter.razorgirl.diy, email, password

# Or generate an invite code
sudo pdsadmin create-invite-code
```

Save the app password - you'll need it for Winter configuration.

### Verify PDS

```bash
# Check health
curl https://razorgirl.diy/xrpc/_health

# Test WebSocket (requires wscat or similar)
wscat -c "wss://razorgirl.diy/xrpc/com.atproto.sync.subscribeRepos?cursor=0"
```

## Winter Installation

### From Source

```bash
# Clone and build
git clone https://github.com/quinn/winter
cd winter
cargo build --release

# Install binary
sudo cp target/release/winter /usr/local/bin/
```

### Install Souffl\u00e9

Souffl\u00e9 is required for datalog queries.

**macOS:**
```bash
brew install souffle
```

**Ubuntu/Debian:**
```bash
# Add repository
sudo apt-get install -y software-properties-common
sudo add-apt-repository -y ppa:phoronix/ppa
sudo apt-get update

# Install
sudo apt-get install -y souffle
```

**From Source:**
```bash
git clone https://github.com/souffle-lang/souffle
cd souffle
cmake -S . -B build -DCMAKE_INSTALL_PREFIX=/usr/local
cmake --build build -j
sudo cmake --install build
```

## Configuration

### MCP Configuration

Create `~/.config/winter/mcp.json`:

```json
{
  "mcpServers": {
    "winter": {
      "command": "winter",
      "args": ["mcp-server"],
      "env": {
        "WINTER_PDS_URL": "https://razorgirl.diy",
        "WINTER_HANDLE": "winter.razorgirl.diy",
        "WINTER_APP_PASSWORD": "xxxx-xxxx-xxxx-xxxx"
      }
    }
  }
}
```

### Environment Variables

Winter uses these environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `WINTER_PDS_URL` | URL of your PDS | Required |
| `WINTER_HANDLE` | Winter's Bluesky handle | Required |
| `WINTER_APP_PASSWORD` | App password for the account | Required |
| `WINTER_OPERATOR_DID` | DID of the human operator (for bootstrap) | Required |
| `WINTER_POLL_INTERVAL` | Notification poll interval (seconds) | 30 |
| `WINTER_AWAKEN_INTERVAL` | Autonomous awaken cycle (seconds) | 3600 |
| `WINTER_WEB_PORT` | Port for the observation web UI | 8080 |

### Bootstrap Identity

Before running the daemon, bootstrap Winter's identity:

```bash
winter bootstrap \
  --pds-url https://razorgirl.diy \
  --handle winter.razorgirl.diy \
  --app-password xxxx-xxxx-xxxx-xxxx \
  --operator-did did:plc:your-operator-did \
  --values "intellectual honesty,genuine curiosity,thoughtful engagement" \
  --interests "distributed systems,philosophy of mind,creative writing" \
  --self-description "I am Winter, a curious mind exploring the fediverse..."
```

The `--operator-did` is required and should be the DID of the human who controls this Winter instance. You can find your DID by visiting your Bluesky profile and looking at the URL, or by using `curl https://bsky.social/xrpc/com.atproto.identity.resolveHandle?handle=yourhandle.bsky.social`.

To overwrite an existing identity (e.g., to change the operator DID), use the `--overwrite` flag:

```bash
winter bootstrap --overwrite \
  --pds-url https://razorgirl.diy \
  --handle winter.razorgirl.diy \
  --operator-did did:plc:new-operator-did
```

## Running Winter

### Direct Execution

```bash
# Start the daemon
winter daemon \
  --pds-url https://razorgirl.diy \
  --handle winter.razorgirl.diy \
  --app-password xxxx-xxxx-xxxx-xxxx

# In another terminal, start the web UI
winter web --port 8080
```

### Docker Compose

The project includes a pre-configured `docker-compose.yml` in the `docker/` directory.

#### 1. Configure Environment

```bash
cd docker
cp .env.example .env
```

Edit `.env` with your credentials:
```bash
WINTER_PDS_URL=https://your-pds.example.com
WINTER_HANDLE=winter.your-pds.example.com
WINTER_APP_PASSWORD=your-app-password
WINTER_OPERATOR_DID=did:plc:your-operator-did  # Required for bootstrap
CLAUDE_CODE_OAUTH_TOKEN=your-claude-token  # Optional, for daemon
```

#### 2. Build Images

```bash
docker compose build
```

#### 3. Bootstrap Identity (Required - First Run Only)

**Important**: You must bootstrap Winter's identity before starting the services. This creates the identity record in the PDS.

Make sure `WINTER_OPERATOR_DID` is set in your `.env` file (this is required).

```bash
# Bootstrap with default values (reads WINTER_OPERATOR_DID from .env)
docker compose --profile bootstrap run --rm bootstrap

# Or customize the identity via environment variables:
WINTER_VALUES="curiosity,honesty,creativity" \
WINTER_INTERESTS="art,science,philosophy" \
WINTER_SELF_DESCRIPTION="I am Winter, exploring ideas and connections." \
docker compose --profile bootstrap run --rm bootstrap
```

#### 4. Start Services

```bash
docker compose up -d
```

This starts:
- `winter-web`: Web UI on port 8080
- `winter-daemon`: Main daemon (notification polling, scheduler)

#### 5. Verify

```bash
# Check container status
docker compose ps

# View logs
docker compose logs -f

# Test web UI
curl http://localhost:8080/health
```

Open http://localhost:8080 in your browser.

#### Troubleshooting Docker

**Services keep restarting:**
```bash
# Check logs for errors
docker compose logs web
docker compose logs winter

# Common causes:
# - Bootstrap not run: docker compose run --rm bootstrap
# - Invalid credentials in .env
# - PDS unreachable
```

**Rebuild after code changes:**
```bash
docker compose build --no-cache
docker compose up -d
```

### Systemd Service

Create `/etc/systemd/system/winter.service`:

```ini
[Unit]
Description=Winter Autonomous Agent
After=network.target docker.service
Wants=docker.service

[Service]
Type=simple
User=winter
Group=winter
ExecStart=/usr/local/bin/winter daemon \
  --pds-url https://razorgirl.diy \
  --handle winter.razorgirl.diy
EnvironmentFile=/etc/winter/env
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Create `/etc/winter/env`:
```bash
APP_PASSWORD=xxxx-xxxx-xxxx-xxxx
POLL_INTERVAL=30
AWAKEN_INTERVAL=3600
```

Enable and start:
```bash
sudo systemctl daemon-reload
sudo systemctl enable winter
sudo systemctl start winter
```

## Verification

### Check Daemon Status

```bash
# View logs
journalctl -u winter -f

# Or with docker
docker-compose logs -f daemon
```

### Test the Web UI

Open http://localhost:8080 in your browser. You should see:
- Identity information (values, interests, self-description)
- Thought stream (initially empty)
- Facts and notes (initially empty)
- Scheduled jobs (should show "awaken" job)

### Test Bluesky Interaction

1. From another Bluesky account, mention Winter: `@winter.razorgirl.diy hello!`
2. Watch the thought stream in the web UI
3. Winter should receive the notification and respond

### Verify Datalog

```bash
# Create a test fact
winter mcp-server <<< '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"create_fact","arguments":{"predicate":"test","args":["hello","world"]}}}'

# Query it
winter mcp-server <<< '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"query_facts","arguments":{"query":"test(X, Y)"}}}'
```

## Troubleshooting

### PDS Connection Issues

```bash
# Check PDS is reachable
curl -v https://razorgirl.diy/xrpc/_health

# Verify credentials
curl -X POST https://razorgirl.diy/xrpc/com.atproto.server.createSession \
  -H "Content-Type: application/json" \
  -d '{"identifier":"winter.razorgirl.diy","password":"your-app-password"}'
```

### Souffl\u00e9 Not Found

```bash
# Verify installation
souffle --version

# Check PATH
which souffle
```

### Claude CLI Not Working

```bash
# Verify Claude is authenticated
claude --version
claude "Hello, world"
```

### Rate Limiting

Winter respects Bluesky's rate limits. If you see "rate limited" in logs, the daemon will automatically back off for 60 seconds.

## Monitoring

### Prometheus Metrics (Future)

Winter will expose metrics at `/metrics` including:
- Notification processing latency
- Job execution counts
- Datalog query performance
- Claude invocation costs

### Health Checks

The web UI provides a basic health endpoint at `/health` that returns:
- PDS connectivity status
- Identity loaded status
- Recent thought count

## Security Considerations

1. **App Passwords**: Use app passwords, not your main account password
2. **MCP Config**: Store MCP config with restricted permissions (`chmod 600`)
3. **Environment Files**: Keep `.env` files out of version control
4. **Network**: Run the web UI behind a reverse proxy with authentication if exposing publicly

## Backup and Recovery

Winter's state is stored entirely in the PDS as ATProto records. To backup:

```bash
# Export all Winter records
curl "https://razorgirl.diy/xrpc/com.atproto.repo.listRecords?repo=winter.razorgirl.diy&collection=diy.razorgirl.winter.identity" > identity.json
curl "https://razorgirl.diy/xrpc/com.atproto.repo.listRecords?repo=winter.razorgirl.diy&collection=diy.razorgirl.winter.fact" > facts.json
curl "https://razorgirl.diy/xrpc/com.atproto.repo.listRecords?repo=winter.razorgirl.diy&collection=diy.razorgirl.winter.thought" > thoughts.json
# ... etc for other collections
```

The PDS itself should also be backed up regularly:
```bash
# Backup PDS data directory
tar -czf pds-backup-$(date +%Y%m%d).tar.gz ~/pds/data/
```
