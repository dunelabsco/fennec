# Architecture

Internal design reference. For the user-facing overview, see
[`README.md`](README.md).

## Stack

| Concern | Choice | Why |
|---|---|---|
| Language | Rust 2024 (MSRV 1.87) | Single binary, memory safety, predictable resource use under long-running agent workloads. |
| Async runtime | Tokio 1.50, multi-threaded | Standard for production-grade Rust async. |
| Web framework | Axum 0.8 + Tower HTTP | Tower middleware stack for timeouts, body caps, CORS. |
| DB | SQLite via `rusqlite` (bundled) | Zero-ops persistence. WAL mode + FTS5 + memory-mapped I/O. |
| Crypto | ChaCha20-Poly1305 (AEAD), SHA-256 (digests, PKCE), `subtle` (constant-time compares) | AEAD for at-rest secrets; constant-time for auth tokens. |
| HTTP client | `reqwest` 0.12 with rustls-tls | No system OpenSSL dependency. |
| CLI | `clap` 4.5 derive | Standard. |
| Serialization | `serde`, `serde_json`, `serde_yaml_ng` (maintained fork) | YAML fork because upstream `serde_yaml` is unmaintained. |
| Sync primitives | `parking_lot::Mutex`, `tokio::sync::Mutex` | parking_lot: no poisoning, faster path; tokio for async-aware locking. |

Release profile: LTO=fat, single codegen unit, stripped, panic=abort.

## Top-level layout

```
src/
├── agent/        Core agent loop, tool orchestration, system-prompt building
├── auth/         Anthropic OAuth (PKCE) flow
├── bus/          In-process message bus (mpsc channels)
├── channels/     Messaging platform implementations
├── collective/   Plurum client, scrub, cache, search (the differentiator)
├── config/       FennecConfig schema + TOML loading
├── cron/         Job scheduler with cron expressions
├── doctor/       Self-diagnostic checks
├── gateway/      Axum HTTP server
├── heartbeat/    Periodic health probes
├── memory/       SQLite, embeddings, FTS5, consolidation, snapshot
├── mcp/          MCP client (stdio + HTTP transports)
├── onboard/      Interactive setup wizard
├── providers/    LLM abstractions + reliable failover wrapper
├── security/     SecretStore, prompt_guard, path/url sandbox, ct, pairing
├── sessions/     Per-user session state
├── skills/       Skill loader (markdown + YAML frontmatter)
└── tools/        ~27 built-in tools + MCP tool bridge

skills/           76 shipped skill markdown files
scripts/          install.sh, update.sh
tests/            33 integration tests
.github/          CI workflows
```

## Data flow — a single inbound message

```
                ┌──────────────────────────────────────┐
                │          Channel (e.g. Slack)        │
                │   (listener task, owned by Manager)  │
                └─────────────────┬────────────────────┘
                                  │
                                  │ InboundMessage
                                  ▼
                ┌──────────────────────────────────────┐
                │         MessageBus (mpsc)            │
                └─────────────────┬────────────────────┘
                                  │
                                  ▼
              ┌────────────────────────────────────────┐
              │       Gateway agent loop               │
              │  - acquire agent lock                  │
              │  - run agent.turn(msg)                 │
              │  - drop lock before publish_outbound   │
              └─────────────────┬──────────────────────┘
                                │
                                ▼
                      ┌─────────────────┐
                      │   Agent.turn    │
                      └────────┬────────┘
                               │
        ┌──────────────────────┼─────────────────────────────┐
        ▼                      ▼                             ▼
  Prompt-guard         Collective search              Memory recall
  scan input           (local + cache + remote)       (FTS5 + vector)
                       Inject ranked context          Inject relevant facts
                               │
                               ▼
                       Provider.chat() ──► tool calls? ──► execute
                               │           (loop until    (history grows)
                               ▼            no more tool
                        Final assistant     calls)
                        text response
                               │
                               ▼
                       OutboundMessage on bus
                               │
                               ▼
                  ChannelManager.dispatch_outbound
                  (find channel, send via vendor API)
```

Lock release order matters: `agent_lock` is held only for the LLM turn body
itself. Streaming delivery, bus publish, and channel send all run without the
lock so concurrent gateway `/chat` requests aren't serialized through inbound
publish. See `src/main.rs::run_gateway` for the actual implementation.

## Modules in detail

### `agent/`

- `agent.rs` — `Agent` struct holding history, provider, tool registry,
  consolidation hooks, max-tool-iterations cap.
- `compressor.rs` — context compression when history exceeds budget.
- `loop_.rs` — `LoopDetector` (window-based exact-repeat / ping-pong /
  no-progress detection). **Defined and tested but not currently wired into
  the agent's tool loop**; the audit's "wire it or remove it" guidance was
  intentionally deferred because wiring would change capability.
- `scrub.rs` — output-side scrubbing for agent responses before they leave
  the agent.
- `thinking.rs` — `ThinkingLevel` parsing (`/think:<level>` directive) and
  per-provider parameter translation.

### `bus/`

`MessageBus` is `Clone`-able and holds two `mpsc::Sender`s
(`inbound_tx`, `outbound_tx`). The receivers live on `BusReceiver`
which the gateway holds. Anyone can publish; only the gateway consumes.

`turn_context.rs` defines `TurnOrigin` (`channel + chat_id`),
`PendingReplies` (registry for `ask_user`-style synchronous prompts that
need a reply on the same channel), and `ChatDirectory` (channel-to-home-id
mapping for `send_message` without a bound origin).

A future architectural refactor (deferred) would split `MessageBus` into
explicit `Publisher` / `Subscriber` halves with `Weak` senders so the
inbound channel actually closes when listeners exit.

### `channels/`

Six implementations behind a single `Channel` trait
(`channels/traits.rs`). Each implements:
- `name() -> &str` — stable identifier (`"telegram"`, `"slack"`, etc.)
- `start_listening(...)` — spawn the listener task; returns a `JoinHandle`
- `send(...)` — send a final message
- `send_typing(...)` — typing indicator
- `supports_streaming()` + `send_streaming_start/delta/end()` — for
  partial-message edits on platforms that support them
- `send_recording_indicator(...)` — voice-channels analog

`ChannelManager` wraps a `Vec<Arc<dyn Channel>>` plus a bus handle. It owns
the listener handles and the outbound dispatch task, so shutdown can abort
both.

### `collective/`

The differentiator vs Hermes / OpenClaw / generic agent CLIs.

- `traits.rs` — `CollectiveLayer` interface (`search`, `get_experience`,
  `publish`, `report_outcome`, `health_check`).
- `plurum.rs` — Plurum API client. HTTPS-only, `connect_timeout`,
  classified 429 / 503 errors with `Retry-After`, fallible constructor.
- `scrub.rs` — pre-publish redaction of secrets (API keys, tokens, PEM
  blocks, JWTs) and PII (emails, phones, SSNs). Run on every outbound to
  the collective AND on the inputs to the consolidation LLM, so the
  pattern set is the single source of truth for "what we never reveal."
- `cache.rs` — local LRU cache of fetched collective results with TTL.
- `search.rs` — orchestrates local-memory hits + cache hits + remote
  Plurum hits, dedups by normalized goal, drops `Suspicious` and `Blocked`
  remote results via `prompt_guard`.
- `mock.rs` — testing-only `CollectiveLayer` that stores experiences in
  a `parking_lot::Mutex<Vec<Experience>>`.

Search confidence is bucketed into `High` (top result > 0.7), `Partial`
(> 0.3), `None`. Buckets gate how aggressively the agent injects
collective context into the system prompt.

### `memory/`

- `sqlite.rs` — `SqliteMemory`. Schema: `memories`, `experiences`,
  `embedding_cache`. WAL mode, FTS5 index, configurable mmap size and
  cache size. Embedding cache enforces an LRU cap (`cache_max`) with
  `accessed_at`-based eviction.
- `embedding.rs` — `OpenAIEmbedding` (ada-002) and `NoopEmbedding`
  (returns a zero vector — for local-only mode).
- `vector.rs` — cosine similarity, hybrid merge (`vector_score * α +
  fts5_bm25 * (1-α)`).
- `fts.rs` — FTS5 setup, query escaping.
- `consolidation.rs` — daily summaries + core-fact extraction. Scrubs
  both input messages and LLM output before storing.
- `snapshot.rs` — atomic export/hydrate of the Core memory category.
  Tempfile + `rename(2)` to prevent zero-byte corruption on crash. Entry
  delimiter is an HTML-comment sentinel rather than `\n## ` to avoid
  splitting on header characters that occur naturally in content.
- `decay.rs` — time-based score decay for relevance ranking.
- `experience.rs` — `Experience` struct shared with `collective/`.
- `traits.rs` — `Memory` trait.

### `providers/`

- `traits.rs` — `Provider` trait with `chat`, `chat_stream`, `name`.
- `anthropic.rs` — full SSE parsing, OAuth bearer support, extended-thinking
  budget tokens, content-block-state machine.
- `openai.rs` — chunked transfer, `reasoning_effort` for o1 family,
  shared OpenAI-shaped surface used by Kimi/Moonshot, OpenRouter, custom
  endpoints.
- `ollama.rs` — local LLM, ND-JSON streaming.
- `openrouter.rs` — thin wrapper around `OpenAIProvider`.
- `reliable.rs` — failover wrapper. Keeps a list of providers in priority
  order; on rate-limit (429) or other classified errors, applies a
  per-provider cooldown and rolls to the next; jittered exponential
  backoff; overall deadline (default 60s).
- `sse.rs` — server-sent events parser shared between providers.
- `thinking.rs` — `apply_thinking_params` for cross-provider translation.

### `security/`

- `secrets.rs` — `SecretStore`. ChaCha20-Poly1305 AEAD. 32-byte key in
  `~/.fennec/.key` (mode 0600), generated on first run via
  `write_secure` (atomic, mode-restricted). Master key zeroized on drop.
- `prompt_guard.rs` — RegexSet against category-tagged pattern library
  (jailbreak, role confusion, instruction override, etc.).
  `ScanResult::{Safe, Suspicious(categories, score), Blocked(reason)}`.
  Action mode is `Warn` / `Block` / `Sanitize`.
- `path_sandbox.rs` — canonicalization + whitelist enforcement.
  Symlink-aware (walks each component to detect escape).
- `url_guard.rs` — blocks private IP ranges (RFC 1918, link-local,
  localhost, IPv6 ULA), accepts an optional whitelist, rejects
  non-`http`/`https` schemes by default.
- `fs.rs` — `write_secure` (tempfile-then-rename + mode 0600 atomic
  writer), `create_dir_private` (0700 on creation).
- `ct.rs` — constant-time bearer/token comparison via `subtle`.
- `pairing.rs` — multi-device pairing protocol; persists token hashes
  and failed-attempt counters; resets only after a successful pair.
  Currently lightweight; planned to expand with the multi-instance work.

### `gateway/`

Axum server. Endpoints:

- `GET /health` — public, returns `{"status":"ok"}`.
- `GET /status` — public, returns version + uptime.
- `POST /chat` — protected by `Authorization: Bearer <token>` (constant-time
  compared). Body cap 1 MiB. Per-request timeout 600s via `TimeoutLayer`.

Errors are wrapped in a `ErrorResponse` envelope with a `request_id` that's
also logged so log-spelunking can match a 500 to a backend trace without
leaking the trace text to the caller.

### `mcp/`

- `client.rs` — `McpClient`. Initialize handshake, tool-list discovery,
  oversized-description truncation, namespaced-tool-name composition.
- `transport.rs` — `StdioTransport` (subprocess with `kill_on_drop`,
  stderr piped to a logger task) and `HttpTransport` (reqwest, fallible
  constructor).
- `types.rs` — wire shapes (`McpToolSpec`, `McpContent`, `McpToolResult`).

Tool names are namespaced (`mcp_<server>_<tool>`) to prevent shadowing of
built-ins by hostile or compromised servers.

### `cron/`

`CronScheduler` parses cron expressions (`every 5m`, `every 1h`, etc.),
persists jobs to a SQLite store, and runs them with backoff on failure.
Originating channel is captured (`origin_channel`, `origin_chat_id`) so a
job created on Telegram delivers its result back to Telegram.

### `doctor/`

Self-diagnostic command. Each check returns `Pass` / `Warn` / `Fail` with
a message. Categories:

1. Config: file present, parses, required fields populated
2. Provider: API key valid, endpoint reachable
3. Memory: DB schema migrated, FTS5 index queryable
4. Plurum: connectivity, token valid (if configured)
5. Skills: load all without errors
6. Channels: token shapes valid, no duplicate home_chat_ids
7. Gateway: bind address available
8. Filesystem: `~/.fennec` permissions tight (0700, 0600 on key files)

## Tools

| Tool | File | Purpose |
|---|---|---|
| `list_dir` / `read_file` / `write_file` | `tools/files.rs` | Path-sandbox-gated file ops |
| `shell` | `tools/shell.rs` | Local subprocess with timeout + output cap |
| `web_fetch` / `web_search` | `tools/web.rs` | URL fetch (URL-guard gated) and search |
| `http_request` | `tools/http_request_tool.rs` | Generic HTTP (URL-guard gated) |
| `vision` | `tools/vision_tool.rs` | Image analysis via vision provider |
| `screenshot` | `tools/screenshot_tool.rs` | OS screenshot capture |
| `image_info` | `tools/image_info_tool.rs` | Image metadata extraction |
| `voice` (TTS + transcribe) | `tools/voice_tool.rs` | OpenAI TTS, Whisper transcription |
| `code_exec` | `tools/code_exec_tool.rs` | Python / JS subprocess with timeout |
| `image_gen` | `tools/image_gen_tool.rs` | DALL-E 3 |
| `memory_store` / `memory_recall` / `memory_forget` | `tools/memory_tools.rs` | Durable memory |
| `collective_search` / `collective_publish` / `collective_get_experience` / `collective_report` | `tools/collective_tools.rs` | Peer-learning network |
| `cronjob` | `tools/cron_tool.rs` | Schedule recurring tasks |
| `todo` | `tools/todo_tool.rs` | In-session task tracking |
| `git` | `tools/git_tool.rs` | Git command wrappers |
| `delegate` | `tools/delegate_tool.rs` | Spawn sub-agent with restricted tool set |
| `pdf_read` | `tools/pdf_read_tool.rs` | PDF text extraction |
| `ask_user` | `tools/ask_user_tool.rs` | Synchronous user prompt routed to origin channel |
| `browser` | `tools/browser_tool.rs` | Headless browser |
| `mcp` (dynamic) | `tools/mcp_tools.rs` | MCP server tool bridge |
| `skills` | `tools/skills_tool.rs` | List available skills |
| `claude_code_cli` | `tools/claude_code_cli_tool.rs` | Spawn local `claude` CLI |
| `session` | `tools/session_tools.rs` | Session state |
| `weather` | `tools/weather_tool.rs` | Weather API |
| `calc` | `tools/calc_tool.rs` | Expression evaluation |
| `send_message` | `tools/send_message_tool.rs` | Route messages between channels |

Total: ~27 built-ins + an unbounded number loaded from MCP servers.

## Security posture

The April 2026 audit drove a set of hardening PRs grouped G1-G10. Each
group is a single PR cut against `main`:

- **G1** — `send_message_tool` and `ask_user_tool` race fix. Replaced
  ad-hoc `channel.listen()` calls with a `PendingReplies` registry; added
  channel allowlisting via `target='channel:chat_id'`.
- **G2** — Provider stream correctness. `handle_sse_event` returns a
  terminator flag; `chat_stream` got the same per-provider retry loop
  `chat()` already had.
- **G3** — Gateway hardening. `TimeoutLayer` (600s), `RequestBodyLimitLayer`
  (1 MiB), `ErrorResponse` envelope with `request_id`, graceful shutdown
  via `tokio::sync::watch`.
- **G4** — Auth hygiene. `key_bytes: Zeroizing<[u8; 32]>`,
  `paired_token_hashes` and `failed_attempts` persisted, OAuth `state`
  validation (CSRF), `fs2` advisory lock around refresh.
- **G5** — Channel resilience round 2. UTF-8-safe Telegram split,
  Slack `ratelimited` 200-status path, empty-streaming-end guards.
- **G7** — Memory + collective round 2. Snapshot atomic write,
  consolidation input scrub, embedding cache LRU cap, `spawn_blocking`
  for SQLite calls in `store()`, `Suspicious` results dropped from
  collective search, dedup by normalized goal, mock `parking_lot::Mutex`.
- **G8** — Channel-token decryption. Every channel secret routed through
  `SecretStore::decrypt` with the same fail-closed pattern as
  `provider.api_key`.
- **G9** — Bus + shutdown lifecycle. SIGTERM + SIGINT handler, abort
  listener and dispatch handles on shutdown, drop `agent_lock` before
  `bus.publish_outbound`.
- **G10** — Mid-tier cleanup. `PlurumlClient::new` returns `Result`,
  adds `connect_timeout`, `https_only`; classifies 429/503 +
  `Retry-After`; same fix for `HttpTransport::new`; cron poisoned-mutex
  recovery; full UUID job IDs.

(G6 was scoped, evaluated, and skipped — see commit history.)

## Configuration shape

`FennecConfig` (`src/config/`) is the in-memory shape; `~/.fennec/config.toml`
is the on-disk serialization. Top-level sections, in declaration order:

```toml
[identity]
[provider]
[memory]
[security]
[agent]
[channels.telegram]
[channels.discord]
[channels.slack]
[channels.whatsapp]
[channels.email]
[gateway]
[cron]
[collective]
```

Each `enc2:`-prefixed string is decrypted through `SecretStore` at load
time. Plaintext values pass through; a `tracing::warn!` is logged once per
plaintext-passthrough call so operators can spot legacy configs and
migrate.

## Where to read next

- `src/main.rs::run_gateway` — the actual lifecycle of a Fennec server.
- `src/agent/agent.rs::Agent::turn` — the tool-call loop.
- `src/collective/scrub.rs` — the pattern set the agent will never reveal.
- `src/security/secrets.rs` — secrets at rest.
- `tests/` — the integration test suite is the most accurate spec.
