# Fennec — Design Spec

Date: 2026-04-03

---

## 1. Identity & Core Philosophy

**Language:** Rust (edition 2024). Single binary, edge-deployable, <5MB RAM target.

**Core Principles:**
- **Search first, solve second** — before reasoning from scratch, check if the collective already has the answer (inspired by Plurum)
- **Experiences over facts** — capture structured problem-solving journeys (goal, attempts, dead ends, solution, gotchas), not just raw data
- **Local-first** — the agent works fully offline; the collective is additive, never required
- **Simplicity over features** — complexity kills agents; keep the core readable and lean
- **Dead ends are first-class data** — "I tried X and it failed because Y" is as valuable as "do X"

**Two Deliverables:**
1. **The Agent** — personal AI agent with full tooling, multi-provider LLM support, multi-channel messaging, hybrid memory system, and collective intelligence client
2. **The Protocol** — the open contract between agent and server (experience schema, API endpoints, trust rules) so anyone can build compatible agents or servers

The collective server is **Plurum** (plurum.ai) — an existing platform tailored to fit our needs. Not rebuilt from scratch.

**Business Model:** OSS agent, OSS protocol. Plurum hosts the collective infrastructure (free tier with rate limits, paid tier for heavy use).

---

## 2. Architecture Overview

```
┌─────────────────────────────────────────────────┐
│                    Channels                      │
│  CLI · Telegram · Discord · Slack · WhatsApp ... │
└──────────────────────┬──────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────┐
│                    Gateway                       │
│  WebSocket control plane · HTTP API · Auth       │
└──────────────────────┬──────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────┐
│                  Agent Loop                      │
│  1. Prompt guard                                 │
│  2. Classify intent                              │
│  3. Search collective                            │
│  4. Recall personal memory                       │
│  5. Build context                                │
│  6. LLM call (multi-provider)                    │
│  7. Tool execution (parallel where safe)         │
│  8. Loop detection circuit breaker               │
│  9. Post-turn: consolidate + log experience      │
└───────┬──────────┬──────────┬───────────────────┘
        │          │          │
   ┌────▼───┐ ┌───▼────┐ ┌──▼──────────────┐
   │ Memory │ │ Tools  │ │ Collective      │
   │        │ │        │ │ Client          │
   │ SQLite │ │ Shell  │ │                 │
   │ brain  │ │ Files  │ │ Search          │
   │ hybrid │ │ Web    │ │ Publish         │
   │ search │ │ Browser│ │ Report outcomes │
   │ decay  │ │ MCP    │ │ Cache locally   │
   │ consol.│ │ ...    │ │                 │
   └────────┘ └────────┘ └──────┬──────────┘
                                │
                    ┌───────────▼──────────┐
                    │  Collective Server   │
                    │                      │
                    │  Experience store     │
                    │  Search index         │
                    │  Trust scoring        │
                    │  Poison detection     │
                    │  Protocol API         │
                    └──────────────────────┘
```

**Design principle:** Every major subsystem is a trait. Provider, Memory, Tool, Channel, CollectiveLayer — all swappable via configuration, not recompilation.

---

## 3. Memory System

### Tier 1: Personal Memory (local brain.db)

SQLite database at `~/.fennec/memory/brain.db`.

**Schema:**

`memories` table:
- `id`, `key`, `content`, `category`, `embedding` (BLOB), `importance`, `created_at`, `updated_at`, `session_id`, `namespace`, `superseded_by`

`memories_fts` — FTS5 virtual table with BM25 scoring, kept in sync via triggers (after insert, delete, update).

`embedding_cache` — stores embedding vectors keyed by content hash with LRU access-time tracking.

`experiences` table:
- `id`, `goal`, `context` (JSON), `attempts` (JSON), `solution`, `gotchas` (JSON), `tags` (JSON), `confidence`, `session_id`, `created_at`

`experiences_fts` — FTS5 over goal + solution + gotchas text.

**SQLite tuning:** WAL mode, `synchronous = NORMAL`, 8MB mmap, 2MB page cache, temp tables in memory.

**Memory Categories:**
- `Core` — long-term facts, preferences, decisions. Never time-decayed. Evergreen.
- `Daily` — per-session conversation summaries. 7-day half-life decay.
- `Conversation` — turn-level context. Ephemeral.

**Retrieval:** Every `recall()` call merges vector cosine similarity (default weight 0.7) with BM25 keyword score (default weight 0.3). Embeddings generated via a pluggable `EmbeddingProvider` trait. Top 5 relevant memories injected into system prompt.

**Time Decay:** Exponential: `score * 2^(-age_days / half_life)`. Default half-life 7 days. `Core` entries exempt.

**LLM-Driven Consolidation:** After each turn, fire-and-forget extraction:
1. Summarize conversation to `Daily`
2. Extract durable facts/preferences/decisions to `Core` (if any)
3. If a task just completed — distill into a structured `Experience`

**Soul Snapshot:** Export all `Core` memories to `MEMORY_SNAPSHOT.md` as human-readable Markdown. Auto-hydrate on cold boot if brain.db is missing. Survives disk wipes and machine migrations.

### Tier 2: Collective Cache (local)

When the agent finds useful experiences from the collective server, they're written to a `collective_cache` table in brain.db:
- Same schema as `experiences` plus: `source_server`, `original_id`, `trust_score`, `outcome_reports`
- Searched alongside local experiences but ranked lower by default
- Trust decays if no positive outcome reports within 30 days

**Search Order (every turn):**
1. Local experiences — fast, fully trusted
2. Local collective cache — fast, medium trust
3. Remote collective server — network call, only if local results are low confidence

The agent gets faster over time as frequently useful collective experiences become local.

---

## 4. Experience System & Collective Protocol

### Experience Schema

```rust
Experience {
    id: UUID,
    goal: String,                  // what the agent was trying to accomplish
    context: ExperienceContext {
        tools_used: Vec<String>,
        environment: String,       // OS, language, framework
        constraints: String,       // what made this hard
    },
    attempts: Vec<Attempt> {
        action: String,            // what was tried
        outcome: String,           // what happened
        dead_end: bool,            // did this fail?
        insight: String,           // why it failed or worked
    },
    solution: Option<String>,      // what ultimately worked (None if unsolved)
    gotchas: Vec<String>,          // non-obvious things discovered
    tags: Vec<String>,             // searchable labels
    confidence: f32,               // 0.0-1.0, self-assessed
    created_at: DateTime,
    session_id: UUID,              // link to full session transcript
}
```

**When created:** Only for non-trivial tasks. After a task ends, the LLM runs one extraction pass to decide if the experience is worth capturing and distills it.

**Target size:** 500-1000 tokens per experience. Compact enough to inject into prompts without blowing context budget. Full session transcript available locally via `session_id` if deeper detail needed.

**Scrubbing before collective publish:**
- API keys, tokens, passwords
- File paths containing usernames or org names
- IP addresses, hostnames, database URLs
- Customer/user data
- Anything user marked as private

### Collective Protocol

**API Endpoints:**

```
POST   /experiences              Publish a scrubbed experience
GET    /search?q=...&limit=5     Semantic + keyword search
GET    /experiences/{id}         Fetch full experience details
POST   /experiences/{id}/outcome Report success or failure
GET    /health                   Server status
```

**Search Response (compact by default):**

```json
{
    "results": [
        {
            "id": "...",
            "goal": "...",
            "solution": "...",
            "gotchas": ["..."],
            "trust_score": 0.82,
            "relevance_score": 0.91,
            "outcome_reports": { "success": 14, "failure": 2 }
        }
    ]
}
```

Returns goal, solution, gotchas, and scores. Full attempts/context fetched separately only if needed.

**Trust Scoring:**
- New experience starts at 0.5
- Success report: +0.05 (capped at 0.95)
- Failure report: -0.10 (floor at 0.05)
- 3+ failure reports with 0 successes: quarantined, excluded from search
- No outcome reports in 90 days: gradual decay toward 0.3

**Auth:** API key per agent. Rate limited per key. Free tier with reasonable limits, paid tier for heavy use.

---

## 5. Agent Loop

```
message arrives
  │
  ├─ 1. PROMPT GUARD
  │     Compiled regex scan for injection patterns
  │     Patterns: system prompt override, role confusion, tool-call
  │     injection, jailbreak, secret extraction
  │     Returns: Safe / Suspicious(score) / Blocked
  │     If Blocked: reject, log, notify
  │
  ├─ 2. CLASSIFY INTENT
  │     Keyword + heuristic classification
  │     Returns hint for model routing
  │     Simple chat → cheap model, complex task → strong model
  │
  ├─ 3. SEARCH COLLECTIVE (the differentiator)
  │     Extract intent from user message
  │     Search order: local experiences → collective cache → remote server
  │     High-confidence match (>0.85): inject as primary context
  │     Partial match (0.5-0.85): inject as "related experience"
  │     No match: proceed from scratch, flag for experience capture
  │
  ├─ 4. RECALL PERSONAL MEMORY
  │     Hybrid search (vector 0.7 + BM25 0.3) with time decay
  │     Top 5 relevant memories injected into system prompt
  │
  ├─ 5. BUILD CONTEXT
  │     System prompt (frozen at session start for prompt cache):
  │       identity + core memories + tool schemas + channel guidance + datetime
  │     Per-turn injection (not in system prompt, avoids cache invalidation):
  │       collective results + recalled daily memories + experience matches
  │
  ├─ 6. LLM CALL
  │     Multi-provider via ReliableProvider (retry + failover)
  │     Streaming with idle timeout detection
  │     Auth rotation with cooldown on rate limits
  │
  ├─ 7. TOOL EXECUTION
  │     Parallel where safe (read-only tools via tokio::join!)
  │     Sequential for exclusive tools (shell, file write)
  │     Credential scrubbing on all tool outputs
  │     Approval gating for medium/high risk operations
  │
  ├─ 8. LOOP DETECTION
  │     Sliding window of 20 recent tool calls
  │     Detect: exact repeats, ping-pong, no-progress
  │     Escalate: Warning → Block → Break
  │
  ├─ 9. LOOP (back to 6 if tool calls pending, max 15 iterations)
  │
  └─ 10. POST-TURN
        ├─ Consolidate to personal memory (fire-and-forget)
        ├─ If task completed: LLM extraction pass → Experience
        ├─ If experience extracted + sharing enabled: scrub → publish
        └─ Context compression if approaching token budget
```

**Context Compression:** Triggers at 50% context window fill. Summarizes middle block of conversation using a cheap model, preserving first few and last few messages. Tool result truncation attempted first as lighter alternative.

---

## 6. Security

### Layer 1: Prompt Injection Guard
- Compiled regex scan on all inbound messages (fast, no allocation per call)
- Patterns: system prompt override, role confusion, tool-call injection, jailbreak, secret extraction
- Also scans collective experiences on ingest — poisoned collective content is a novel attack vector
- Returns Safe / Suspicious(score) / Blocked

### Layer 2: Secret Management
- ChaCha20-Poly1305 encrypted secret store
- Key file at `~/.fennec/.secret_key` with 0600 permissions
- Config stores only `enc:<hex(nonce||ciphertext||tag)>`
- No plaintext API keys in config files

### Layer 3: Tool Execution Safety
- Command allowlist (default safe set: git, ls, cat, grep, cargo, npm, node, python, etc.)
- Forbidden paths (/etc, /root, ~/.ssh, ~/.aws, ~/.gnupg, ~/.config)
- Path traversal blocking (reject `..` and absolute paths outside workspace)
- Timeout with process kill (60s default, 600s max)
- Credential scrubbing on all tool outputs before LLM context
- Approval gating for medium/high risk operations

### Layer 4: Sandbox
- OS-level sandboxing as compile-time features:
  - Landlock (Linux)
  - Bubblewrap (Linux, optional)
  - macOS Seatbelt (sandbox-exec)
- Docker sandbox option for full isolation (read-only root, no network, all caps dropped)

### Layer 5: Collective Safety
- Scrub validation on ingest (reject experiences containing patterns that look like secrets)
- Prompt injection scan on all incoming collective content
- Trust tiers: local (1.0) > collective cache with positive outcomes (0.5-0.95) > new collective (0.3)
- Quarantine: 3+ failure reports with 0 successes → excluded from search
- Rate limiting per API key
- DM pairing on messaging channels — unknown senders get a challenge code

---

## 7. Providers

### LLM Provider Trait

```rust
trait Provider {
    async fn chat(&self, messages, tools) -> ChatResponse;
    async fn chat_stream(&self, messages, tools) -> Stream<StreamEvent>;
    fn supports_tool_calling(&self) -> bool;
    fn supports_thinking(&self) -> bool;
    fn context_window(&self) -> usize;
}
```

**Day-one providers:**
- Anthropic (Claude) — native SDK, prompt caching support
- OpenAI — native SDK
- Google Gemini
- OpenRouter — access to dozens more via one integration
- Ollama — local models, no API key needed

**ReliableProvider wrapper:** Retry with backoff, credential rotation on rate limits, failover to next provider in chain. Idle timeout detection on streams.

**Model routing:** Classify intent → route to cheap model (chat) or strong model (complex tasks). Configurable per channel.

Implementation details will reference Hermes and OpenClaw codebases for proven patterns.

---

## 8. Channels

### Channel Trait

```rust
trait Channel {
    async fn start(&self, bus: MessageBus);
    async fn send(&self, message: OutboundMessage);
    async fn send_stream(&self, delta: StreamDelta);  // optional
    fn allows_sender(&self, sender_id: &str) -> bool;
}
```

**Day-one channels:**
- CLI (always, primary dev interface)
- Telegram
- Discord
- Slack
- WhatsApp
- Signal
- Email

Each channel enforces its own `allow_from` list. DM pairing on all messaging channels. Streaming support opt-in per channel.

**Gateway:** Single process serves all channels, routes to isolated agent sessions. WebSocket control plane for real-time communication. HTTP API for programmatic access.

Implementation details will reference Hermes and OpenClaw codebases for proven patterns.

---

## 9. Tools

### Tool Trait

```rust
trait Tool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> JsonSchema;
    fn is_read_only(&self) -> bool;       // safe for parallel execution
    fn is_exclusive(&self) -> bool;       // must run alone
    fn risk_level(&self) -> RiskLevel;    // Low / Medium / High
    async fn execute(&self, args: Value) -> ToolResult;
}
```

**Core tools:**
- Shell execution (with allowlist + sandbox)
- File read / write / edit / glob / grep
- Web fetch / search
- Browser automation
- MCP client (connect to external tool servers)
- Memory tools (recall, store, search history)
- Collective tools (search collective, report outcome)
- Subagent spawning (delegate subtasks)
- Cron scheduling
- Skills (markdown-based, agent can create new ones)

**Parallel execution:** Read-only tools run concurrently via `tokio::join!`. Exclusive tools (shell, file write) run alone. Heuristic decides per-turn.

**Credential scrubbing:** Compiled regex on every tool output before context.

Implementation details will reference all four studied codebases for proven patterns.

---

## 10. Collective Server — Plurum

The collective server is **Plurum** (plurum.ai), an existing platform built for AI agent collective intelligence. We use Plurum rather than building a custom server — it already provides hybrid search (pgvector + FTS with Reciprocal Rank Fusion), outcome reporting, quality scoring (Wilson Lower Bound), auth (API key + JWT), rate limiting, real-time pulse (WebSocket + inbox), and SDKs (Python, TypeScript, MCP).

**Stack:** Python/FastAPI, PostgreSQL on Supabase, pgvector for embeddings (OpenAI text-embedding-3-small, 1536 dims), HNSW index.

**What our agent uses from Plurum:**

| Plurum Endpoint | Our Agent Uses It For |
|---|---|
| `POST /experiences` | Publish scrubbed experience after task completion |
| `POST /experiences/search` | Search collective (step 3 of agent loop) |
| `GET /experiences/{id}` | Fetch full experience when compact result needs more detail |
| `POST /experiences/{id}/outcome` | Report success/failure after using a collective experience |
| `GET /pulse/inbox` | Check for relevant new experiences (heartbeat routine) |

**Tailoring needed (done separately on the Plurum side):**
1. Schema changes: add `attempts` JSONB, `solution` TEXT, `tags` TEXT[], `confidence` FLOAT
2. Simplify `gotchas` from `{warning, context}` objects to plain strings
3. Restructure `context` from free-form TEXT to structured JSONB `{tools_used, environment, constraints}`
4. Rename `quality_score` → `trust_score` across API responses
5. Add scrub validation on ingest (reject experiences containing secret patterns)
6. Add prompt injection scan on submitted content
7. Add quarantine logic (exclude experiences with 3+ failures, 0 successes from search)
8. Add 90-day trust decay on experiences with no outcome reports

**Our agent's collective client talks to Plurum's API.** The `CollectiveLayer` trait in the agent abstracts this — if Plurum is ever replaced or supplemented, only the trait implementation changes. The agent loop doesn't know or care what's behind the trait.

**Business model:** OSS agent, OSS protocol. Plurum hosts the collective infrastructure (free tier with rate limits, paid tier for heavy use).

---

## 11. Key Differentiators vs. Competitors

| | OpenClaw | Hermes | NanoBot | ZeroClaw | Ours |
|---|---|---|---|---|---|
| Memory | Pluggable, no built-in | HRR + 7 backends | Flat markdown | SQLite hybrid | SQLite hybrid + experiences |
| Collective | None | None | None | None | Core feature |
| Dead ends | Lost | Lost | Lost | Lost | First-class data |
| First action | Think | Think | Think | Think | Search collective |
| Performance | Node.js | Python + numpy | Python | Rust <5MB | Rust <5MB |
| Secret storage | SecretRef system | Plaintext + redaction | Plaintext | ChaCha20 encrypted | ChaCha20 encrypted |
| Loop detection | None | None | None | Circuit breaker | Circuit breaker |

---

## 12. Lineage & Inspiration

| Component | Primary Inspiration |
|---|---|
| Storage engine + search + time decay | ZeroClaw |
| Pluggable trait design | ZeroClaw + Hermes |
| Prompt cache preservation | Hermes |
| Experience model | Plurum |
| Collective cache (local caching of remote experiences) | Original |
| Search-first agent loop | Original (inspired by Plurum philosophy) |
| Security layers 1-4 | ZeroClaw |
| Security layer 5 (collective safety) | Original |
| Loop detection circuit breaker | ZeroClaw |
| Trust scoring model | Plurum (adapted) |
| Soul snapshots | ZeroClaw |
| Channel/provider architecture | Hermes + OpenClaw |
