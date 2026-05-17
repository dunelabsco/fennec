//! Lifecycle callbacks an agent run can fan out to a frontend.
//!
//! The TUI (F1) and the dashboard (F2 — future) both consume the
//! same set of events: streaming text, tool start / progress /
//! complete, reasoning deltas, and human-in-the-loop prompts
//! (approval / clarify / secret). This trait lets a frontend
//! subscribe to those events without modifying the agent loop's
//! per-callsite logic.
//!
//! Channels (Telegram, Discord, etc.) DO NOT use this surface —
//! they receive only the final assistant message, the same as
//! today. Callbacks are an *additional* layer for frontends that
//! want intra-turn visibility.
//!
//! Default-impl methods on the trait make every event optional:
//! a frontend can implement only the ones it cares about. The
//! agent loop fires every event regardless; consumers ignore what
//! they don't need.

use std::sync::Arc;

use async_trait::async_trait;

/// A live tool execution in progress.
#[derive(Debug, Clone, Default)]
pub struct ToolStart {
    /// Stable identifier for matching `start` ↔ `progress` ↔ `complete`.
    pub tool_id: String,
    /// Tool name (e.g. `"weather"`, `"shell"`, `"memory.recall"`).
    pub name: String,
    /// One-line preview of the call (e.g. arg summary).
    pub preview: String,
    /// Raw tool arguments — carried so observers (sub-agent
    /// tracker, file-tracker) can extract structured fields like
    /// `path` without re-parsing the preview string. `Null` when
    /// the caller doesn't have / want to surface the args.
    pub args: serde_json::Value,
}

/// Tool progress update — emitted between start and complete for
/// long-running tools that surface incremental state. Most tools
/// don't emit this; tools that take more than a couple of seconds
/// (shell commands, http downloads) should.
#[derive(Debug, Clone)]
pub struct ToolProgress {
    pub tool_id: String,
    /// Updated preview (e.g. "downloaded 4/12 MB").
    pub preview: String,
}

/// Tool execution finished.
#[derive(Debug, Clone)]
pub struct ToolComplete {
    pub tool_id: String,
    pub name: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// `Some(message)` on error; `None` on success.
    pub error: Option<String>,
    /// Optional one-line summary of the result. Frontends use this
    /// for the inline tool-call collapsed display.
    pub summary: Option<String>,
}

/// A request that requires user input mid-turn. The agent suspends
/// until the frontend resolves the request (or times out per the
/// frontend's policy).
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Human-readable command being requested (e.g. `rm -rf build/`).
    pub command: String,
    /// Why the agent wants to run it.
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ClarifyRequest {
    /// The question the agent wants the user to answer.
    pub question: String,
    /// Optional multiple-choice options. Empty means free-text.
    pub options: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SecretRequest {
    /// What's being asked for (e.g. "GitHub personal access token").
    pub label: String,
}

/// A delegated sub-agent has been spawned. `id` is locally
/// unique within a session; `parent_id` is `None` for roots
/// spawned by the main agent and `Some(id)` for nested
/// delegations. `goal` is the task description the parent
/// passed via the `delegate` tool.
#[derive(Debug, Clone)]
pub struct SubagentSpawn {
    pub id: String,
    pub parent_id: Option<String>,
    pub goal: String,
    /// 0-based depth in the spawn tree. Root sub-agents have
    /// depth = 0; nested delegations carry parent.depth + 1.
    pub depth: u32,
    /// 0-based ordering index among siblings sharing the same
    /// `parent_id`. Drives sort-stable rendering even when network
    /// reordering shuffles the live event stream.
    pub index: u32,
    /// Provider model the sub-agent will run against (e.g.
    /// `"claude-haiku-4-5"`). `None` when the sub-agent inherits
    /// the parent's model and we don't bother to materialise it.
    pub model: Option<String>,
    /// Toolset bundles the sub-agent was granted at spawn time.
    /// Empty when the sub-agent inherits the parent's tools.
    pub toolsets: Vec<String>,
}

/// A structured entry in the sub-agent's "output tail" — the last
/// few tool calls and their previews, captured at complete time so
/// archived snapshots can show what happened even after the live
/// tool stream is gone.
#[derive(Debug, Clone)]
pub struct SubagentOutputEntry {
    pub tool: String,
    pub preview: String,
    pub is_error: bool,
}

/// A delegated sub-agent has finished. Carries enough metadata
/// for the TUI to roll up subtree metrics and surface the
/// outcome in the spawn-tree overlay.
#[derive(Debug, Clone, Default)]
pub struct SubagentComplete {
    pub id: String,
    /// Final assistant text the subagent returned.
    pub output: String,
    /// `true` when the subagent's turn returned Ok and the
    /// agent didn't bail (max iterations, provider error, etc.).
    pub success: bool,
    pub duration_ms: u64,
    /// Names of tools the subagent invoked.
    pub tools_used: Vec<String>,
    /// Token usage from the sub-agent's provider response.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// USD cost rolled up across the sub-agent's API calls. 0.0
    /// when pricing for the model isn't known.
    pub cost_usd: f64,
    /// Files the sub-agent read / wrote during its turn (tracked
    /// best-effort by tool wrappers — see `tools::read_file` /
    /// `tools::write_file`).
    pub files_read: Vec<String>,
    pub files_written: Vec<String>,
    /// Structured tail of the last N tool calls + their previews.
    /// Used by the overlay's detail pane as a fallback when the
    /// live tools list is empty (archived snapshots).
    pub output_tail: Vec<SubagentOutputEntry>,
    /// Iteration count (number of provider turns the sub-agent
    /// completed). Useful for "agent went 12 rounds" intuition.
    pub iteration: u32,
    /// API call count (provider HTTP requests). Distinct from
    /// `iteration` because retries + tool calls can issue multiple
    /// API requests per iteration.
    pub api_calls: u32,
}

/// Lifecycle hooks an agent run fires for each event of interest
/// to a real-time frontend.
///
/// All methods have default no-op implementations so a frontend
/// only overrides the ones it consumes. The agent loop calls them
/// directly; they're synchronous and should not block — frontends
/// that need to do real work (write to a UI, etc.) should send a
/// message to their own event loop and return immediately.
///
/// The blocking-prompt methods (`on_approval_request`,
/// `on_clarify_request`, `on_secret_request`) are the exception:
/// they DO block the agent until the user responds. Frontends
/// implement them by suspending their event loop, presenting a
/// modal, and returning the response. A return of `None` means
/// "user cancelled / declined" and the agent should treat that
/// as a deny.
#[async_trait]
pub trait AgentCallbacks: Send + Sync {
    /// Streaming text delta from the assistant. Called many times
    /// per turn; concatenating all deltas in order yields the
    /// final assistant message.
    fn on_text_delta(&self, _delta: &str) {}

    /// Streaming reasoning delta (extended thinking, e.g.
    /// Claude 4.x thinking blocks). Same shape as text deltas.
    fn on_reasoning_delta(&self, _delta: &str) {}

    /// Tool started executing.
    fn on_tool_start(&self, _start: ToolStart) {}

    /// Tool progress update.
    fn on_tool_progress(&self, _progress: ToolProgress) {}

    /// Tool finished.
    fn on_tool_complete(&self, _complete: ToolComplete) {}

    /// Status update for the active turn (e.g. "calling provider",
    /// "compressing context"). Transient — frontend usually shows
    /// it in a status bar that auto-clears after a few seconds.
    fn on_status(&self, _message: &str) {}

    /// Turn started. `prompt` is the user message that initiated
    /// the turn.
    fn on_turn_start(&self, _prompt: &str) {}

    /// Turn ended. `summary` is the final assistant message.
    fn on_turn_complete(&self, _summary: &str) {}

    /// Async: ask the user to approve a privileged action.
    /// Default impl auto-approves so callbacks-less code paths
    /// behave exactly as they did before this trait existed.
    /// Frontends that present a modal `await` until the user
    /// resolves it.
    async fn on_approval_request(&self, _request: ApprovalRequest) -> bool {
        true
    }

    /// Async: ask the user a clarifying question. Default
    /// returns `None` so callers fall back to whatever they did
    /// before.
    async fn on_clarify_request(&self, _request: ClarifyRequest) -> Option<String> {
        None
    }

    /// Async: ask the user for a secret (e.g. an API token).
    async fn on_secret_request(&self, _request: SecretRequest) -> Option<String> {
        None
    }

    /// A new sub-agent was spawned (via the `delegate` tool).
    /// Fires once per spawn, before any sub-agent text /
    /// tool-call events arrive.
    fn on_subagent_spawn(&self, _spawn: SubagentSpawn) {}

    /// The sub-agent's main loop began executing.
    fn on_subagent_start(&self, _id: &str) {}

    /// Streaming text delta from a sub-agent. Routes the
    /// equivalent of [`Self::on_text_delta`] but tagged with
    /// the subagent_id so the frontend can attribute it.
    fn on_subagent_text(&self, _id: &str, _delta: &str) {}

    /// Streaming reasoning delta from a sub-agent. Counterpart
    /// to [`Self::on_reasoning_delta`].
    fn on_subagent_thinking(&self, _id: &str, _delta: &str) {}

    /// A tool started executing inside a sub-agent.
    fn on_subagent_tool(&self, _id: &str, _start: ToolStart) {}

    /// Free-text progress note from a sub-agent (the agent's
    /// own status messages, e.g. "compressing context").
    fn on_subagent_progress(&self, _id: &str, _note: &str) {}

    /// A sub-agent finished — terminal event for that subtree
    /// branch.
    fn on_subagent_complete(&self, _complete: SubagentComplete) {}
}

/// No-op implementation. Used in code paths that don't have a
/// frontend (existing channel sends, batch runs, etc.) so the
/// agent can call callback methods unconditionally.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoCallbacks;

#[async_trait]
impl AgentCallbacks for NoCallbacks {}

/// Owned, type-erased handle so an `Agent` can hold a callbacks
/// implementation without leaking the concrete type into its
/// signature.
pub type CallbacksHandle = Arc<dyn AgentCallbacks>;

/// Convenience: the default no-op handle, useful in tests and in
/// code paths that explicitly want "no frontend".
pub fn noop_callbacks() -> CallbacksHandle {
    Arc::new(NoCallbacks)
}
