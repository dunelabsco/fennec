//! WASM plugin runtime — wasmtime engine, component compilation,
//! and per-plugin instance management.
//!
//! The bindgen invocation reads `wit/plugin.wit` and produces typed
//! Rust bindings for the `fennec-plugin` world. The resulting module
//! gives us:
//!
//! - `FennecPlugin` — the top-level world struct, providing
//!   `instantiate(...)` and accessors for plugin exports.
//! - Per-interface host traits — `fennec::plugin::host::Host` is
//!   the trait we implement on `PluginHostState` to expose host
//!   functions to the wasm side.
//! - Per-interface generated types — `LogLevel`, `HttpRequest`,
//!   `HttpResponse`, `MemoryEntry`, `ToolSpec`, etc.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tokio::sync::Mutex as AsyncMutex;
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

use super::host::PluginHostState;

// Re-export the bindgen-generated types so other modules in this
// crate can name them without re-running the macro.
//
// Bindgen produces a module tree that mirrors the WIT package and
// world. The generated `FennecPlugin` struct represents the world;
// its `add_to_linker` and `instantiate` methods are how we wire
// hosts and plugins together.
wasmtime::component::bindgen!({
    path: "wit/plugin.wit",
    world: "fennec-plugin",
    async: false,
    trappable_imports: true,
});

/// Shared wasmtime engine. One per Fennec process; cheap to clone
/// (it's an `Arc<EngineInner>` internally).
#[derive(Clone)]
pub struct WasmEngine {
    engine: Engine,
}

impl WasmEngine {
    /// Build a fresh engine with default settings (Cranelift JIT,
    /// component model enabled).
    pub fn new() -> Result<Self> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        // Async would let host imports await futures directly, but
        // we drive async ourselves via the Tokio handle inside
        // PluginHostState (see `host.rs`). Sync mode keeps the wasm
        // execution path simpler and avoids the wasmtime fiber stack
        // overhead.
        config.async_support(false);
        let engine = Engine::new(&config).context("building wasmtime engine")?;
        Ok(Self { engine })
    }

    /// Compile a `.wasm` component from a file on disk.
    pub fn compile_component(&self, path: &Path) -> Result<Component> {
        Component::from_file(&self.engine, path)
            .with_context(|| format!("compiling wasm component {}", path.display()))
    }

    /// Build a linker pre-loaded with all of Fennec's host imports.
    pub fn linker(&self) -> Result<Linker<PluginHostState>> {
        let mut linker = Linker::<PluginHostState>::new(&self.engine);
        // Wire the bindgen-generated `host` interface against our
        // implementation of the `Host` trait (impl is in host.rs).
        FennecPlugin::add_to_linker(&mut linker, |state: &mut PluginHostState| state)
            .context("adding fennec-plugin host imports to linker")?;
        Ok(linker)
    }
}

/// One instantiated WASM plugin. Holds the Store + Instance behind
/// an async mutex so concurrent tool calls into the same plugin
/// are serialised (wasmtime stores are `!Sync`).
pub struct WasmPluginInstance {
    inner: Arc<AsyncMutex<InstanceInner>>,
    /// Cached plugin name for log/error context.
    pub plugin_name: String,
}

struct InstanceInner {
    store: Store<PluginHostState>,
    bindings: FennecPlugin,
}

impl WasmPluginInstance {
    /// Instantiate a compiled component with the given host state.
    pub fn instantiate(
        engine: &WasmEngine,
        component: &Component,
        host_state: PluginHostState,
    ) -> Result<Self> {
        let plugin_name = host_state.plugin_name.clone();
        let linker = engine.linker()?;
        let mut store = Store::new(&engine.engine, host_state);
        let bindings = FennecPlugin::instantiate(&mut store, component, &linker)
            .with_context(|| format!("instantiating wasm plugin '{}'", plugin_name))?;
        Ok(Self {
            inner: Arc::new(AsyncMutex::new(InstanceInner { store, bindings })),
            plugin_name,
        })
    }

    /// Call the plugin's exported `register` function and return the
    /// list of tool specs it provides.
    pub async fn call_register(&self) -> Result<Vec<ToolSpecOwned>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let specs = inner
            .bindings
            .fennec_plugin_plugin()
            .call_register(&mut inner.store)
            .with_context(|| format!("plugin '{}' register() trapped", self.plugin_name))?;
        Ok(specs.into_iter().map(ToolSpecOwned::from).collect())
    }

    /// Call the plugin's exported `invoke` function for a specific
    /// tool name with a JSON-encoded arguments string. Returns the
    /// JSON-encoded result on success or the plugin's error message
    /// on failure.
    pub async fn call_invoke(&self, tool_name: &str, args_json: &str) -> Result<String> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let result = inner
            .bindings
            .fennec_plugin_plugin()
            .call_invoke(&mut inner.store, tool_name, args_json)
            .with_context(|| {
                format!(
                    "plugin '{}' invoke({}) trapped",
                    self.plugin_name, tool_name
                )
            })?;
        result.map_err(|e| anyhow!("plugin '{}' returned error: {}", self.plugin_name, e))
    }

    /// Call the plugin's exported `on-pre-tool-call` hook.
    pub async fn call_on_pre_tool_call(
        &self,
        tool_name: &str,
        args_json: &str,
    ) -> Result<crate::plugins::PreToolCallAction> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let action = inner
            .bindings
            .fennec_plugin_plugin()
            .call_on_pre_tool_call(&mut inner.store, tool_name, args_json)
            .with_context(|| {
                format!(
                    "plugin '{}' on-pre-tool-call({}) trapped",
                    self.plugin_name, tool_name
                )
            })?;
        Ok(wit_to_pre_tool_action(action))
    }

    /// Call the plugin's exported `on-post-tool-call` hook.
    pub async fn call_on_post_tool_call(
        &self,
        tool_name: &str,
        args_json: &str,
        output: &str,
        success: bool,
    ) -> Result<crate::plugins::PostToolCallAction> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let action = inner
            .bindings
            .fennec_plugin_plugin()
            .call_on_post_tool_call(&mut inner.store, tool_name, args_json, output, success)
            .with_context(|| {
                format!(
                    "plugin '{}' on-post-tool-call({}) trapped",
                    self.plugin_name, tool_name
                )
            })?;
        Ok(wit_to_post_tool_action(action))
    }

    /// Call the plugin's exported `on-pre-llm-call` hook (observer).
    pub async fn call_on_pre_llm_call(&self, messages_json: &str) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner
            .bindings
            .fennec_plugin_plugin()
            .call_on_pre_llm_call(&mut inner.store, messages_json)
            .with_context(|| {
                format!("plugin '{}' on-pre-llm-call trapped", self.plugin_name)
            })?;
        Ok(())
    }

    /// Call the plugin's exported `on-post-llm-call` hook (observer).
    pub async fn call_on_post_llm_call(&self, response_json: &str) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner
            .bindings
            .fennec_plugin_plugin()
            .call_on_post_llm_call(&mut inner.store, response_json)
            .with_context(|| {
                format!("plugin '{}' on-post-llm-call trapped", self.plugin_name)
            })?;
        Ok(())
    }

    /// Call the plugin's exported `on-session-start` hook (observer).
    pub async fn call_on_session_start(&self, session_id: &str) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner
            .bindings
            .fennec_plugin_plugin()
            .call_on_session_start(&mut inner.store, session_id)
            .with_context(|| {
                format!("plugin '{}' on-session-start trapped", self.plugin_name)
            })?;
        Ok(())
    }

    /// Call the plugin's exported `on-session-end` hook (observer).
    pub async fn call_on_session_end(&self, session_id: &str) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner
            .bindings
            .fennec_plugin_plugin()
            .call_on_session_end(&mut inner.store, session_id)
            .with_context(|| {
                format!("plugin '{}' on-session-end trapped", self.plugin_name)
            })?;
        Ok(())
    }

    // ---- Memory provider exports -----------------------------------

    pub async fn call_memory_is_available(&self) -> Result<bool> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let v = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_is_available(&mut inner.store)
            .with_context(|| {
                format!("plugin '{}' memory-is-available trapped", self.plugin_name)
            })?;
        Ok(v)
    }

    pub async fn call_memory_initialize(
        &self,
        session_id: &str,
        fennec_home: &str,
        platform: &str,
    ) -> Result<Result<(), String>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let ctx = exports::fennec::plugin::plugin::MemoryInitContext {
            session_id: session_id.to_string(),
            fennec_home: fennec_home.to_string(),
            platform: platform.to_string(),
        };
        let res = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_initialize(&mut inner.store, &ctx)
            .with_context(|| {
                format!("plugin '{}' memory-initialize trapped", self.plugin_name)
            })?;
        Ok(res)
    }

    pub async fn call_memory_system_prompt_block(&self) -> Result<String> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let s = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_system_prompt_block(&mut inner.store)
            .with_context(|| {
                format!(
                    "plugin '{}' memory-system-prompt-block trapped",
                    self.plugin_name
                )
            })?;
        Ok(s)
    }

    pub async fn call_memory_prefetch(
        &self,
        query: &str,
    ) -> Result<Result<String, String>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let res = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_prefetch(&mut inner.store, query)
            .with_context(|| {
                format!("plugin '{}' memory-prefetch trapped", self.plugin_name)
            })?;
        Ok(res)
    }

    pub async fn call_memory_sync_turn(
        &self,
        user: &str,
        assistant: &str,
    ) -> Result<Result<(), String>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let res = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_sync_turn(&mut inner.store, user, assistant)
            .with_context(|| {
                format!("plugin '{}' memory-sync-turn trapped", self.plugin_name)
            })?;
        Ok(res)
    }

    pub async fn call_memory_tool_schemas(
        &self,
    ) -> Result<Vec<MemoryToolSchemaOwned>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let schemas = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_tool_schemas(&mut inner.store)
            .with_context(|| {
                format!(
                    "plugin '{}' memory-tool-schemas trapped",
                    self.plugin_name
                )
            })?;
        Ok(schemas
            .into_iter()
            .map(|s| MemoryToolSchemaOwned {
                name: s.name,
                description: s.description,
                parameters_schema_json: s.parameters_schema_json,
            })
            .collect())
    }

    pub async fn call_memory_handle_tool_call(
        &self,
        name: &str,
        args_json: &str,
    ) -> Result<Result<MemoryToolResultOwned, String>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let res = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_handle_tool_call(&mut inner.store, name, args_json)
            .with_context(|| {
                format!(
                    "plugin '{}' memory-handle-tool-call trapped",
                    self.plugin_name
                )
            })?;
        Ok(res.map(|r| MemoryToolResultOwned {
            success: r.success,
            output: r.output,
            error: r.error,
        }))
    }

    pub async fn call_memory_shutdown(&self) -> Result<Result<(), String>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let res = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_shutdown(&mut inner.store)
            .with_context(|| {
                format!("plugin '{}' memory-shutdown trapped", self.plugin_name)
            })?;
        Ok(res)
    }

    pub async fn call_memory_on_turn_start(
        &self,
        user_message: &str,
    ) -> Result<Result<(), String>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let res = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_on_turn_start(&mut inner.store, user_message)
            .with_context(|| {
                format!(
                    "plugin '{}' memory-on-turn-start trapped",
                    self.plugin_name
                )
            })?;
        Ok(res)
    }

    pub async fn call_memory_on_pre_compress(
        &self,
        messages_json: &str,
    ) -> Result<Result<String, String>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let res = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_on_pre_compress(&mut inner.store, messages_json)
            .with_context(|| {
                format!(
                    "plugin '{}' memory-on-pre-compress trapped",
                    self.plugin_name
                )
            })?;
        Ok(res)
    }

    pub async fn call_memory_on_memory_write(
        &self,
        action: crate::plugins::MemoryWriteAction,
        key: &str,
        content: &str,
    ) -> Result<Result<(), String>> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let wit_action = match action {
            crate::plugins::MemoryWriteAction::Store => {
                exports::fennec::plugin::plugin::MemoryWriteAction::Store
            }
            crate::plugins::MemoryWriteAction::Forget => {
                exports::fennec::plugin::plugin::MemoryWriteAction::Forget
            }
        };
        let res = inner
            .bindings
            .fennec_plugin_plugin()
            .call_memory_on_memory_write(&mut inner.store, wit_action, key, content)
            .with_context(|| {
                format!(
                    "plugin '{}' memory-on-memory-write trapped",
                    self.plugin_name
                )
            })?;
        Ok(res)
    }
}

/// Owned copy of a WIT `memory-tool-schema`. Parallels
/// [`ToolSpecOwned`] for plugin-provided tools that are scoped to
/// the memory provider rather than the regular tool list.
#[derive(Debug, Clone)]
pub struct MemoryToolSchemaOwned {
    pub name: String,
    pub description: String,
    pub parameters_schema_json: String,
}

/// Owned copy of a WIT `memory-tool-result`.
#[derive(Debug, Clone)]
pub struct MemoryToolResultOwned {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Map the bindgen-generated `PreToolCallAction` into our public
/// Rust enum. Keeps the WIT shape internal so callers don't have to
/// import the bindgen module.
fn wit_to_pre_tool_action(
    action: exports::fennec::plugin::plugin::PreToolCallAction,
) -> crate::plugins::PreToolCallAction {
    use exports::fennec::plugin::plugin::PreToolCallAction as Wit;
    match action {
        Wit::Continue => crate::plugins::PreToolCallAction::Continue,
        Wit::Skip(reason) => crate::plugins::PreToolCallAction::Skip { reason },
        Wit::Rewrite(args_json) => {
            // The plugin returned a JSON-encoded args object. Parse
            // it; on parse failure fall back to Continue with a warn
            // log so a malformed plugin can't crash the agent.
            match serde_json::from_str::<serde_json::Value>(&args_json) {
                Ok(args) => crate::plugins::PreToolCallAction::Rewrite { args },
                Err(e) => {
                    tracing::warn!(
                        "Plugin returned invalid JSON in pre-tool-call rewrite: {e}; treating as Continue"
                    );
                    crate::plugins::PreToolCallAction::Continue
                }
            }
        }
    }
}

/// Map the bindgen-generated `PostToolCallAction` into our public
/// Rust enum.
fn wit_to_post_tool_action(
    action: exports::fennec::plugin::plugin::PostToolCallAction,
) -> crate::plugins::PostToolCallAction {
    use exports::fennec::plugin::plugin::PostToolCallAction as Wit;
    match action {
        Wit::Continue => crate::plugins::PostToolCallAction::Continue,
        Wit::Rewrite(rw) => crate::plugins::PostToolCallAction::Rewrite {
            output: rw.output,
            success: rw.success,
        },
    }
}

/// Owned copy of the bindgen-generated `ToolSpec`. The bindgen type
/// borrows from the wasm store; we copy out so callers don't need
/// to hold the store lock across await points.
#[derive(Debug, Clone)]
pub struct ToolSpecOwned {
    pub name: String,
    pub description: String,
    pub parameters_schema_json: String,
}

impl From<exports::fennec::plugin::plugin::ToolSpec> for ToolSpecOwned {
    fn from(s: exports::fennec::plugin::plugin::ToolSpec) -> Self {
        Self {
            name: s.name,
            description: s.description,
            parameters_schema_json: s.parameters_schema_json,
        }
    }
}

// ---------------------------------------------------------------------------
// Host trait implementation
// ---------------------------------------------------------------------------
//
// Bindgen generates a `Host` trait under
// `fennec::plugin::host::Host` based on the WIT host interface.
// We implement it on `PluginHostState` here so that the linker
// (above) can wire wasm imports to our Rust functions.

use fennec::plugin::host::{
    Host, HostError, HttpHeader, HttpReq, HttpResp, LogLevel as WitLogLevel, MemoryEntry as WitMemEntry,
};

use super::host::{
    self, host_channel_send, host_config_get_string, host_http_request, host_log,
    host_memory_forget, host_memory_get, host_memory_recall, host_memory_store,
    host_now_millis, host_read_file, host_write_file, LogLevel, WasmHttpRequest,
};

// `trappable_imports: true` wraps every host fn return in a
// `Result<T, anyhow::Error>` where the outer `Err` triggers a wasm
// trap. We never trap from these functions — application errors
// surface through the inner `Result<_, HostError>` per WIT — so the
// outer is always `Ok(...)`.
impl Host for PluginHostState {
    fn log(&mut self, level: WitLogLevel, message: String) -> wasmtime::Result<()> {
        host_log(self, wit_to_log_level(level), &message);
        Ok(())
    }

    fn now_millis(&mut self) -> wasmtime::Result<u64> {
        Ok(host_now_millis())
    }

    fn http_request(
        &mut self,
        req: HttpReq,
    ) -> wasmtime::Result<Result<HttpResp, HostError>> {
        let r = WasmHttpRequest {
            method: req.method,
            url: req.url,
            headers: req
                .headers
                .into_iter()
                .map(|h| (h.name, h.value))
                .collect(),
            body: req.body,
        };
        Ok(host_http_request(self, r)
            .map(|resp| HttpResp {
                status: resp.status,
                headers: resp
                    .headers
                    .into_iter()
                    .map(|(name, value)| HttpHeader { name, value })
                    .collect(),
                body: resp.body,
            })
            .map_err(|message| HostError { message }))
    }

    fn read_file(&mut self, path: String) -> wasmtime::Result<Result<Vec<u8>, HostError>> {
        Ok(host_read_file(self, &path).map_err(|message| HostError { message }))
    }

    fn write_file(
        &mut self,
        path: String,
        contents: Vec<u8>,
    ) -> wasmtime::Result<Result<(), HostError>> {
        Ok(host_write_file(self, &path, &contents)
            .map_err(|message| HostError { message }))
    }

    fn memory_recall(
        &mut self,
        query: String,
        limit: u32,
    ) -> wasmtime::Result<Result<Vec<WitMemEntry>, HostError>> {
        Ok(host_memory_recall(self, &query, limit)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|e| WitMemEntry {
                        key: e.key,
                        content: e.content,
                        category: e.category,
                        created_at: e.created_at,
                    })
                    .collect()
            })
            .map_err(|message| HostError { message }))
    }

    fn memory_store(
        &mut self,
        key: String,
        content: String,
    ) -> wasmtime::Result<Result<(), HostError>> {
        Ok(host_memory_store(self, &key, &content)
            .map_err(|message| HostError { message }))
    }

    fn memory_get(
        &mut self,
        key: String,
    ) -> wasmtime::Result<Result<Option<WitMemEntry>, HostError>> {
        Ok(host_memory_get(self, &key)
            .map(|opt| {
                opt.map(|e| WitMemEntry {
                    key: e.key,
                    content: e.content,
                    category: e.category,
                    created_at: e.created_at,
                })
            })
            .map_err(|message| HostError { message }))
    }

    fn memory_forget(&mut self, key: String) -> wasmtime::Result<Result<bool, HostError>> {
        Ok(host_memory_forget(self, &key).map_err(|message| HostError { message }))
    }

    fn config_get_string(&mut self, key: String) -> wasmtime::Result<Option<String>> {
        Ok(host_config_get_string(self, &key))
    }

    fn channel_send(
        &mut self,
        channel: String,
        chat_id: String,
        content: String,
    ) -> wasmtime::Result<Result<(), HostError>> {
        Ok(host_channel_send(self, &channel, &chat_id, &content)
            .map_err(|message| HostError { message }))
    }
}

fn wit_to_log_level(l: WitLogLevel) -> LogLevel {
    match l {
        WitLogLevel::Trace => LogLevel::Trace,
        WitLogLevel::Debug => LogLevel::Debug,
        WitLogLevel::Info => LogLevel::Info,
        WitLogLevel::Warn => LogLevel::Warn,
        WitLogLevel::Error => LogLevel::Error,
    }
}
