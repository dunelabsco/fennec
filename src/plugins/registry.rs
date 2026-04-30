//! The plugin loader and runtime registry.
//!
//! [`PluginRegistry`] is the orchestrator. It walks the inventory of
//! bundled plugins, applies the user's `[plugins].enabled` allowlist,
//! validates each manifest, calls each enabled plugin's `register`
//! method, and collects the resulting tool boxes for the agent
//! builder.
//!
//! A plugin that fails validation or panics in `register` is dropped
//! with an error log; the registry continues. This matches the
//! "one bad plugin should not bring down the agent" policy used by
//! Hermes (see `hermes_cli/plugins.py` — failed plugins log a warning
//! and the loader proceeds).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tokio::runtime::Handle;

use crate::bus::MessageBus;
use crate::memory::traits::Memory;
use crate::security::path_sandbox::PathSandbox;
use crate::tools::traits::Tool;

use super::context::PluginContext;
use super::hooks::HookRegistry;
use super::manifest::PluginManifest;
use super::traits::PluginEntry;
use super::wasm::host::PluginHostState;
use super::wasm::loader::discover_wasm_plugins;
use super::wasm::runtime::{WasmEngine, WasmPluginInstance};
use super::wasm::tool::WasmTool;

/// Diagnostic record for a successfully-loaded plugin. Surfaced by
/// `fennec doctor` (later) and through [`PluginRegistry::loaded`].
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub tool_count: usize,
}

/// The plugin registry — drives discovery, activation, and
/// collection of plugin-provided tools.
///
/// Lifecycle in `main.rs`:
///
/// 1. Construct: `let mut reg = PluginRegistry::new();`
/// 2. Load enabled bundled plugins from config:
///    `reg.load_bundled(&config.plugins.enabled)?;`
/// 3. Drain into the agent builder:
///    `for tool in reg.into_tools() { builder = builder.tool(tool); }`
pub struct PluginRegistry {
    loaded: Vec<LoadedPlugin>,
    tools: Vec<Box<dyn Tool>>,
    hooks: HookRegistry,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            loaded: Vec::new(),
            tools: Vec::new(),
            hooks: HookRegistry::new(),
        }
    }

    /// Load all bundled plugins whose manifest name appears in
    /// `enabled`. Plugins not in `enabled` are skipped silently —
    /// they remain compiled into the binary but inactive.
    ///
    /// Returns `Ok(())` even if some plugins fail to load; failures
    /// are logged and tracked separately via tracing. Returns `Err`
    /// only on a structural failure that prevents iterating the
    /// inventory at all (which should never happen in practice).
    pub fn load_bundled(&mut self, enabled: &[String]) -> Result<()> {
        if enabled.is_empty() {
            tracing::debug!("Plugin loader: no plugins enabled in config");
            return Ok(());
        }

        let want: HashSet<&str> = enabled.iter().map(String::as_str).collect();
        let mut seen_names: HashSet<String> = HashSet::new();

        for entry in inventory::iter::<PluginEntry> {
            let manifest = entry.plugin.manifest();

            // Validate first so we never invoke a plugin with a junk name
            // (which could later cause path-traversal in WASM-plugin
            // resolution paths or break filtering).
            if let Err(e) = manifest.validate() {
                tracing::error!(
                    "Skipping plugin with invalid manifest: {e}"
                );
                continue;
            }

            // Detect duplicate names across bundled plugins. This is
            // a coding mistake (two crates registered the same name);
            // log it loudly but keep the first one we saw.
            if !seen_names.insert(manifest.name.clone()) {
                tracing::error!(
                    plugin = %manifest.name,
                    "Duplicate bundled plugin name; ignoring second registration"
                );
                continue;
            }

            // Allowlist filter. Bundled plugins ship with the binary
            // but stay dormant until the operator names them.
            if !want.contains(manifest.name.as_str()) {
                tracing::debug!(
                    plugin = %manifest.name,
                    "Bundled plugin present but not in [plugins].enabled; skipping"
                );
                continue;
            }

            let mut ctx = PluginContext::new();
            match entry.plugin.register(&mut ctx) {
                Ok(()) => {
                    let parts = ctx.into_parts();
                    let tool_count = parts.tools.len();
                    let hook_count = parts.pre_tool_hooks.len()
                        + parts.post_tool_hooks.len()
                        + parts.pre_llm_hooks.len()
                        + parts.post_llm_hooks.len()
                        + parts.on_session_start_hooks.len()
                        + parts.on_session_end_hooks.len();
                    tracing::info!(
                        plugin = %manifest.name,
                        version = %manifest.version,
                        tools = tool_count,
                        hooks = hook_count,
                        "Loaded bundled plugin"
                    );
                    self.tools.extend(parts.tools);
                    for h in parts.pre_tool_hooks {
                        self.hooks.register_pre_tool(h);
                    }
                    for h in parts.post_tool_hooks {
                        self.hooks.register_post_tool(h);
                    }
                    for h in parts.pre_llm_hooks {
                        self.hooks.register_pre_llm(h);
                    }
                    for h in parts.post_llm_hooks {
                        self.hooks.register_post_llm(h);
                    }
                    for h in parts.on_session_start_hooks {
                        self.hooks.register_on_session_start(h);
                    }
                    for h in parts.on_session_end_hooks {
                        self.hooks.register_on_session_end(h);
                    }
                    self.loaded.push(LoadedPlugin {
                        manifest,
                        tool_count,
                    });
                }
                Err(e) => {
                    tracing::error!(
                        plugin = %manifest.name,
                        "Plugin register() failed: {e}; continuing with other plugins"
                    );
                }
            }
        }

        // Surface unmatched names: user listed a plugin that isn't
        // bundled (and isn't a WASM plugin yet either, since C2 hasn't
        // landed). This is almost always a typo; warn so they notice.
        let loaded_names: HashSet<&str> =
            self.loaded.iter().map(|p| p.manifest.name.as_str()).collect();
        for requested in &want {
            if !loaded_names.contains(requested) {
                tracing::warn!(
                    plugin = %requested,
                    "Plugin requested in [plugins].enabled but not found among bundled plugins"
                );
            }
        }

        Ok(())
    }

    /// Discover, compile, and instantiate every WASM plugin under
    /// `plugins_root` whose manifest name appears in `enabled`.
    ///
    /// On any per-plugin failure (manifest invalid, component
    /// compilation failed, instantiation trapped, register() trapped)
    /// the plugin is dropped with an error log; the registry continues
    /// with the remaining plugins.
    ///
    /// The host resources (path sandbox, memory, http client, runtime
    /// handle) are cloned into a fresh [`PluginHostState`] for each
    /// plugin instance — wasmtime stores require owned state.
    pub fn load_wasm(
        &mut self,
        plugins_root: &Path,
        enabled: &[String],
        resources: WasmHostResources,
    ) -> Result<()> {
        let want: HashSet<&str> = enabled.iter().map(String::as_str).collect();

        let discovered = discover_wasm_plugins(plugins_root)?;
        if discovered.is_empty() {
            tracing::debug!(
                "WASM plugin loader: no plugins discovered under {}",
                plugins_root.display()
            );
            return Ok(());
        }

        // Build the engine once; share across all plugin
        // instantiations. Compilation is per-plugin, but engine
        // construction is the expensive part.
        let engine = match WasmEngine::new() {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    "Failed to build wasmtime engine: {e}; skipping all WASM plugins"
                );
                return Ok(());
            }
        };

        for d in discovered {
            let name = d.manifest.name.clone();
            if !want.contains(name.as_str()) {
                tracing::debug!(
                    plugin = %name,
                    "WASM plugin discovered but not in [plugins].enabled; skipping"
                );
                continue;
            }

            // Compile.
            let component = match engine.compile_component(&d.wasm_path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        plugin = %name,
                        path = %d.wasm_path.display(),
                        "Failed to compile WASM component: {e}"
                    );
                    continue;
                }
            };

            // Instantiate with a fresh host state per plugin. The
            // plugin sees only its own slice of `[plugins.settings]`
            // — pulled from the resources map at instantiation
            // time, so a config reload would require a restart
            // (matches every other config field).
            let plugin_settings = resources
                .settings
                .get(&name)
                .cloned()
                .unwrap_or_default();
            let state = PluginHostState {
                plugin_name: name.clone(),
                path_sandbox: Arc::clone(&resources.path_sandbox),
                memory: Arc::clone(&resources.memory),
                http_client: resources.http_client.clone(),
                rt_handle: resources.rt_handle.clone(),
                settings: plugin_settings,
                bus: resources.bus.clone(),
            };
            let instance = match WasmPluginInstance::instantiate(&engine, &component, state) {
                Ok(i) => Arc::new(i),
                Err(e) => {
                    tracing::error!(
                        plugin = %name,
                        "Failed to instantiate WASM plugin: {e}"
                    );
                    continue;
                }
            };

            // Call register() to discover the tool list. We have to
            // bridge sync→async here since the registry call is sync
            // but call_register awaits a Mutex. block_in_place tells
            // the multi-threaded runtime that this worker will
            // block; another worker takes over async tasks. Without
            // it, block_on panics when load_bundled() is itself
            // called from an async fn (which it is — build_agent is
            // async).
            let specs = match tokio::task::block_in_place(|| {
                resources.rt_handle.block_on(instance.call_register())
            }) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(
                        plugin = %name,
                        "WASM plugin register() failed: {e}"
                    );
                    continue;
                }
            };

            let tool_count = specs.len();
            for spec in specs {
                let spec_name = spec.name.clone();
                match WasmTool::from_spec(Arc::clone(&instance), spec) {
                    Ok(tool) => self.tools.push(Box::new(tool)),
                    Err(e) => {
                        tracing::error!(
                            plugin = %name,
                            tool = %spec_name,
                            "Skipping WASM-provided tool: {e}"
                        );
                    }
                }
            }

            // Register lifecycle hooks that bridge into this WASM
            // plugin. Each hook is a Rust closure that drives the
            // async wasm call via `block_in_place + block_on`.
            // Plugins that don't implement a particular export will
            // trap when called; the closure logs the trap and falls
            // back to a safe default (Continue for action hooks,
            // no-op for observer hooks).
            register_wasm_hooks(&mut self.hooks, Arc::clone(&instance), &resources.rt_handle);

            tracing::info!(
                plugin = %name,
                version = %d.manifest.version,
                tools = tool_count,
                "Loaded WASM plugin"
            );
            self.loaded.push(LoadedPlugin {
                manifest: d.manifest,
                tool_count,
            });
        }

        // Surface unmatched names: user listed a plugin that isn't
        // installed (matches the `load_bundled` behaviour above).
        let loaded_names: HashSet<&str> =
            self.loaded.iter().map(|p| p.manifest.name.as_str()).collect();
        for requested in &want {
            if !loaded_names.contains(requested) {
                tracing::warn!(
                    plugin = %requested,
                    "Plugin requested in [plugins].enabled but not found among bundled or WASM plugins"
                );
            }
        }

        Ok(())
    }

    /// Diagnostic snapshot of every successfully-loaded plugin.
    pub fn loaded(&self) -> &[LoadedPlugin] {
        &self.loaded
    }

    /// Drain the collected tools out of the registry into a Vec the
    /// agent builder can consume. Consumes `self`.
    pub fn into_tools(self) -> Vec<Box<dyn Tool>> {
        self.tools
    }

    /// Split the registry into the things the agent builder
    /// actually needs: the tool list + the [`HookRegistry`].
    /// Preferred over `into_tools()` when the caller also wants
    /// lifecycle hooks (which is everyone except a couple of
    /// internal tests).
    pub fn into_tools_and_hooks(self) -> (Vec<Box<dyn Tool>>, HookRegistry) {
        (self.tools, self.hooks)
    }
}

/// Register lifecycle hooks that route into a single WASM plugin
/// instance.
///
/// Each callback is a sync `Fn` (the `HookRegistry` shape) that
/// drives an async wasmtime call to completion via
/// `block_in_place + block_on`. If the plugin doesn't implement the
/// matching export, wasmtime traps; the trap is caught, logged, and
/// the hook falls back to a safe default (Continue for action
/// hooks, observer hooks become no-ops).
fn register_wasm_hooks(
    registry: &mut HookRegistry,
    instance: Arc<WasmPluginInstance>,
    rt_handle: &Handle,
) {
    // Plugin name captured for log context. The instance carries it
    // already but closures can't easily move into generic fn args.
    let plugin_name = instance.plugin_name.clone();

    // ---- pre_tool_call ----
    {
        let inst = Arc::clone(&instance);
        let rt = rt_handle.clone();
        let name = plugin_name.clone();
        registry.register_pre_tool(Arc::new(move |event| {
            let args_json = event.args.to_string();
            let result = tokio::task::block_in_place(|| {
                rt.block_on(inst.call_on_pre_tool_call(event.tool_name, &args_json))
            });
            match result {
                Ok(action) => action,
                Err(e) => {
                    tracing::warn!(
                        plugin = %name,
                        tool = %event.tool_name,
                        "WASM pre_tool_call hook failed: {e}; treating as Continue"
                    );
                    crate::plugins::PreToolCallAction::Continue
                }
            }
        }));
    }

    // ---- post_tool_call ----
    {
        let inst = Arc::clone(&instance);
        let rt = rt_handle.clone();
        let name = plugin_name.clone();
        registry.register_post_tool(Arc::new(move |event| {
            let args_json = event.args.to_string();
            let result = tokio::task::block_in_place(|| {
                rt.block_on(inst.call_on_post_tool_call(
                    event.tool_name,
                    &args_json,
                    event.output,
                    event.success,
                ))
            });
            match result {
                Ok(action) => action,
                Err(e) => {
                    tracing::warn!(
                        plugin = %name,
                        tool = %event.tool_name,
                        "WASM post_tool_call hook failed: {e}; treating as Continue"
                    );
                    crate::plugins::PostToolCallAction::Continue
                }
            }
        }));
    }

    // ---- pre_llm_call (observer) ----
    {
        let inst = Arc::clone(&instance);
        let rt = rt_handle.clone();
        let name = plugin_name.clone();
        registry.register_pre_llm(Arc::new(move |event| {
            let result = tokio::task::block_in_place(|| {
                rt.block_on(inst.call_on_pre_llm_call(event.messages_json))
            });
            if let Err(e) = result {
                tracing::warn!(
                    plugin = %name,
                    "WASM pre_llm_call hook failed: {e}"
                );
            }
        }));
    }

    // ---- post_llm_call (observer) ----
    {
        let inst = Arc::clone(&instance);
        let rt = rt_handle.clone();
        let name = plugin_name.clone();
        registry.register_post_llm(Arc::new(move |event| {
            let result = tokio::task::block_in_place(|| {
                rt.block_on(inst.call_on_post_llm_call(event.response_json))
            });
            if let Err(e) = result {
                tracing::warn!(
                    plugin = %name,
                    "WASM post_llm_call hook failed: {e}"
                );
            }
        }));
    }

    // ---- on_session_start (observer) ----
    {
        let inst = Arc::clone(&instance);
        let rt = rt_handle.clone();
        let name = plugin_name.clone();
        registry.register_on_session_start(Arc::new(move |event| {
            let result = tokio::task::block_in_place(|| {
                rt.block_on(inst.call_on_session_start(event.session_id))
            });
            if let Err(e) = result {
                tracing::warn!(
                    plugin = %name,
                    "WASM on_session_start hook failed: {e}"
                );
            }
        }));
    }

    // ---- on_session_end (observer) ----
    {
        let inst = Arc::clone(&instance);
        let rt = rt_handle.clone();
        let name = plugin_name.clone();
        registry.register_on_session_end(Arc::new(move |event| {
            let result = tokio::task::block_in_place(|| {
                rt.block_on(inst.call_on_session_end(event.session_id))
            });
            if let Err(e) = result {
                tracing::warn!(
                    plugin = %name,
                    "WASM on_session_end hook failed: {e}"
                );
            }
        }));
    }
}

/// Bundle of host-side resources passed into the WASM loader.
///
/// Each WASM plugin receives a clone of these handles in its
/// [`PluginHostState`]. The bundle exists so callers can pass one
/// argument instead of five and so future host-import additions
/// only require extending this struct.
pub struct WasmHostResources {
    pub path_sandbox: Arc<PathSandbox>,
    pub memory: Arc<dyn Memory>,
    pub http_client: reqwest::Client,
    pub rt_handle: Handle,
    /// Per-plugin string settings, keyed by plugin name.
    /// Each plugin sees only its own slice; the registry slices the
    /// map at instantiation time and hands a per-plugin `HashMap`
    /// to that plugin's host state.
    pub settings: HashMap<String, HashMap<String, String>>,
    /// Optional message bus for outbound channel sends. `None` in
    /// CLI / agent mode where there are no channels; the
    /// `channel-send` host import returns an error in that case.
    pub bus: Option<MessageBus>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calling `load_bundled` with an empty allowlist must be a no-op
    /// even when bundled plugins exist in inventory. This is the
    /// default-config path; if it ever activated something we'd be
    /// changing behaviour for every existing install.
    #[test]
    fn empty_enabled_list_loads_nothing() {
        let mut reg = PluginRegistry::new();
        reg.load_bundled(&[]).unwrap();
        assert!(reg.loaded.is_empty());
        assert!(reg.tools.is_empty());
    }

    /// Asking for a plugin name that doesn't exist should not error
    /// (we don't want a typo in config to abort startup) but should
    /// leave nothing loaded.
    #[test]
    fn unknown_plugin_name_loads_nothing() {
        let mut reg = PluginRegistry::new();
        reg.load_bundled(&["definitely-not-a-real-plugin-name".to_string()])
            .unwrap();
        assert!(reg.loaded.is_empty());
    }

    /// Loading the bundled `echo-demo` plugin should produce one
    /// loaded entry and one registered tool (the `echo` tool). This
    /// is the round-trip smoke test of the entire scaffold:
    /// inventory submission → validation → register → into_tools.
    #[test]
    fn echo_demo_round_trips() {
        let mut reg = PluginRegistry::new();
        reg.load_bundled(&["echo-demo".to_string()]).unwrap();
        assert_eq!(reg.loaded.len(), 1, "expected 1 loaded plugin");
        let lp = &reg.loaded[0];
        assert_eq!(lp.manifest.name, "echo-demo");
        assert_eq!(lp.tool_count, 1);
        let tools = reg.into_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "echo");
    }
}
