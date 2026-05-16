//! Fennec TUI palette — desert-fox warm theme.
//!
//! Colors as `Color::Rgb(...)` so truecolor terminals render the
//! intended shades; older terminals (Apple Terminal pre-Sequoia,
//! basic xterm) fall back to the closest 256-color match, which
//! is visually ~95% identical for these specific values.
//!
//! The palette is deliberately warm — sand-gold accents, antique
//! cream text on a dusk-warm-dark background, terracotta for
//! errors. This gives the TUI a distinct identity from the
//! generic-cyan-and-pink palette of most dev tools.

use ratatui::style::Color;

/// Deep dusk-warm-dark — slight brown undertone, not pure black.
/// Background everywhere.
pub const BG_DUSK: Color = Color::Rgb(0x0F, 0x0D, 0x0A);

/// Warm cream / antique white. Body text everywhere; easier on
/// the eyes than pure white against the warm background.
pub const TEXT_CREAM: Color = Color::Rgb(0xF0, 0xE8, 0xD6);

/// Warm sand-gold. Primary accent: panel headers, session-source
/// badges, the bot's speaker label. Like fennec fur in golden
/// hour.
pub const SAND_GOLD: Color = Color::Rgb(0xE8, 0xC7, 0x7B);

/// Warm amber. Secondary accent: the "you" speaker, hotkey
/// indicators (`[f]`, `[s]`, etc.), unread badges.
pub const AMBER: Color = Color::Rgb(0xF5, 0xB6, 0x42);

/// Tool-call magenta. Distinct from the warm palette so tool
/// invocations pop visually. Used for `▸ tool · ...` lines and
/// the TOOL LIVE panel border.
pub const TOOL_PINK: Color = Color::Rgb(0xD8, 0x7C, 0xAB);

/// Subdued warm gray with brown undertone. Metadata, timestamps,
/// dim labels.
pub const SUBDUED: Color = Color::Rgb(0x8A, 0x7E, 0x6B);

/// Deep terracotta / rust red. Errors, alerts. Easier on the eye
/// than pure bright red, and stays in the warm palette.
pub const TERRACOTTA: Color = Color::Rgb(0xC7, 0x5A, 0x4A);

/// Warm muted green. Success indicators, "connected" dots, ✓
/// checkmarks. More olive than the typical bright terminal
/// green — fits the warm palette.
pub const MUTED_GREEN: Color = Color::Rgb(0x7F, 0xA8, 0x6B);

/// Background for the bottom status / shortcut bar — slightly
/// warmer/lighter than the dusk so the bar visually separates
/// from the panel area.
pub const SHORTCUT_BG: Color = Color::Rgb(0x1A, 0x16, 0x10);

/// Panel border — dim warm gray, doesn't compete with content.
pub const PANEL_BORDER: Color = Color::Rgb(0x55, 0x4B, 0x3C);

/// Highlighted-row background (currently selected session, etc.).
/// One stop brighter than the dusk so the row stands out.
pub const HIGHLIGHT_BG: Color = Color::Rgb(0x3D, 0x32, 0x22);
