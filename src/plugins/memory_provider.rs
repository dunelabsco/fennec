//! Pluggable memory provider — augmentation layer on top of the
//! always-on built-in [`Memory`](crate::memory::traits::Memory) store.
//!
//! # Two memories run side by side
//!
//! Fennec's built-in [`SqliteMemory`](crate::memory::sqlite::SqliteMemory)
//! is always active. It stores literal facts the agent decides to
//! remember, FTS5-indexes them, and exposes the
//! `memory_recall` / `memory_store` / `memory_forget` tools.
//!
//! Optionally, **one** external [`MemoryProvider`] runs alongside it.
//! The external provider does something the built-in store doesn't —
//! for example: dialectic user modeling, semantic clustering, hosted
//! cross-session recall, etc. It augments rather than replaces:
//!
//! - The user's local SQLite data is untouched. `memory_recall` keeps
//!   working. No migration question, no data loss.
//! - The external provider gets called at lifecycle points
//!   (initialize / prefetch / sync_turn / shutdown) and can contribute
//!   formatted context, observe writes, surface its own tools.
//!
//! Only one external runs at a time. This is a hard constraint —
//! letting two providers fight for tool schemas in the LLM's context
//! produces confused tool selection.
//!
//! # Lifecycle
//!
//! Called by the [`MemoryManager`](super::memory_manager::MemoryManager)
//! at well-defined agent lifecycle points:
//!
//! | Method | When |
//! |---|---|
//! | `is_available()` | At agent build, to decide whether to activate this provider |
//! | `initialize(session_id)` | Once per session |
//! | `system_prompt_block()` | At system prompt assembly |
//! | `prefetch(query)` | Before each LLM call, returns formatted context |
//! | `sync_turn(user, assistant)` | After each turn |
//! | `get_tool_schemas()` | At system prompt assembly, schemas merged into the agent's tool list |
//! | `handle_tool_call(name, args)` | When the LLM calls one of the provider's tools |
//! | `shutdown()` | At session end (`clear_history`) |
//!
//! # Trust and namespacing
//!
//! Provider tool names live in their own namespace. The
//! `MemoryManager` rejects collisions with built-in tools at
//! agent-build time. A provider can never overwrite the built-in
//! `memory_recall` / `memory_store` / `memory_forget` tools.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::providers::traits::ChatMessage;
use crate::tools::traits::ToolSpec;

/// One pluggable memory provider. Implementations come from bundled
/// plugins (Rust impls) or WASM plugins (the host wraps wasm
/// exports as a [`MemoryProvider`]).
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    /// Short identifier used in config and diagnostics.
    fn name(&self) -> &str;

    /// Whether this provider is configured and ready to activate.
    /// Called at agent build BEFORE `initialize` to gate
    /// activation. Should not make network calls — check config and
    /// installed deps only.
    fn is_available(&self) -> bool;

    /// Per-session setup. Called once when the agent starts a
    /// session. Implementations may create resources, open
    /// connections, start background threads.
    async fn initialize(&self, ctx: &MemoryProviderContext) -> Result<()>;

    /// Static text injected into the agent's system prompt. Returns
    /// empty string for providers that don't need static guidance.
    /// Called once per session prompt build.
    fn system_prompt_block(&self) -> String {
        String::new()
    }

    /// Recall additional context for the upcoming turn. Returns
    /// formatted text suitable for direct inclusion in the agent's
    /// context. Empty string means "nothing relevant."
    ///
    /// Called once per turn, before the LLM call. Slow providers
    /// can serialise the call (it runs synchronously within the
    /// turn) — they should keep latency in mind.
    async fn prefetch(&self, query: &str) -> Result<String>;

    /// Observe a completed turn. Both messages are passed so the
    /// provider can update its internal model.
    ///
    /// Called once per turn, after the assistant response is
    /// finalised. Errors are logged and swallowed — provider
    /// failures must not abort the agent's turn.
    async fn sync_turn(
        &self,
        user_message: &str,
        assistant_message: &str,
    ) -> Result<()>;

    /// Tool schemas the provider exposes to the LLM. These appear
    /// alongside built-in tools in the agent's tool list.
    /// Provider-supplied schemas must NOT shadow built-in names
    /// (`memory_recall`, `memory_store`, `memory_forget`,
    /// `shell`, `read_file`, etc.); the manager rejects collisions
    /// at agent-build time.
    fn get_tool_schemas(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    /// Handle a tool call routed to this provider (when the LLM
    /// invokes one of the names returned by [`Self::get_tool_schemas`]).
    /// Returns the tool's output and success flag, identical to
    /// the agent's regular [`crate::tools::traits::Tool::execute`]
    /// shape.
    ///
    /// Default impl errors — providers that return non-empty
    /// schemas MUST override.
    async fn handle_tool_call(
        &self,
        name: &str,
        _args: Value,
    ) -> Result<MemoryToolResult> {
        anyhow::bail!(
            "memory provider '{}' has no handler for tool '{}'",
            self.name(),
            name
        )
    }

    /// Per-session teardown. Called when the agent's session ends
    /// (`clear_history`) or the agent is dropped. Implementations
    /// should release resources (connections, file handles).
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    // -- Optional hooks ------------------------------------------------------

    /// Observe each agent turn at start. Useful for tracking the
    /// "current user message" before any other processing kicks in.
    async fn on_turn_start(&self, _user_message: &str) -> Result<()> {
        Ok(())
    }

    /// Observe context-compression events. Returns text that the
    /// agent merges into the compressed context. Empty string skips
    /// contribution. Default impl returns empty.
    async fn on_pre_compress(&self, _messages: &[ChatMessage]) -> Result<String> {
        Ok(String::new())
    }

    /// Observe a built-in memory write so the provider can mirror
    /// it (e.g. echo a `memory_store` into a hosted index). Default
    /// impl does nothing.
    async fn on_memory_write(
        &self,
        _action: MemoryWriteAction,
        _key: &str,
        _content: &str,
    ) -> Result<()> {
        Ok(())
    }
}

/// Result returned from [`MemoryProvider::handle_tool_call`].
#[derive(Debug, Clone)]
pub struct MemoryToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Action discriminator for [`MemoryProvider::on_memory_write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryWriteAction {
    /// `memory_store` was called to add or update an entry.
    Store,
    /// `memory_forget` was called to delete an entry.
    Forget,
}

/// Context passed to [`MemoryProvider::initialize`]. Carries
/// agent-side info the provider may need to scope its setup
/// (profile-aware paths, platform identity, etc.).
#[derive(Debug, Clone)]
pub struct MemoryProviderContext {
    /// Stable session identifier. Same shape as the
    /// [`SessionEvent`](super::hooks::SessionEvent) used by lifecycle
    /// hooks; survives until [`Agent::clear_history`] generates a
    /// new one.
    pub session_id: String,
    /// Active Fennec home directory for profile-aware storage.
    /// Providers should NOT hardcode `~/.fennec` paths — they break
    /// the `--profile` flag and any future per-profile isolation.
    pub fennec_home: std::path::PathBuf,
    /// Platform identifier (`"cli"`, `"telegram"`, `"discord"`,
    /// `"gateway"`, etc.). Providers can skip writes for
    /// non-primary contexts if they care.
    pub platform: String,
}
