//! On-disk discovery of WASM plugins.
//!
//! Walks `~/.fennec/plugins/` (or the equivalent under a profile
//! directory) and finds every subdirectory containing a `plugin.toml`
//! manifest plus a `<name>.wasm` component. Returns the parsed
//! manifests so the registry can apply the `[plugins].enabled`
//! allowlist before paying the wasmtime compilation cost.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::plugins::manifest::PluginManifest;

/// One plugin found on disk.
#[derive(Debug, Clone)]
pub struct DiscoveredWasmPlugin {
    /// Parsed manifest (already validated).
    pub manifest: PluginManifest,
    /// Path to the `.wasm` component file.
    pub wasm_path: PathBuf,
    /// Path to the `plugin.toml` (kept for diagnostic logging).
    pub manifest_path: PathBuf,
}

/// Walk `plugins_root` looking for plugin directories. Returns one
/// entry per valid `<name>/plugin.toml` + `<name>/<name>.wasm` pair.
///
/// Failure to read the directory is not an error — it usually means
/// no plugins are installed yet. Failure to parse a single
/// `plugin.toml` is also not fatal: that one plugin is dropped with
/// a warn log and the rest continue.
pub fn discover_wasm_plugins(plugins_root: &Path) -> Result<Vec<DiscoveredWasmPlugin>> {
    let mut found = Vec::new();

    let entries = match std::fs::read_dir(plugins_root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                "WASM plugin loader: {} does not exist; no plugins to load",
                plugins_root.display()
            );
            return Ok(found);
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!("reading plugin root {}", plugins_root.display())
            });
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Skipping unreadable plugin dir entry: {e}");
                continue;
            }
        };

        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        let manifest_path = dir.join("plugin.toml");
        if !manifest_path.exists() {
            // Not every directory has to be a plugin — operators may
            // use `~/.fennec/plugins/.cache/` or similar for staging.
            continue;
        }

        let manifest = match read_and_validate_manifest(&manifest_path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "Skipping plugin at {}: manifest invalid ({e})",
                    dir.display()
                );
                continue;
            }
        };

        // Convention: the `.wasm` component is named after the
        // plugin's manifest name.
        let wasm_path = dir.join(format!("{}.wasm", manifest.name));
        if !wasm_path.exists() {
            tracing::warn!(
                "Skipping plugin '{}': expected component at {} but file is missing",
                manifest.name,
                wasm_path.display()
            );
            continue;
        }

        found.push(DiscoveredWasmPlugin {
            manifest,
            wasm_path,
            manifest_path,
        });
    }

    Ok(found)
}

/// Parse a `plugin.toml` file and run manifest validation.
fn read_and_validate_manifest(path: &Path) -> Result<PluginManifest> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let manifest: PluginManifest =
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    manifest.validate()?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    /// Discovery on a non-existent root should be a clean no-op, not
    /// an error.
    #[test]
    fn discovery_nonexistent_root() {
        let path = std::path::Path::new("/definitely-does-not-exist-fennec-test-12345");
        let found = discover_wasm_plugins(path).unwrap();
        assert!(found.is_empty());
    }

    /// Discovery on an empty root should return an empty list.
    #[test]
    fn discovery_empty_root() {
        let tmp = TempDir::new().unwrap();
        let found = discover_wasm_plugins(tmp.path()).unwrap();
        assert!(found.is_empty());
    }

    /// A valid plugin directory must produce one discovery entry.
    /// The `.wasm` file's contents don't matter for discovery — we
    /// only check that the file exists; compilation happens later.
    #[test]
    fn discovers_valid_plugin() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("hello");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"name = "hello"
version = "0.1.0"
description = "test"
"#,
        )
        .unwrap();
        fs::write(plugin_dir.join("hello.wasm"), b"\x00asm\x01\x00\x00\x00").unwrap();

        let found = discover_wasm_plugins(tmp.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.name, "hello");
    }

    /// A plugin directory missing `<name>.wasm` is dropped with a
    /// warning but doesn't abort discovery for other plugins.
    #[test]
    fn skips_missing_wasm_file() {
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("broken");
        fs::create_dir_all(&bad).unwrap();
        fs::write(
            bad.join("plugin.toml"),
            r#"name = "broken"
version = "0.1.0"
"#,
        )
        .unwrap();
        // Note: no broken.wasm

        let good = tmp.path().join("ok");
        fs::create_dir_all(&good).unwrap();
        fs::write(
            good.join("plugin.toml"),
            r#"name = "ok"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::write(good.join("ok.wasm"), b"\x00asm\x01\x00\x00\x00").unwrap();

        let found = discover_wasm_plugins(tmp.path()).unwrap();
        // Only "ok" is included; "broken" is skipped.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.name, "ok");
    }

    /// A directory without `plugin.toml` is silently ignored
    /// (operators may stash unrelated files under plugins/).
    #[test]
    fn ignores_directory_without_manifest() {
        let tmp = TempDir::new().unwrap();
        let staging = tmp.path().join(".cache");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("random.txt"), b"hello").unwrap();

        let found = discover_wasm_plugins(tmp.path()).unwrap();
        assert!(found.is_empty());
    }

    /// An invalid `plugin.toml` (e.g. name fails validation) is
    /// dropped with a warn but doesn't abort the loader.
    #[test]
    fn skips_invalid_manifest() {
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("badname");
        fs::create_dir_all(&bad).unwrap();
        // "../escape" fails name validation (path traversal).
        fs::write(
            bad.join("plugin.toml"),
            r#"name = "../escape"
version = "0.1.0"
"#,
        )
        .unwrap();

        let found = discover_wasm_plugins(tmp.path()).unwrap();
        assert!(found.is_empty());
    }
}
