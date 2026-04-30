//! Plugin metadata.
//!
//! For bundled plugins, the manifest is returned by the
//! [`Plugin::manifest`](super::Plugin::manifest) trait method. For
//! WASM plugins (later phase) the same shape will be parsed from a
//! `plugin.toml` next to the `.wasm` file on disk.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// What kind of role a plugin fills.
///
/// Matches Hermes' three categories so that the same conceptual model
/// works for both bundled and (later) WASM plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// Adds tools / hooks of its own. Opt-in via `[plugins].enabled`.
    Standalone,
    /// Pluggable backend for an existing core feature (e.g. a different
    /// image-generation provider, a different embedding source). Bundled
    /// backends auto-load when the relevant feature is enabled; user
    /// backends still respect `[plugins].enabled`.
    Backend,
    /// Category with exactly one active provider at a time (memory
    /// providers are the canonical example). Selection is via a
    /// `<category>.provider` config key, not via `[plugins].enabled`.
    Exclusive,
}

impl PluginKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PluginKind::Standalone => "standalone",
            PluginKind::Backend => "backend",
            PluginKind::Exclusive => "exclusive",
        }
    }
}

/// Static metadata for a plugin.
///
/// For bundled plugins, instances are constructed inside
/// [`Plugin::manifest`](super::Plugin::manifest). For WASM plugins
/// (later phase) instances will be deserialised from `plugin.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Stable identifier. ASCII alphanumeric plus `-` and `_`,
    /// 1-64 chars. This is the value users put in
    /// `[plugins].enabled`. Treated as case-sensitive.
    pub name: String,
    /// Free-form version string. Convention is semver
    /// (`MAJOR.MINOR.PATCH`) but not enforced. Surfaced in diagnostic
    /// output and in `fennec doctor`.
    pub version: String,
    /// One-line human-readable description of what the plugin does.
    /// Surfaced in `fennec doctor` listings.
    #[serde(default)]
    pub description: String,
    /// Plugin author. Free-form; surfaced in diagnostics.
    #[serde(default)]
    pub author: String,
    /// Role this plugin fills. See [`PluginKind`].
    #[serde(default = "default_kind")]
    pub kind: PluginKind,
}

fn default_kind() -> PluginKind {
    PluginKind::Standalone
}

impl PluginManifest {
    /// Construct a fresh standalone manifest with the given name and
    /// version. Use the `with_*` builder methods to fill in the
    /// optional fields.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            description: String::new(),
            author: String::new(),
            kind: PluginKind::Standalone,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = author.into();
        self
    }

    pub fn with_kind(mut self, kind: PluginKind) -> Self {
        self.kind = kind;
        self
    }

    /// Validate that the manifest is well-formed.
    ///
    /// Validation runs at registry-load time. Failure rejects the
    /// plugin (with an error log) but does not abort agent startup.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name)?;
        if self.version.trim().is_empty() {
            bail!("plugin '{}': version is empty", self.name);
        }
        if self.version.len() > 64 {
            bail!(
                "plugin '{}': version too long ({} chars; max 64)",
                self.name,
                self.version.len()
            );
        }
        Ok(())
    }
}

/// ASCII alphanumeric plus `-` and `_`, 1-64 chars. Same constraints
/// as profile names (`config::schema::validate_profile_name`) for the
/// same reasons: it's a path component (the WASM-plugin loader will
/// resolve `~/.fennec/plugins/<name>/`), it shows up in shell history
/// and config files, and it must not allow path-traversal or quoting
/// hazards.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("plugin name cannot be empty");
    }
    if name.len() > 64 {
        bail!(
            "plugin name '{}' is too long ({} chars; max 64)",
            name,
            name.len()
        );
    }
    for (i, ch) in name.chars().enumerate() {
        let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_';
        if !ok {
            bail!(
                "plugin name '{}' contains invalid character '{}' at position {}; \
                 allowed: ASCII letters, digits, '-', '_'",
                name,
                ch,
                i
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_simple_name() {
        let m = PluginManifest::new("echo-demo", "0.1.0");
        m.validate().unwrap();
    }

    #[test]
    fn rejects_empty_name() {
        let m = PluginManifest::new("", "0.1.0");
        let err = m.validate().unwrap_err().to_string();
        assert!(err.contains("empty"), "expected 'empty' in: {}", err);
    }

    #[test]
    fn rejects_path_traversal() {
        for bad in ["..", "../../etc", "foo/bar", "/absolute"] {
            let m = PluginManifest::new(bad, "0.1.0");
            assert!(
                m.validate().is_err(),
                "expected '{}' to be rejected",
                bad
            );
        }
    }

    #[test]
    fn rejects_overlong_name() {
        let long = "a".repeat(65);
        let m = PluginManifest::new(long, "0.1.0");
        assert!(m.validate().is_err());
        let limit = "a".repeat(64);
        let m = PluginManifest::new(limit, "0.1.0");
        m.validate().unwrap();
    }

    #[test]
    fn rejects_empty_version() {
        let m = PluginManifest::new("ok", "");
        assert!(m.validate().is_err());
    }

    #[test]
    fn builder_methods_populate_fields() {
        let m = PluginManifest::new("p", "0.1.0")
            .with_description("desc")
            .with_author("me")
            .with_kind(PluginKind::Backend);
        assert_eq!(m.description, "desc");
        assert_eq!(m.author, "me");
        assert_eq!(m.kind, PluginKind::Backend);
    }

    #[test]
    fn manifest_round_trips_toml() {
        // Important for the WASM plugin loader (later phase) which will
        // read manifests from plugin.toml on disk.
        let m = PluginManifest::new("p", "0.1.0")
            .with_description("d")
            .with_kind(PluginKind::Standalone);
        let s = toml::to_string(&m).unwrap();
        let back: PluginManifest = toml::from_str(&s).unwrap();
        assert_eq!(back.name, "p");
        assert_eq!(back.version, "0.1.0");
        assert_eq!(back.description, "d");
        assert_eq!(back.kind, PluginKind::Standalone);
    }
}
