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
    mkdir -p "$FENNEC_HOME"
    mkdir -p "$FENNEC_HOME/memory"
    mkdir -p "$FENNEC_HOME/cron"
    mkdir -p "$FENNEC_HOME/pairing"

    if [ -f "$FENNEC_HOME/config.toml" ]; then
        log "Config already exists at $FENNEC_HOME/config.toml — skipping."
        return
    fi

    echo ""
    echo -e "${BOLD}Quick Setup${NC}"
    echo ""

    # Provider selection
    echo "Which LLM provider? (default: anthropic)"
    echo "  1) anthropic (Claude)"
    echo "  2) openai (GPT-4o)"
    echo "  3) kimi (Moonshot)"
    echo "  4) openrouter (any model)"
    echo "  5) ollama (local)"
    read -rp "> " provider_choice < /dev/tty
    provider_base_url=""
    case "${provider_choice:-1}" in
        1) provider="anthropic"; model="claude-sonnet-4-20250514"; key_env="ANTHROPIC_API_KEY" ;;
        2) provider="openai"; model="gpt-4o"; key_env="OPENAI_API_KEY" ;;
        3) provider="kimi"; model="moonshot-v1-128k"; key_env="KIMI_API_KEY"; provider_base_url="" ;;
        4) provider="openrouter"; model="anthropic/claude-sonnet-4"; key_env="OPENROUTER_API_KEY"; provider_base_url="https://openrouter.ai/api/v1" ;;
        5) provider="ollama"; model="llama3.1"; key_env="" ;;
        *) provider="anthropic"; model="claude-sonnet-4-20250514"; key_env="ANTHROPIC_API_KEY" ;;
    esac

    # API key
    api_key=""
    if [ -n "$key_env" ]; then
        if [ -n "${!key_env:-}" ]; then
            api_key="${!key_env}"
            log "Using $key_env from environment."
        else
            read -rp "Enter your API key (or press Enter to set later): " api_key < /dev/tty
        fi
    fi

    # Agent name
    read -rp "Agent name (default: Fennec): " agent_name < /dev/tty
    agent_name="${agent_name:-Fennec}"

    # Telegram setup
    echo ""
    echo "Set up Telegram channel? (y/N)"
    echo "  (Create a bot via @BotFather on Telegram to get a token)"
    read -rp "> " enable_telegram < /dev/tty
    telegram_token=""
    if [[ "${enable_telegram:-n}" =~ ^[Yy] ]]; then
        read -rp "Enter your Telegram bot token: " telegram_token < /dev/tty
    fi

    # Collective (Plurum)
    echo ""
    echo "Enable collective intelligence via Plurum? (y/N)"
    read -rp "> " enable_collective < /dev/tty
    plurum_key=""
    if [[ "${enable_collective:-n}" =~ ^[Yy] ]]; then
        if [ -n "${PLURUM_API_KEY:-}" ]; then
            plurum_key="$PLURUM_API_KEY"
            log "Using existing PLURUM_API_KEY from environment."
        else
            log "Registering with Plurum..."
            local register_response=""
            local agent_username
            agent_username=$(echo "$agent_name" | tr '[:upper:]' '[:lower:]' | tr ' ' '-' | tr -cd 'a-z0-9-')
            agent_username="${agent_username}-$(head -c 4 /dev/urandom | od -An -tx1 | tr -d ' \n')"
            register_response=$(curl -s --max-time 10 -X POST "https://api.plurum.ai/api/v1/agents/register" \
                -H "Content-Type: application/json" \
                -d "{\"name\": \"$agent_name\", \"username\": \"$agent_username\"}" 2>/dev/null) || register_response=""

            if [ -n "$register_response" ]; then
                plurum_key=$(echo "$register_response" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('api_key',''))" 2>/dev/null) || plurum_key=""
            fi

            if [ -n "$plurum_key" ]; then
                log "Registered with Plurum! Your agent is now part of the collective."
                echo ""
                echo -e "  ${YELLOW}Save this key — it won't be shown again:${NC}"
                echo "  $plurum_key"
                echo ""
            else
                warn "Could not auto-register with Plurum (server may be unreachable)."
                warn "You can set collective.api_key in ~/.fennec/config.toml later."
                read -rp "Or paste a Plurum API key now (Enter to skip): " plurum_key < /dev/tty
            fi
        fi
    fi

    # Write config
    cat > "$FENNEC_HOME/config.toml" << TOML
[identity]
name = "$agent_name"
persona = "A fast, helpful AI assistant with collective intelligence."

[provider]
name = "$provider"
model = "$model"
api_key = "$api_key"
base_url = "$provider_base_url"
temperature = 0.7
max_tokens = 8192

[memory]
vector_weight = 0.7
keyword_weight = 0.3
half_life_days = 7.0
consolidation_enabled = true

[security]
prompt_guard_action = "warn"
prompt_guard_sensitivity = 0.7
encrypt_secrets = true
command_timeout_secs = 60

[agent]
max_tool_iterations = 15
context_window = 200000

[channels.telegram]
enabled = $([ -n "$telegram_token" ] && echo "true" || echo "false")
token = "$telegram_token"

[channels.discord]
enabled = false
token = ""

[channels.slack]
enabled = false
bot_token = ""
app_token = ""

[gateway]
host = "127.0.0.1"
port = 8990

[cron]
enabled = false

[collective]
enabled = $([ -n "$plurum_key" ] && echo "true" || echo "false")
api_key = "$plurum_key"
base_url = "https://api.plurum.ai"
publish_enabled = true
search_enabled = true
TOML

    log "Config written to $FENNEC_HOME/config.toml"
}

ensure_path() {
    if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
        warn "$INSTALL_DIR is not in your PATH."
        echo ""
        echo "Add this to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
        echo ""
        echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
        echo ""
    fi
}

print_done() {
    echo ""
    echo -e "${GREEN}${BOLD}Fennec is installed!${NC}"
    echo ""
    echo "  Quick start:"
    echo "    fennec status              # Check it's working"
    echo "    fennec agent               # Interactive chat"
    echo "    fennec agent -m 'Hello'    # Single message"
    echo "    fennec gateway             # Start all channels"
    echo ""
    echo "  Config: $FENNEC_HOME/config.toml"
    echo "  Memory: $FENNEC_HOME/memory/brain.db"
    echo ""
    echo "  To add channels (Telegram, Discord, Slack):"
    echo "    Edit $FENNEC_HOME/config.toml and set tokens"
    echo ""
}

main() {
    print_banner
    check_deps
    build_fennec
    setup_config
    ensure_path
    print_done
}

main "$@"
