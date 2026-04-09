#!/usr/bin/env bash
set -euo pipefail

GREEN='\033[0;32m'
NC='\033[0m'
log() { echo -e "${GREEN}[fennec]${NC} $1"; }

INSTALL_DIR="$HOME/.local/bin"

# Stop service if running
if command -v systemctl &>/dev/null && systemctl is-active fennec &>/dev/null 2>&1; then
    log "Stopping fennec service..."
    systemctl stop fennec
fi
# Also kill any stray fennec processes
pkill -f "fennec gateway" 2>/dev/null || true
pkill -f "fennec agent" 2>/dev/null || true
sleep 1

# Build
log "Pulling latest code..."
cd /tmp && rm -rf fennec
git clone --depth=1 https://github.com/dunelabsco/fennec.git
cd fennec

log "Building..."
source "$HOME/.cargo/env"
cargo build --release 2>&1 | tail -3

log "Installing..."
cp target/release/fennec "$INSTALL_DIR/fennec"
cd ~ && rm -rf /tmp/fennec

# Restart service if it was running
if command -v systemctl &>/dev/null && systemctl is-enabled fennec &>/dev/null 2>&1; then
    log "Restarting fennec service..."
    systemctl start fennec
fi

log "Updated! $(fennec status 2>&1 | head -1)"
