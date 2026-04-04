# Fennec Plan 3: Channels & Gateway

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multi-channel messaging (Telegram, Discord, Slack) with a message bus, Axum HTTP/WebSocket gateway, DM pairing security, streaming responses, cron scheduling, and a `fennec gateway` command that serves all channels from a single process.

**Architecture:** Following ZeroClaw's pattern — hand-roll platform clients with `reqwest` + `tokio-tungstenite` (no heavy SDKs). Single `mpsc` message bus decouples channels from the agent. Supervised channel tasks with restart backoff. DM pairing with code-based auth. Draft-edit streaming for platforms that support message editing.

**Tech Stack:** axum 0.8 (already a dep), tokio-tungstenite for Discord WebSocket, lettre for email sending.

**Current state:** 152 tests, 3.8MB binary. Only CLI channel exists. Channel trait is defined but minimal.

**Reference:** ZeroClaw hand-rolls all platform clients. Hermes/NanoBot use python-telegram-bot and discord.py. OpenClaw uses grammy + @buape/carbon. We follow ZeroClaw's approach for minimal deps.

---

## File Structure (new/modified files only)

```
src/
├── bus/
│   ├── mod.rs               # CREATE: MessageBus (mpsc inbound + outbound)
│   └── events.rs            # CREATE: InboundMessage, OutboundMessage
├── channels/
│   ├── traits.rs            # MODIFY: add streaming support to Channel trait
│   ├── cli.rs               # MODIFY: adapt to MessageBus pattern
│   ├── telegram.rs          # CREATE: Telegram Bot API (long-poll)
│   ├── discord.rs           # CREATE: Discord Gateway WebSocket
│   ├── slack.rs             # CREATE: Slack Web API
│   └── manager.rs           # CREATE: ChannelManager (supervisor, dispatch)
├── gateway/
│   ├── mod.rs               # CREATE: Axum HTTP/WebSocket gateway
│   ├── auth.rs              # CREATE: Gateway auth (token, pairing)
│   └── routes.rs            # CREATE: API routes (/chat, /status, /health)
├── security/
│   └── pairing.rs           # CREATE: DM pairing code system
├── cron/
│   ├── mod.rs               # CREATE: Cron scheduler
│   └── jobs.rs              # CREATE: Job storage + execution
├── main.rs                  # MODIFY: add gateway command
```

---

### Task 1: Message Bus

**Files:**
- Create: `src/bus/mod.rs`
- Create: `src/bus/events.rs`
- Modify: `src/lib.rs`
- Test: `tests/bus_test.rs`

- [ ] **Step 1: Define bus event types**

`src/bus/events.rs`: `InboundMessage` { id, sender, content, channel, chat_id, timestamp, reply_to: Option, metadata: HashMap<String,String> }. `OutboundMessage` { content, channel, chat_id, reply_to: Option, metadata: HashMap }.

- [ ] **Step 2: Implement MessageBus**

`src/bus/mod.rs`: `MessageBus` with `inbound_tx/rx: mpsc::channel<InboundMessage>(100)` and `outbound_tx/rx: mpsc::channel<OutboundMessage>(100)`. Methods: `publish_inbound()`, `publish_outbound()`, `subscribe_inbound() -> Receiver`, `subscribe_outbound() -> Receiver`. Use `Arc` wrappers so bus can be cloned to multiple channels.

- [ ] **Step 3: Tests**

Test: publish and receive inbound, publish and receive outbound, multiple producers.

- [ ] **Step 4: Commit**

---

### Task 2: Channel Manager + Adapt CLI

**Files:**
- Create: `src/channels/manager.rs`
- Modify: `src/channels/traits.rs`
- Modify: `src/channels/cli.rs`
- Modify: `src/channels/mod.rs`
- Test: `tests/channel_manager_test.rs`

- [ ] **Step 1: Extend Channel trait for streaming**

Add to `Channel` trait:
```rust
fn supports_streaming(&self) -> bool { false }
async fn send_streaming_start(&self, _chat_id: &str) -> anyhow::Result<Option<String>> { Ok(None) }
async fn send_streaming_delta(&self, _chat_id: &str, _message_id: &str, _full_text: &str) -> anyhow::Result<()> { Ok(()) }
async fn send_streaming_end(&self, _chat_id: &str, _message_id: &str, _full_text: &str) -> anyhow::Result<()> { Ok(()) }
fn allows_sender(&self, _sender_id: &str) -> bool { true }
```

- [ ] **Step 2: Implement ChannelManager**

`src/channels/manager.rs`: `ChannelManager` holds `Vec<Arc<dyn Channel>>`, a `MessageBus`, and a map of `channel_name -> Arc<dyn Channel>`. Methods:
- `start_all()`: spawn each channel's `listen()` as a supervised task. On crash, restart with backoff (1s, 2s, 4s, max 60s, max 10 restarts).
- `dispatch_outbound()`: consume from bus outbound queue, route to correct channel by name.

- [ ] **Step 3: Adapt CliChannel to use MessageBus**

CLI channel publishes `InboundMessage` to bus instead of raw `ChannelMessage`. Keep backward compat with the existing mpsc pattern too (for single-channel mode).

- [ ] **Step 4: Tests + Commit**

---

### Task 3: DM Pairing System

**Files:**
- Create: `src/security/pairing.rs`
- Modify: `src/security/mod.rs`
- Test: `tests/pairing_test.rs`

- [ ] **Step 1: Implement PairingGuard**

`src/security/pairing.rs`: `PairingGuard` with:
- `generate_code() -> String`: 6-digit numeric code via CSPRNG rejection sampling
- `verify_code(input: &str) -> Result<String>`: constant-time compare, on success generate a `fc_<64-hex>` bearer token, store SHA-256 hash, return token
- `is_authorized(token: &str) -> bool`: hash incoming token, check against stored hashes
- `add_allowed_user(user_id: &str)`: add to persistent allowlist
- `is_allowed_user(user_id: &str) -> bool`: check allowlist (supports `"*"` wildcard)
- Brute-force lockout: 5 failed attempts → 5-minute lockout per client_id
- Persist allowlist to JSON file at `~/.fennec/pairing/<channel>.json`

- [ ] **Step 2: Tests**

Test: code generation is 6 digits, verification succeeds/fails, lockout after 5 failures, allowlist persistence, wildcard.

- [ ] **Step 3: Commit**

---

### Task 4: Telegram Channel

**Files:**
- Create: `src/channels/telegram.rs`
- Test: `tests/telegram_test.rs`

- [ ] **Step 1: Implement TelegramChannel**

Hand-roll using `reqwest` (following ZeroClaw's pattern — no teloxide/grammy):
- `TelegramChannel` { bot_token, client, allowed_users, pairing_guard: Option }
- `new(bot_token, allowed_users, pairing_guard)`
- `listen()`: long-poll via `getUpdates?timeout=30&offset={last_update_id+1}`. For each update with a message, check `allows_sender()`, create `InboundMessage`, send to bus.
- `send()`: POST to `sendMessage` API with `chat_id` and `text` (markdown parse mode)
- `send_streaming_start()`: send initial message, return message_id
- `send_streaming_delta()`: `editMessageText` (rate-limited: max 1 edit per 300ms per chat)
- `send_streaming_end()`: final `editMessageText`
- Bot API base: `https://api.telegram.org/bot{token}/`

- [ ] **Step 2: Tests**

Test: message parsing from Telegram JSON format, allowed user check, rate limit on edits. No live API calls.

- [ ] **Step 3: Commit**

---

### Task 5: Discord Channel

**Files:**
- Create: `src/channels/discord.rs`
- Add dep: `tokio-tungstenite` with `rustls-tls-native-roots` feature
- Test: `tests/discord_test.rs`

- [ ] **Step 1: Implement DiscordChannel**

Hand-roll using `tokio-tungstenite` for Gateway + `reqwest` for REST (ZeroClaw pattern):
- `DiscordChannel` { bot_token, client, allowed_users, pairing_guard: Option }
- `listen()`:
  1. GET `https://discord.com/api/v10/gateway/bot` → get `url`
  2. Connect WebSocket to `{url}?v=10&encoding=json`
  3. Receive Hello (op:10) → extract heartbeat_interval
  4. Send Identify (op:2) with token and intents (GUILD_MESSAGES + MESSAGE_CONTENT + DIRECT_MESSAGES)
  5. Enter select! loop: heartbeat timer tick → send op:1, ws message → handle dispatch
  6. On MESSAGE_CREATE: check `allows_sender()`, skip bot messages, create InboundMessage
  7. Handle op:7 (Reconnect) and op:9 (Invalid Session) with resume/re-identify
- `send()`: POST to `https://discord.com/api/v10/channels/{channel_id}/messages`
- `send_streaming_start()`: send initial message, return message_id
- `send_streaming_delta()`: PATCH `channels/{channel_id}/messages/{message_id}` (rate-limited)
- `send_streaming_end()`: final PATCH

- [ ] **Step 2: Tests**

Test: Gateway payload parsing, message creation, heartbeat logic. No live API calls.

- [ ] **Step 3: Commit**

---

### Task 6: Slack Channel

**Files:**
- Create: `src/channels/slack.rs`
- Test: `tests/slack_test.rs`

- [ ] **Step 1: Implement SlackChannel**

Using Slack Socket Mode (WebSocket) + Web API:
- `SlackChannel` { bot_token, app_token, client, allowed_users }
- `listen()`:
  1. POST `https://slack.com/api/apps.connections.open` with app_token → get WebSocket URL
  2. Connect WebSocket
  3. On `events_api` envelope with `event.type == "message"`: check sender, create InboundMessage
  4. Send `{"envelope_id": "...", "payload": {}}` acknowledgement for each envelope
- `send()`: POST `https://slack.com/api/chat.postMessage` with bot_token
- `send_streaming_start()`: post initial message, return ts
- `send_streaming_delta()`: `chat.update` with ts
- `send_streaming_end()`: final `chat.update`

- [ ] **Step 2: Tests + Commit**

---

### Task 7: Axum Gateway

**Files:**
- Create: `src/gateway/mod.rs`
- Create: `src/gateway/auth.rs`
- Create: `src/gateway/routes.rs`
- Modify: `src/lib.rs`
- Test: `tests/gateway_test.rs`

- [ ] **Step 1: Implement gateway auth**

`src/gateway/auth.rs`: `GatewayAuth` with token-based auth. Validates `Authorization: Bearer <token>` header. Supports pairing via `POST /pair` endpoint.

- [ ] **Step 2: Implement gateway routes**

`src/gateway/routes.rs`:
- `GET /health` → `{"status": "ok"}`
- `GET /status` → `{"version": "...", "channels": [...], "uptime": ...}`
- `POST /chat` → accept `{"message": "...", "channel": "api"}`, run agent turn, return response
- `GET /ws` → WebSocket upgrade for real-time streaming

- [ ] **Step 3: Implement gateway server**

`src/gateway/mod.rs`: `GatewayServer` with `run(addr, agent, auth)`. Build axum Router with routes, layer auth middleware, serve with `axum::serve`.

- [ ] **Step 4: Tests + Commit**

---

### Task 8: Cron Scheduler

**Files:**
- Create: `src/cron/mod.rs`
- Create: `src/cron/jobs.rs`
- Modify: `src/lib.rs`
- Test: `tests/cron_test.rs`

- [ ] **Step 1: Implement cron job storage**

`src/cron/jobs.rs`: `CronJob` { id, name, schedule: String, command: String, enabled: bool, last_run: Option, next_run: Option }. `JobStore` backed by a JSON file at `~/.fennec/cron/jobs.json`. Methods: `add_job`, `remove_job`, `list_jobs`, `load`, `save`.

- [ ] **Step 2: Implement cron scheduler**

`src/cron/mod.rs`: `CronScheduler` that runs a tokio timer loop. Each tick: check jobs, find any where `next_run <= now`, execute by sending the command as an InboundMessage to the bus (or directly to the agent). Parse schedule strings: support `every <N>m/h/d` (simple interval) and standard cron expressions (via simple parser or the `cron` crate).

- [ ] **Step 3: Tests + Commit**

---

### Task 9: Gateway Command + Integration

**Files:**
- Modify: `src/main.rs`
- Modify: `src/config/schema.rs`

- [ ] **Step 1: Add channel configs**

To `FennecConfig`, add:
```rust
pub channels: ChannelsConfig  // telegram, discord, slack configs
pub gateway: GatewayConfig     // host, port, auth
pub cron: CronConfig           // enabled, jobs file path
```

- [ ] **Step 2: Add `fennec gateway` command**

New CLI subcommand that:
1. Loads config
2. Creates all configured channels (Telegram, Discord, Slack based on which have tokens)
3. Creates MessageBus
4. Creates ChannelManager, starts all channels
5. Creates Agent
6. Starts GatewayServer (HTTP/WS)
7. Starts CronScheduler if enabled
8. Main loop: consume inbound messages, run agent turns, publish outbound
9. Graceful shutdown on SIGINT/SIGTERM

- [ ] **Step 3: Run full test suite + release build**

```bash
source "$HOME/.cargo/env" && cargo test && cargo build --release && ls -lh target/release/fennec
```

- [ ] **Step 4: Commit**

---

## What Plan 3 Delivers

- **Message bus** decoupling channels from agent
- **3 messaging channels**: Telegram, Discord, Slack (hand-rolled, no heavy SDKs)
- **DM pairing** with brute-force lockout
- **Draft-edit streaming** for Telegram and Discord
- **Axum HTTP/WS gateway** for programmatic access
- **Cron scheduler** for periodic tasks
- **`fennec gateway`** command to serve all channels from one process
- **Channel supervisor** with restart backoff

## What's Next (Plan 4)

- Collective integration (Plurum client)
- Search-first in agent loop
- Outcome reporting
- Collective cache
