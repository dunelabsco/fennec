#!/usr/bin/env bash
set -euo pipefail

REPO="https://github.com/dunelabsco/fennec.git"
INSTALL_DIR="$HOME/.local/bin"
FENNEC_HOME="$HOME/.fennec"
BOLD='\033[1m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

print_banner() {
    echo -e "${BOLD}"
    echo "  ╔═══════════════════════════════════════╗"
    echo "  ║           FENNEC INSTALLER             ║"
    echo "  ║  The fastest AI agent with collective  ║"
    echo "  ║           intelligence                 ║"
    echo "  ╚═══════════════════════════════════════╝"
    echo -e "${NC}"
}

log()   { echo -e "${GREEN}[fennec]${NC} $1"; }
warn()  { echo -e "${YELLOW}[fennec]${NC} $1"; }
error() { echo -e "${RED}[fennec]${NC} $1"; exit 1; }

install_system_deps() {
    log "Checking system dependencies..."

    local need_install=false

    for cmd in git gcc make; do
        if ! command -v "$cmd" &>/dev/null; then
            need_install=true
            break
        fi
    done

    if [ "$need_install" = true ]; then
        log "Installing build tools..."
        if command -v apt-get &>/dev/null; then
            sudo apt-get update -qq
            sudo apt-get install -y -qq build-essential pkg-config libssl-dev git curl
        elif command -v yum &>/dev/null; then
            sudo yum install -y gcc gcc-c++ make openssl-devel pkg-config git curl
        elif command -v dnf &>/dev/null; then
            sudo dnf install -y gcc gcc-c++ make openssl-devel pkg-config git curl
        elif command -v pacman &>/dev/null; then
            sudo pacman -Sy --noconfirm base-devel openssl git curl
        elif command -v apk &>/dev/null; then
            sudo apk add build-base openssl-dev pkgconfig git curl
        else
            error "Could not detect package manager. Please install: git, gcc, make, pkg-config, libssl-dev"
        fi
        log "System dependencies installed."
    else
        log "System dependencies already present."
    fi
}

check_deps() {
    install_system_deps

    if ! command -v cargo &>/dev/null; then
        warn "Rust not found. Installing via rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "$HOME/.cargo/env"
        log "Rust installed: $(rustc --version)"
    else
        log "Rust found: $(rustc --version)"
    fi
}

build_fennec() {
    if [ -x "$INSTALL_DIR/fennec" ]; then
        log "Fennec binary already exists at $INSTALL_DIR/fennec — skipping build."
        return
    fi

    local build_dir
    build_dir=$(mktemp -d)
    log "Cloning Fennec into $build_dir..."
    git clone --depth=1 "$REPO" "$build_dir/fennec"
    cd "$build_dir/fennec"

    log "Building release binary (this may take a minute)..."
    cargo build --release

    mkdir -p "$INSTALL_DIR"
    cp target/release/fennec "$INSTALL_DIR/fennec"
    chmod +x "$INSTALL_DIR/fennec"
    log "Installed to $INSTALL_DIR/fennec"

    # Cleanup
    rm -rf "$build_dir"
}

setup_config() {
    if [ -f "$FENNEC_HOME/config.toml" ]; then
        log "Config already exists at $FENNEC_HOME/config.toml — skipping."
        return
    fi

    # Use the TUI setup wizard built into the fennec binary
    log "Running interactive setup wizard..."
    "$INSTALL_DIR/fennec" onboard
}

ensure_path() {
    if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
        export PATH="$INSTALL_DIR:$PATH"
        # Add to shell profile
        local shell_rc=""
        if [ -f "$HOME/.zshrc" ]; then
            shell_rc="$HOME/.zshrc"
        elif [ -f "$HOME/.bashrc" ]; then
            shell_rc="$HOME/.bashrc"
        else
            shell_rc="$HOME/.bashrc"
        fi
        if ! grep -q "$INSTALL_DIR" "$shell_rc" 2>/dev/null; then
            echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$shell_rc"
            log "Added $INSTALL_DIR to PATH in $shell_rc"
        fi
    fi
}

setup_systemd() {
    # Only set up systemd if running as root on Linux with systemctl
    if [ "$(id -u)" != "0" ] || ! command -v systemctl &>/dev/null; then
        return
    fi

    echo ""
    echo "Start Fennec as a background service? (Y/n)"
    read -rp "> " start_service < /dev/tty
    if [[ "${start_service:-y}" =~ ^[Nn] ]]; then
        return
    fi

    cat > /etc/systemd/system/fennec.service << SVCEOF
[Unit]
Description=Fennec AI Agent
After=network.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/fennec gateway
Restart=always
RestartSec=5
Environment=FENNEC_HOME=$FENNEC_HOME
Environment=PATH=$INSTALL_DIR:/usr/local/bin:/usr/bin:/bin
WorkingDirectory=$HOME

[Install]
WantedBy=multi-user.target
SVCEOF

    systemctl daemon-reload
    systemctl enable fennec
    systemctl start fennec
    log "Fennec service started! It will survive reboots."
    log "View logs: journalctl -u fennec -f"
}

print_done() {
    echo ""
    echo -e "${GREEN}${BOLD}Fennec is installed and running!${NC}"
    echo ""
    echo "  Commands:"
    echo "    fennec status              # Check status"
    echo "    fennec agent               # Interactive chat"
    echo "    fennec agent -m 'Hello'    # Single message"
    echo "    fennec gateway             # Start all channels (foreground)"
    echo "    fennec onboard --force     # Re-run setup wizard"
    echo ""
    echo "  Config:  $FENNEC_HOME/config.toml"
    echo "  Logs:    journalctl -u fennec -f"
    echo ""
}

main() {
    print_banner
    check_deps
    build_fennec
    setup_config
    ensure_path
    setup_systemd
    print_done
}

main "$@"
