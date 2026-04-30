//! Plugin system foundation.
//!
//! Plugins extend Fennec without modifying the core. A plugin is a unit
//! of code that registers tools (and, in later phases, lifecycle hooks,
//! channels, providers, and CLI commands) into the agent at startup.
//!
//! # Plugin sources
//!
//! This module ships the **bundled** path: plugins compiled into the
//! Fennec binary itself, registered at compile time via the
//! [`inventory`] crate. Each bundled plugin calls
//! [`inventory::submit!`] at module scope to register a static
//! reference to itself; the [`PluginRegistry`] iterates those entries
//! at startup and calls each plugin's [`Plugin::register`].
//!
//! Subsequent phases will add:
//!
//! - **WASM plugins** under `~/.fennec/plugins/<name>/<name>.wasm` —
//!   user-installed, sandboxed by `wasmtime`, ABI-stable across Fennec
//!   versions, can be authored in any language that targets WASM.
//! - **Memory-provider plugins** with the same trait shape but a
//!   single-active-provider category model.
//! - **Lifecycle hooks** (`pre_tool_call`, `post_tool_call`,
//!   `pre_llm_call`, `on_session_start`, etc).
//! - **CLI command registration** so plugins can add `fennec <plugin>
//!   <subcommand>` entries.
//!
//! # Default behaviour
//!
//! Bundled plugins are compiled into the binary but are NOT
//! automatically activated. Each plugin must be explicitly listed in
//! `[plugins] enabled = ["..."]` in `config.toml`. The default
//! `enabled` list is empty, so installs that don't opt in see no
//! plugin behaviour at all — byte-identical with the pre-plugin
//! Fennec.
//!
//! # Authoring a bundled plugin
//!
//! ```rust,ignore
//! use fennec::plugins::{Plugin, PluginContext, PluginEntry, PluginManifest, PluginKind};
//!
//! struct MyPlugin;
//!
//! impl Plugin for MyPlugin {
//!     fn manifest(&self) -> PluginManifest {
//!         PluginManifest::new("my-plugin", env!("CARGO_PKG_VERSION"))
//!             .with_description("Adds my custom tools")
//!             .with_kind(PluginKind::Standalone)
//!     }
//!
//!     fn register(&self, ctx: &mut PluginContext) -> anyhow::Result<()> {
//!         ctx.register_tool(Box::new(MyTool));
//!         Ok(())
//!     }
//! }
//!
//! inventory::submit! { PluginEntry { plugin: &MyPlugin } }
//! ```

mod context;
mod demo;
mod hooks;
mod manifest;
mod registry;
mod traits;
pub mod wasm;

pub use context::PluginContext;
pub use hooks::{
    HookKind, HookRegistry, OnSessionEndHook, OnSessionStartHook, PostLlmCallEvent,
    PostLlmCallHook, PostToolCallAction, PostToolCallEvent, PostToolCallHook,
    PostToolResolution, PreLlmCallEvent, PreLlmCallHook, PreToolCallAction,
    PreToolCallEvent, PreToolCallHook, PreToolResolution, SessionEvent,
};
pub use manifest::{PluginKind, PluginManifest};
pub use registry::{LoadedPlugin, PluginRegistry, WasmHostResources};
pub use traits::{Plugin, PluginEntry};

/// Re-export the inventory crate so plugin authors don't have to add it
/// to their own `Cargo.toml`. The convention is:
///
/// ```rust,ignore
/// use fennec::plugins::inventory;
/// inventory::submit! { fennec::plugins::PluginEntry { plugin: &MyPlugin } }
/// ```
pub use inventory;
