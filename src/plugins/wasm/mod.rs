//! WASM-loaded plugin runtime.
//!
//! Turns a `.wasm` Component-Model component on disk into a set of
//! [`Tool`](crate::tools::traits::Tool)s the agent can call. The flow:
//!
//! 1. Loader scans `~/.fennec/plugins/<name>/` for `plugin.toml` +
//!    `<name>.wasm`.
//! 2. Manifest is parsed and validated.
//! 3. If the plugin's name is in `[plugins].enabled`, the `.wasm` is
//!    compiled by `wasmtime` and instantiated against a `Linker`
//!    that has the host imports (log, http_request, read_file,
//!    write_file, memory_recall, memory_store, now-millis) wired up.
//! 4. The plugin's exported `register` function is called once to
//!    obtain the list of tool specs it provides.
//! 5. Each spec is wrapped in a [`WasmTool`] — a `Box<dyn Tool>` that
//!    calls back into the plugin's `invoke` whenever the agent fires
//!    the corresponding tool.
//!
//! Per-plugin isolation: each plugin has its own [`wasmtime::Store`]
//! and [`wasmtime::component::Instance`]. Wasmtime stores are `!Sync`
//! by design, so each plugin's tool calls are serialised through a
//! [`tokio::sync::Mutex`]. Multiple plugins still run in parallel.
//!
//! Trust model: WASM gives Fennec a sandbox. Plugins can only do what
//! the host explicitly exposes — host imports are the entire
//! capability surface. Path / URL guards inside the host functions
//! ensure that a plugin can't do anything the agent itself couldn't.

pub mod host;
pub mod loader;
pub mod runtime;
pub mod tool;

pub use host::PluginHostState;
pub use loader::{discover_wasm_plugins, DiscoveredWasmPlugin};
pub use runtime::{WasmEngine, WasmPluginInstance};
pub use tool::WasmTool;
