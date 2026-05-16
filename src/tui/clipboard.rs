//! Clipboard helpers for `/copy` and `/paste`.
//!
//! Wraps `arboard` for native clipboard read/write and provides
//! an OSC52 fallback so SSH sessions or sandboxed terminals
//! still get text into the user's host clipboard. Image-paste
//! (`arboard::Clipboard::get_image`) is exposed as raw RGBA so
//! the caller can re-encode to PNG before persisting.
//!
//! Mirrors Hermes' clipboard plumbing
//! (`tui_gateway/server.py:3321-3358` for image paste,
//! `ui-tui/src/app/slash/commands/core/core.ts:325-372` for
//! /copy with OSC52 fallback).

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

/// Write `text` to the OS clipboard via arboard.
pub fn write_native(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| anyhow!("clipboard unavailable: {e}"))?;
    cb.set_text(text)
        .map_err(|e| anyhow!("clipboard set_text failed: {e}"))?;
    Ok(())
}

/// Build the OSC 52 escape sequence that asks the *terminal*
/// (not the local clipboard daemon) to put `text` on the host
/// clipboard. Useful when arboard fails — typically inside
/// SSH sessions or under terminals without X/Wayland access.
///
/// The caller is responsible for writing the returned string
/// to a terminal stream (stdout / stderr). We build but don't
/// emit so the renderer can decide where to send it.
pub fn osc52_payload(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", B64.encode(text.as_bytes()))
}

/// Try arboard first; if it fails, fall back to printing the
/// OSC52 sequence to stdout. Returns a tag describing which
/// path succeeded so the chat can confirm "copied" vs
/// "OSC52 sent".
pub fn write_with_fallback(text: &str) -> CopyResult {
    match write_native(text) {
        Ok(()) => CopyResult::Native,
        Err(_native_err) => {
            let payload = osc52_payload(text);
            // Best-effort write to stdout. If it fails, the user
            // sees no copy — return that explicitly.
            use std::io::Write;
            let mut stdout = std::io::stdout();
            if stdout.write_all(payload.as_bytes()).is_ok()
                && stdout.flush().is_ok()
            {
                CopyResult::Osc52
            } else {
                CopyResult::Failed
            }
        }
    }
}

/// Outcome of [`write_with_fallback`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyResult {
    /// Native clipboard accepted the text.
    Native,
    /// arboard rejected the write; the OSC52 escape was sent
    /// to stdout instead. The host terminal may or may not have
    /// honoured it depending on its config.
    Osc52,
    /// Both paths failed. The text isn't on the clipboard.
    Failed,
}

/// Read an image from the OS clipboard. Returns the raw RGBA
/// bytes plus dimensions; the caller is expected to encode to
/// PNG (see `image::save_buffer_with_format`) before persisting
/// for the `/paste` flow.
pub fn read_image_rgba() -> Result<ClipboardImage> {
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| anyhow!("clipboard unavailable: {e}"))?;
    let img = cb
        .get_image()
        .map_err(|e| anyhow!("no image in clipboard ({e})"))?;
    Ok(ClipboardImage {
        width: img.width as u32,
        height: img.height as u32,
        rgba: img.bytes.into_owned(),
    })
}

/// Raw RGBA image read from the OS clipboard.
pub struct ClipboardImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ClipboardImage {
    /// Encode the RGBA buffer as PNG and write it to `path`,
    /// then return the path so the caller can hand it to the
    /// existing `/image` attachment loader. Uses the `image`
    /// crate's PNG encoder which is already a transitive dep.
    pub fn write_png(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        // `image` 0.25 expects a Cursor / writer + image::ImageFormat.
        let buf = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .ok_or_else(|| anyhow!("clipboard image: invalid RGBA buffer"))?;
        buf.save_with_format(path, image::ImageFormat::Png)
            .with_context(|| format!("writing PNG to {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_payload_is_well_formed() {
        let p = osc52_payload("hi");
        assert!(p.starts_with("\x1b]52;c;"));
        assert!(p.ends_with("\x07"));
        // Body is the base64 of "hi" = "aGk=".
        assert!(p.contains("aGk="));
    }

    // Native clipboard / image-read tests would require a real
    // display server; skipping in CI. The encoder path is
    // exercised by /paste integration in `cargo run -- agent
    // --tui` with a real clipboard image.
}
