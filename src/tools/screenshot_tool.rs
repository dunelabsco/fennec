//! Screenshot tool — captures the screen using the OS-native utility.
//!
//! Picks the best available binary:
//!   - macOS: `screencapture -x <path>`
//!   - Linux: first found among `gnome-screenshot -f <path>`, `scrot <path>`,
//!            `import -window root <path>` (ImageMagick), `grim <path>` (Wayland).
//!   - Windows / unknown: returns a clear error.
//!
//! Saves PNGs to `<home>/screenshots/<timestamp>.png` by default. The
//! returned path plays nicely with `vision_describe` — agent can chain the
//! two to "screenshot my desktop and tell me what's on it."

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use super::traits::{Tool, ToolResult};

pub fn default_screenshot_dir(fennec_home: &Path) -> PathBuf {
    fennec_home.join("screenshots")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotBackend {
    MacScreencapture,
    GnomeScreenshot,
    Scrot,
    ImImport,
    Grim,
    None,
}

impl ScreenshotBackend {
    pub fn binary(&self) -> Option<&'static str> {
        match self {
            Self::MacScreencapture => Some("screencapture"),
            Self::GnomeScreenshot => Some("gnome-screenshot"),
            Self::Scrot => Some("scrot"),
            Self::ImImport => Some("import"),
            Self::Grim => Some("grim"),
            Self::None => None,
        }
    }

    /// Build the argv for this backend given an output file path.
    pub fn argv(&self, path: &Path) -> Vec<String> {
        let p = path.to_string_lossy().to_string();
        match self {
            Self::MacScreencapture => vec!["-x".to_string(), p],
            Self::GnomeScreenshot => vec!["-f".to_string(), p],
            Self::Scrot => vec![p],
            Self::ImImport => vec!["-window".to_string(), "root".to_string(), p],
            Self::Grim => vec![p],
            Self::None => vec![],
        }
    }
}

/// Probe the system for a screenshot backend.
///
/// On macOS, always prefer `screencapture` (preinstalled).
/// On Linux, take the first available of the known backends.
/// Otherwise, return None.
pub fn detect_backend() -> ScreenshotBackend {
    #[cfg(target_os = "macos")]
    {
        if which_exists("screencapture") {
            return ScreenshotBackend::MacScreencapture;
        }
    }
    #[cfg(target_os = "linux")]
    {
        for b in [
            ScreenshotBackend::GnomeScreenshot,
            ScreenshotBackend::Scrot,
            ScreenshotBackend::ImImport,
            ScreenshotBackend::Grim,
        ] {
            if let Some(bin) = b.binary() {
                if which_exists(bin) {
                    return b;
                }
            }
        }
    }
    ScreenshotBackend::None
}

fn which_exists(binary: &str) -> bool {
    let paths = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    std::env::split_paths(&paths).any(|d| d.join(binary).is_file())
}

pub struct ScreenshotTool {
    output_dir: PathBuf,
    backend: ScreenshotBackend,
}

impl ScreenshotTool {
    pub fn new(output_dir: PathBuf) -> Self {
        Self {
            output_dir,
            backend: detect_backend(),
        }
    }

    /// Test-only constructor that lets us pin a backend regardless of host.
    #[cfg(test)]
    pub fn new_with_backend(output_dir: PathBuf, backend: ScreenshotBackend) -> Self {
        Self { output_dir, backend }
    }
}

#[async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }

    fn description(&self) -> &str {
        "Capture the screen as a PNG. Saves to disk and returns the file \
         path. Chain with vision_describe to 'look' at the screen. macOS \
         uses screencapture; Linux uses gnome-screenshot/scrot/import/grim. \
         Fails with a clear message if no backend is available."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: Value) -> Result<ToolResult> {
        let binary = match self.backend.binary() {
            Some(b) => b,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "no screenshot backend available on this host (need screencapture on macOS; \
                         gnome-screenshot, scrot, import, or grim on Linux)"
                    )),
                });
            }
        };

        if let Err(e) = tokio::fs::create_dir_all(&self.output_dir).await {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("failed to create output dir: {}", e)),
            });
        }

        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let path = self.output_dir.join(format!("screenshot_{}.png", ts));
        let argv = self.backend.argv(&path);

        let status = match Command::new(binary)
            .args(&argv)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .await
        {
            Ok(s) => s,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to spawn {}: {}", binary, e)),
                });
            }
        };

        if !status.success() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "{} exited with status {} (may need screen-recording permission)",
                    binary, status
                )),
            });
        }

        if !path.exists() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "{} completed but no file at {} — may have been cancelled",
                    binary,
                    path.display()
                )),
            });
        }

        Ok(ToolResult {
            success: true,
            output: format!("Screenshot saved to {}", path.display()),
            error: None,
        })
    }

    fn is_read_only(&self) -> bool {
        // Technically writes a file, but intent is observational.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_binary_names() {
        assert_eq!(ScreenshotBackend::MacScreencapture.binary(), Some("screencapture"));
        assert_eq!(ScreenshotBackend::GnomeScreenshot.binary(), Some("gnome-screenshot"));
        assert_eq!(ScreenshotBackend::Scrot.binary(), Some("scrot"));
        assert_eq!(ScreenshotBackend::ImImport.binary(), Some("import"));
        assert_eq!(ScreenshotBackend::Grim.binary(), Some("grim"));
        assert_eq!(ScreenshotBackend::None.binary(), None);
    }

    #[test]
    fn argv_mac_screencapture() {
        let argv = ScreenshotBackend::MacScreencapture.argv(Path::new("/tmp/x.png"));
        assert_eq!(argv, vec!["-x".to_string(), "/tmp/x.png".to_string()]);
    }

    #[test]
    fn argv_gnome_screenshot() {
        let argv = ScreenshotBackend::GnomeScreenshot.argv(Path::new("/tmp/x.png"));
        assert_eq!(argv, vec!["-f".to_string(), "/tmp/x.png".to_string()]);
    }

    #[test]
    fn argv_scrot() {
        let argv = ScreenshotBackend::Scrot.argv(Path::new("/tmp/x.png"));
        assert_eq!(argv, vec!["/tmp/x.png".to_string()]);
    }

    #[test]
    fn argv_im_import() {
        let argv = ScreenshotBackend::ImImport.argv(Path::new("/tmp/x.png"));
        assert_eq!(
            argv,
            vec!["-window".to_string(), "root".to_string(), "/tmp/x.png".to_string()]
        );
    }

    #[test]
    fn argv_grim() {
        let argv = ScreenshotBackend::Grim.argv(Path::new("/tmp/x.png"));
        assert_eq!(argv, vec!["/tmp/x.png".to_string()]);
    }

    #[test]
    fn argv_none_is_empty() {
        let argv = ScreenshotBackend::None.argv(Path::new("/tmp/x.png"));
        assert!(argv.is_empty());
    }

    #[test]
    fn default_dir_under_fennec_home() {
        let p = default_screenshot_dir(Path::new("/home/user/.fennec"));
        assert_eq!(p, PathBuf::from("/home/user/.fennec/screenshots"));
    }

    #[tokio::test]
    async fn execute_returns_error_when_no_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let t = ScreenshotTool::new_with_backend(
            tmp.path().to_path_buf(),
            ScreenshotBackend::None,
        );
        let r = t.execute(json!({})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("no screenshot backend"));
    }

    #[test]
    fn which_exists_true_for_sh() {
        // `sh` is present on every supported Unix host.
        #[cfg(unix)]
        assert!(which_exists("sh"));
    }

    #[test]
    fn which_exists_false_for_made_up_binary() {
        assert!(!which_exists("fennec-made-up-binary-xyz-123"));
    }

    #[test]
    fn new_detects_real_backend() {
        // Verify the constructor runs and picks something sane on the host.
        let tmp = tempfile::tempdir().unwrap();
        let t = ScreenshotTool::new(tmp.path().to_path_buf());
        #[cfg(target_os = "macos")]
        assert_eq!(t.backend, ScreenshotBackend::MacScreencapture);
        // On Linux CI we might get None if nothing's installed; just assert
        // it doesn't panic and returns *some* variant.
        #[cfg(not(target_os = "macos"))]
        {
            let _ = t.backend;
        }
    }
}
