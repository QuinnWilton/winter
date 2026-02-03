#!/bin/bash
set -euo pipefail

# Winter systemd installation script
# Run as root: sudo ./install.sh

INSTALL_DIR="/opt/winter"
SERVICE_USER="winter"

echo "==> Installing Winter systemd services"

# Check if running as root
if [[ $EUID -ne 0 ]]; then
    echo "Error: This script must be run as root (sudo ./install.sh)"
    exit 1
fi

# Check if winter binary exists
if [[ ! -f /usr/local/bin/winter ]]; then
    echo "Warning: /usr/local/bin/winter not found"
    echo "Make sure to build and install the binary first:"
    echo "  cargo build --release"
    echo "  sudo cp target/release/winter /usr/local/bin/"
fi

# Create winter user if it doesn't exist
if ! id "$SERVICE_USER" &>/dev/null; then
    echo "==> Creating $SERVICE_USER user"
    useradd -r -s /sbin/nologin -d "$INSTALL_DIR" "$SERVICE_USER"
else
    echo "==> User $SERVICE_USER already exists"
fi

# Create installation directory
echo "==> Creating $INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
chown "$SERVICE_USER:$SERVICE_USER" "$INSTALL_DIR"

# Initialize secrets file if it doesn't exist
SECRETS_FILE="$INSTALL_DIR/secrets.json"
if [[ ! -f "$SECRETS_FILE" ]]; then
    echo "==> Initializing secrets file"
    sudo -u "$SERVICE_USER" bash -c "echo '{}' > '$SECRETS_FILE'"
    chmod 600 "$SECRETS_FILE"
else
    echo "==> Secrets file already exists"
fi

# Copy .env template if .env doesn't exist
ENV_FILE="$INSTALL_DIR/.env"
if [[ ! -f "$ENV_FILE" ]]; then
    echo "==> Creating .env template"
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    if [[ -f "$SCRIPT_DIR/.env.example" ]]; then
        cp "$SCRIPT_DIR/.env.example" "$ENV_FILE"
    else
        cat > "$ENV_FILE" << 'EOF'
# Winter configuration
WINTER_PDS_URL=https://your-pds.example.com
WINTER_HANDLE=winter.your-pds.example.com
WINTER_APP_PASSWORD=your-app-password-here

# Claude API token (required for daemon)
CLAUDE_CODE_OAUTH_TOKEN=your-claude-token-here

# Web UI URL (for approval links in DMs)
WINTER_WEB_URL=https://winter.your-domain.com

# Operator DID (required for bootstrap)
WINTER_OPERATOR_DID=did:plc:your-operator-did-here
EOF
    fi
    chown "$SERVICE_USER:$SERVICE_USER" "$ENV_FILE"
    chmod 600 "$ENV_FILE"
    echo "    Please edit $ENV_FILE with your credentials"
else
    echo "==> Environment file already exists"
fi

# Install service files
echo "==> Installing systemd service files"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cp "$SCRIPT_DIR/winter.service" /etc/systemd/system/
cp "$SCRIPT_DIR/winter-web.service" /etc/systemd/system/

# Reload systemd
echo "==> Reloading systemd"
systemctl daemon-reload

echo ""
echo "==> Installation complete!"
echo ""
echo "Next steps:"
echo "  1. Edit configuration: sudo vim $ENV_FILE"
echo "  2. Bootstrap identity: sudo -u $SERVICE_USER winter bootstrap ..."
echo "  3. Enable services:    sudo systemctl enable winter-web winter"
echo "  4. Start services:     sudo systemctl start winter-web && sudo systemctl start winter"
echo "  5. Check status:       sudo systemctl status winter-web winter"
echo "  6. View logs:          journalctl -u winter -u winter-web -f"
