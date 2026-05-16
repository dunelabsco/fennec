//! The [`Plugin`] trait and its inventory entry shape.

use anyhow::Result;

use super::cli::CliCommandSpec;
use super::context::PluginContext;
use super::manifest::PluginManifest;

/// A unit of code that extends Fennec.
///
/// Implementations are typically zero-sized unit structs whose
/// `register` method calls [`PluginContext::register_tool`] (or, in
/// later phases, hook-registration / channel-registration methods)
/// to install their contributions.
///
/// `Plugin` impls MUST be `Send + Sync + 'static` because the
/// inventory entry holds a `&'static dyn Plugin` reference.
pub trait Plugin: Send + Sync + 'static {
    /// Return the static metadata for this plugin.
    ///
    /// The manifest is queried by the registry for two reasons:
    ///
    /// 1. To determine whether the plugin is on the `[plugins].enabled`
    ///    allowlist (matching is by `manifest.name`).
    /// 2. For diagnostic output (`fennec doctor`, log lines on
    ///    activation) so operators can tell what's loaded.
    fn manifest(&self) -> PluginManifest;

    /// Register this plugin's contributions into the given context.
    ///
    /// Called exactly once per session, before the agent starts. Any
    /// error returned here aborts plugin activation but does NOT abort
    /// agent startup — the registry logs the error and proceeds with
    /// the rest of the plugins. One broken plugin should not bring
    /// down the agent.
    fn register(&self, ctx: &mut PluginContext) -> Result<()>;

    /// Static list of CLI subcommands this plugin contributes to
    /// the `fennec` binary. Default empty.
    ///
    /// Called at startup, BEFORE clap parses argv, so that plugin
    /// commands appear in `fennec --help` and parse correctly. The
    /// trait method runs against a static `&self` reference — no
    /// plugin instantiation, no agent build. Plugins that want to
    /// add CLI subcommands must:
    ///
    /// 1. Return one or more [`CliCommandSpec`]s here.
    /// 2. Register a handler closure for each name via
    ///    [`PluginContext::register_cli_command`] inside the regular
    ///    [`Self::register`] call. The two are correlated by name.
    ///
    /// Names returned here that don't have a matching handler at
    /// dispatch time produce a runtime error pointing at the
    /// missing closure — this catches typos.
    fn cli_commands(&self) -> Vec<CliCommandSpec> {
        Vec::new()
    }
}

/// The inventory entry that bundles a static plugin reference for
/// compile-time registration.
///
/// Bundled plugins call [`inventory::submit!`] with one of these at
/// module scope. The registry iterates all entries via
/// [`inventory::iter`] at startup.
pub struct PluginEntry {
    /// A `'static` reference to a [`Plugin`] implementation. Typically
    /// a shared reference to a unit struct, e.g. `&MyPlugin`.
    pub plugin: &'static dyn Plugin,
}

inventory::collect!(PluginEntry);
