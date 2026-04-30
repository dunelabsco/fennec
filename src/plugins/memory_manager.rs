//! Orchestrator for the always-on built-in memory + optional
//! single external [`MemoryProvider`].
//!
//! This is the single point through which the agent loops talk to
//! the augmentation layer. Everything outside `plugins/` continues
//! to use the [`Memory`](crate::memory::traits::Memory) trait
//! directly for raw store/recall — the manager only wires the
//! external provider into the lifecycle.
//!
//! # Lifecycle integration
//!
//! - **Session start**: `initialize_active(...)` calls
//!   [`MemoryProvider::initialize`] on the active provider (if any).
//! - **System prompt build**: `system_prompt_block()` returns the
//!   provider's static text to merge into the prompt.
//! - **Before each LLM call**: `prefetch_for_turn(query)` returns
//!   formatted context the agent injects into the user message.
//! - **After each turn**: `sync_turn(user, assistant)` lets the
//!   provider observe.
//! - **Tool dispatch**: `tool_schemas()` returns provider-supplied
//!   schemas (merged into the agent's tool list);
//!   `handle_tool_call(name, args)` dispatches a call to the
//!   provider when its name matches.
//! - **Session end**: `shutdown_active()` calls
//!   [`MemoryProvider::shutdown`].

use std::sync::Arc;

use anyhow::Result;

use crate::providers::traits::ChatMessage;
use crate::tools::traits::ToolSpec;

use super::memory_provider::{
    MemoryProvider, MemoryProviderContext, MemoryToolResult, MemoryWriteAction,
};

/// The orchestrator. Holds the active external provider (if any)
/// and exposes the agent-facing surface.
pub struct MemoryManager {
    /// `Some(provider)` when the configured `memory.provider` name
    /// resolved to a registered, available [`MemoryProvider`].
    /// `None` means built-in memory is the only memory layer
    /// running — the default and current behavior of Fennec.
    active: Option<Arc<dyn MemoryProvider>>,
}

impl MemoryManager {
    /// Build a manager with no external provider active. The
    /// agent's behavior is identical to pre-C3 Fennec.
    pub fn empty() -> Self {
        Self { active: None }
    }

    /// Build a manager with one active external provider.
    pub fn with_provider(provider: Arc<dyn MemoryProvider>) -> Self {
        Self {
            active: Some(provider),
        }
    }

    /// Return the active provider's name, or `"builtin"` when no
    /// external is wired. Used in diagnostic logs and `fennec doctor`.
    pub fn active_name(&self) -> &str {
        self.active.as_ref().map(|p| p.name()).unwrap_or("builtin")
    }

    /// `true` when an external provider is wired and active.
    pub fn has_external(&self) -> bool {
        self.active.is_some()
    }

    /// Call [`MemoryProvider::initialize`] on the active provider.
    /// No-op when none is wired.
    pub async fn initialize(&self, ctx: &MemoryProviderContext) -> Result<()> {
        if let Some(p) = self.active.as_ref() {
            p.initialize(ctx).await?;
        }
        Ok(())
    }

    /// Static system-prompt text from the active provider. Empty
    /// string when no provider is wired.
    pub fn system_prompt_block(&self) -> String {
        match self.active.as_ref() {
            Some(p) => p.system_prompt_block(),
            None => String::new(),
        }
    }

    /// Tool schemas the active provider exposes. Empty when none.
    pub fn tool_schemas(&self) -> Vec<ToolSpec> {
        match self.active.as_ref() {
            Some(p) => p.get_tool_schemas(),
            None => Vec::new(),
        }
    }

    /// Returns `true` if `tool_name` matches one of the active
    /// provider's tools. Used by the agent's tool dispatch to
    /// decide whether to route a call to the provider vs the
    /// regular built-in tool path.
    pub fn handles_tool(&self, tool_name: &str) -> bool {
        match self.active.as_ref() {
            Some(p) => p.get_tool_schemas().iter().any(|s| s.name == tool_name),
            None => false,
        }
    }

    /// Dispatch a tool call to the active provider. Caller is
    /// responsible for checking [`Self::handles_tool`] first.
    pub async fn handle_tool_call(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<MemoryToolResult> {
        let provider = self
            .active
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no memory provider active"))?;
        provider.handle_tool_call(name, args).await
    }

    /// Recall context for the upcoming turn. Errors from the
    /// provider are logged and swallowed — a misbehaving provider
    /// must not block agent progress.
    pub async fn prefetch_for_turn(&self, query: &str) -> String {
        let Some(provider) = self.active.as_ref() else {
            return String::new();
        };
        match provider.prefetch(query).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    provider = %provider.name(),
                    "MemoryProvider::prefetch failed: {e}; ignoring"
                );
                String::new()
            }
        }
    }

    /// Observe a completed turn. Errors are logged and swallowed.
    pub async fn sync_turn(&self, user_message: &str, assistant_message: &str) {
        let Some(provider) = self.active.as_ref() else {
            return;
        };
        if let Err(e) = provider.sync_turn(user_message, assistant_message).await {
            tracing::warn!(
                provider = %provider.name(),
                "MemoryProvider::sync_turn failed: {e}; ignoring"
            );
        }
    }

    /// Optional hook: turn-start observer.
    pub async fn on_turn_start(&self, user_message: &str) {
        let Some(p) = self.active.as_ref() else { return };
        if let Err(e) = p.on_turn_start(user_message).await {
            tracing::warn!(provider = %p.name(), "on_turn_start failed: {e}");
        }
    }

    /// Optional hook: pre-compression observer. Returns text that
    /// the agent's compressor merges into the compressed history.
    pub async fn on_pre_compress(&self, messages: &[ChatMessage]) -> String {
        let Some(p) = self.active.as_ref() else {
            return String::new();
        };
        match p.on_pre_compress(messages).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(provider = %p.name(), "on_pre_compress failed: {e}");
                String::new()
            }
        }
    }

    /// Optional hook: built-in memory write observer (mirror writes).
    pub async fn on_memory_write(
        &self,
        action: MemoryWriteAction,
        key: &str,
        content: &str,
    ) {
        let Some(p) = self.active.as_ref() else { return };
        if let Err(e) = p.on_memory_write(action, key, content).await {
            tracing::warn!(
                provider = %p.name(),
                "on_memory_write failed: {e}"
            );
        }
    }

    /// Shut the active provider down at session end.
    pub async fn shutdown(&self) {
        let Some(p) = self.active.as_ref() else { return };
        if let Err(e) = p.shutdown().await {
            tracing::warn!(provider = %p.name(), "shutdown failed: {e}");
        }
    }
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;

    /// Manager with no external is byte-identical to today: empty
    /// prompt block, empty schemas, no tool routing, prefetch
    /// returns empty.
    #[tokio::test]
    async fn empty_manager_is_no_op() {
        let m = MemoryManager::empty();
        assert_eq!(m.active_name(), "builtin");
        assert!(!m.has_external());
        assert_eq!(m.system_prompt_block(), "");
        assert!(m.tool_schemas().is_empty());
        assert!(!m.handles_tool("anything"));
        assert_eq!(m.prefetch_for_turn("query").await, "");
        m.sync_turn("u", "a").await;
        m.on_turn_start("u").await;
        m.shutdown().await;
    }

    struct StubProvider {
        name: &'static str,
    }

    #[async_trait]
    impl MemoryProvider for StubProvider {
        fn name(&self) -> &str {
            self.name
        }
        fn is_available(&self) -> bool {
            true
        }
        async fn initialize(&self, _ctx: &MemoryProviderContext) -> Result<()> {
            Ok(())
        }
        fn system_prompt_block(&self) -> String {
            "[stub block]".to_string()
        }
        async fn prefetch(&self, query: &str) -> Result<String> {
            Ok(format!("[stub prefetch for: {query}]"))
        }
        async fn sync_turn(&self, _user: &str, _assistant: &str) -> Result<()> {
            Ok(())
        }
        fn get_tool_schemas(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "stub_tool".to_string(),
                description: "Stub provider tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            }]
        }
        async fn handle_tool_call(
            &self,
            name: &str,
            _args: serde_json::Value,
        ) -> Result<MemoryToolResult> {
            Ok(MemoryToolResult {
                success: true,
                output: format!("handled {name}"),
                error: None,
            })
        }
    }

    /// Manager wired with a provider routes lifecycle calls to it.
    #[tokio::test]
    async fn provider_lifecycle_round_trips() {
        let p = Arc::new(StubProvider { name: "stub" });
        let m = MemoryManager::with_provider(p);
        assert_eq!(m.active_name(), "stub");
        assert!(m.has_external());
        assert_eq!(m.system_prompt_block(), "[stub block]");
        let schemas = m.tool_schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "stub_tool");
        assert!(m.handles_tool("stub_tool"));
        assert!(!m.handles_tool("memory_recall"));
        let prefetched = m.prefetch_for_turn("hello").await;
        assert_eq!(prefetched, "[stub prefetch for: hello]");
        let result = m
            .handle_tool_call("stub_tool", serde_json::json!({}))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "handled stub_tool");
    }

    /// A failing provider's prefetch returns empty rather than
    /// propagating the error. Agent must not abort.
    #[tokio::test]
    async fn provider_prefetch_failure_returns_empty() {
        struct Failing;
        #[async_trait]
        impl MemoryProvider for Failing {
            fn name(&self) -> &str {
                "failing"
            }
            fn is_available(&self) -> bool {
                true
            }
            async fn initialize(&self, _ctx: &MemoryProviderContext) -> Result<()> {
                Ok(())
            }
            async fn prefetch(&self, _query: &str) -> Result<String> {
                Err(anyhow::anyhow!("prefetch boom"))
            }
            async fn sync_turn(&self, _user: &str, _assistant: &str) -> Result<()> {
                Ok(())
            }
        }
        let m = MemoryManager::with_provider(Arc::new(Failing));
        let result = m.prefetch_for_turn("q").await;
        assert_eq!(result, "");
    }
}
