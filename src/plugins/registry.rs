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

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tokio::runtime::Handle;

use crate::memory::traits::Memory;
use crate::security::path_sandbox::PathSandbox;
use crate::tools::traits::Tool;

use super::context::PluginContext;
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
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            loaded: Vec::new(),
            tools: Vec::new(),
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
                    let tools = ctx.into_tools();
                    let tool_count = tools.len();
                    tracing::info!(
                        plugin = %manifest.name,
                        version = %manifest.version,
                        tools = tool_count,
                        "Loaded bundled plugin"
                    );
                    self.tools.extend(tools);
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

            // Instantiate with a fresh host state per plugin.
            let state = PluginHostState {
                plugin_name: name.clone(),
                path_sandbox: Arc::clone(&resources.path_sandbox),
                memory: Arc::clone(&resources.memory),
                http_client: resources.http_client.clone(),
                rt_handle: resources.rt_handle.clone(),
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
            // but call_register awaits a Mutex.
            let specs = match resources
                .rt_handle
                .block_on(instance.call_register())
            {
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
