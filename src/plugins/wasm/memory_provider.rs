//! Adapter that exposes a WASM plugin as a [`MemoryProvider`].
//!
//! The plugin implements the memory-* exports declared in
//! `wit/plugin.wit`; this adapter wraps them in the Rust trait the
//! agent's [`MemoryManager`](crate::plugins::MemoryManager) consumes.
//! Each trait method bridges sync→async via `block_in_place +
//! block_on` to drive the wasm call to completion (the same pattern
//! used by `wasm/host.rs` for the inverse direction).

use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;
use tokio::runtime::Handle;

use crate::plugins::memory_provider::{
    MemoryProvider, MemoryProviderContext, MemoryToolResult, MemoryWriteAction,
};
use crate::providers::traits::ChatMessage;
use crate::tools::traits::ToolSpec;

use super::runtime::WasmPluginInstance;

/// `MemoryProvider` impl backed by a WASM plugin's `memory-*`
/// exports.
pub struct WasmMemoryProvider {
    /// Stable name returned to the agent. Sourced from the plugin
    /// manifest at construction time so we don't need a wasm call
    /// to answer `name()`.
    name: String,
    /// The plugin instance whose exports we drive.
    instance: Arc<WasmPluginInstance>,
    /// Tokio runtime handle for `block_on` calls inside the
    /// synchronous trait methods.
    rt_handle: Handle,
}

impl WasmMemoryProvider {
    pub fn new(name: String, instance: Arc<WasmPluginInstance>, rt_handle: Handle) -> Self {
        Self {
            name,
            instance,
            rt_handle,
        }
    }
}

#[async_trait]
impl MemoryProvider for WasmMemoryProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_available(&self) -> bool {
        // Sync method on the trait; we still need to drive the
        // async wasm call. Default to false on error so a
        // misbehaving plugin doesn't accidentally activate.
        let inst = Arc::clone(&self.instance);
        let result = tokio::task::block_in_place(|| {
            self.rt_handle.block_on(inst.call_memory_is_available())
        });
        match result {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    provider = %self.name,
                    "WASM memory_is_available trapped: {e}; treating as unavailable"
                );
                false
            }
        }
    }

    async fn initialize(&self, ctx: &MemoryProviderContext) -> Result<()> {
        let res = self
            .instance
            .call_memory_initialize(
                &ctx.session_id,
                &ctx.fennec_home.display().to_string(),
                &ctx.platform,
            )
            .await?;
        res.map_err(|e| anyhow!("plugin '{}' initialize: {}", self.name, e))
    }

    fn system_prompt_block(&self) -> String {
        let inst = Arc::clone(&self.instance);
        tokio::task::block_in_place(|| {
            self.rt_handle.block_on(inst.call_memory_system_prompt_block())
        })
        .unwrap_or_default()
    }

    async fn prefetch(&self, query: &str) -> Result<String> {
        let res = self.instance.call_memory_prefetch(query).await?;
        res.map_err(|e| anyhow!("plugin '{}' prefetch: {}", self.name, e))
    }

    async fn sync_turn(&self, user: &str, assistant: &str) -> Result<()> {
        let res = self.instance.call_memory_sync_turn(user, assistant).await?;
        res.map_err(|e| anyhow!("plugin '{}' sync_turn: {}", self.name, e))
    }

    fn get_tool_schemas(&self) -> Vec<ToolSpec> {
        let inst = Arc::clone(&self.instance);
        let schemas = match tokio::task::block_in_place(|| {
            self.rt_handle.block_on(inst.call_memory_tool_schemas())
        }) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    provider = %self.name,
                    "WASM memory_tool_schemas trapped: {e}; returning empty"
                );
                return Vec::new();
            }
        };
        schemas
            .into_iter()
            .filter_map(|s| {
                let parameters: Value = if s.parameters_schema_json.trim().is_empty() {
                    serde_json::json!({"type": "object"})
                } else {
                    match serde_json::from_str(&s.parameters_schema_json) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(
                                provider = %self.name,
                                tool = %s.name,
                                "Invalid parameters_schema_json: {e}; skipping tool"
                            );
                            return None;
                        }
                    }
                };
                Some(ToolSpec {
                    name: s.name,
                    description: s.description,
                    parameters,
                })
            })
            .collect()
    }

    async fn handle_tool_call(
        &self,
        name: &str,
        args: Value,
    ) -> Result<MemoryToolResult> {
        let args_json = args.to_string();
        let res = self
            .instance
            .call_memory_handle_tool_call(name, &args_json)
            .await?;
        match res {
            Ok(r) => Ok(MemoryToolResult {
                success: r.success,
                output: r.output,
                error: r.error,
            }),
            Err(e) => Err(anyhow!("plugin '{}' tool '{}': {}", self.name, name, e)),
        }
    }

    async fn shutdown(&self) -> Result<()> {
        let res = self.instance.call_memory_shutdown().await?;
        res.map_err(|e| anyhow!("plugin '{}' shutdown: {}", self.name, e))
    }

    async fn on_turn_start(&self, user_message: &str) -> Result<()> {
        let res = self
            .instance
            .call_memory_on_turn_start(user_message)
            .await?;
        res.map_err(|e| anyhow!("plugin '{}' on_turn_start: {}", self.name, e))
    }

    async fn on_pre_compress(&self, messages: &[ChatMessage]) -> Result<String> {
        let messages_json =
            serde_json::to_string(messages).unwrap_or_else(|_| "[]".to_string());
        let res = self
            .instance
            .call_memory_on_pre_compress(&messages_json)
            .await?;
        res.map_err(|e| anyhow!("plugin '{}' on_pre_compress: {}", self.name, e))
    }

    async fn on_memory_write(
        &self,
        action: MemoryWriteAction,
        key: &str,
        content: &str,
    ) -> Result<()> {
        let res = self
            .instance
            .call_memory_on_memory_write(action, key, content)
            .await?;
        res.map_err(|e| anyhow!("plugin '{}' on_memory_write: {}", self.name, e))
    }
}
