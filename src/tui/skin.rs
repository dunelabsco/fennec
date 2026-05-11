//! Skin (theme variant) system.
//!
//! Fennec's renderer reads colour values from [`App.skin`] rather
//! than the static constants in [`super::theme`]. The constants
//! still exist — they're the default ("fennec-warm") palette
//! values and the source of truth for that variant. Other
//! variants override individual slots.
//!
//! 4 bundled skins out of the box:
//!
//! - `fennec-warm` (default): the existing warm-fox palette
//! - `mono`: greyscale, accessibility / color-blind friendly
//! - `light`: light-terminal friendly (dark text on cream bg)
//! - `cool`: cool-blue alternative for users who prefer it
//!
//! Users can drop additional skins at `~/.fennec/skins/<name>.toml`
//! and select them via `/skin <name>`. Field-level fallback to
//! the default skin: a user TOML can override individual colours
//! and leave the rest at fennec-warm values.

use std::collections::HashMap;
use std::path::Path;

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use super::theme;

/// The full skin palette. Field names mirror [`super::theme`]'s
/// static constants so the mechanical refactor in the renderers
/// is a one-to-one substitution.
#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub bg_dusk: Color,
    pub text_cream: Color,
    pub sand_gold: Color,
    pub amber: Color,
    pub tool_pink: Color,
    pub subdued: Color,
    pub terracotta: Color,
    pub muted_green: Color,
    pub shortcut_bg: Color,
    pub panel_border: Color,
    pub highlight_bg: Color,
    /// Display name shown in `/skin status`. Not used for
    /// rendering — just metadata.
    pub name: &'static str,
}

impl Skin {
    /// The current warm-fox palette. Equivalent to leaving
    /// `/skin` unset.
    pub fn fennec_warm() -> Self {
        Self {
            bg_dusk: theme::BG_DUSK,
            text_cream: theme::TEXT_CREAM,
            sand_gold: theme::SAND_GOLD,
            amber: theme::AMBER,
            tool_pink: theme::TOOL_PINK,
            subdued: theme::SUBDUED,
            terracotta: theme::TERRACOTTA,
            muted_green: theme::MUTED_GREEN,
            shortcut_bg: theme::SHORTCUT_BG,
            panel_border: theme::PANEL_BORDER,
            highlight_bg: theme::HIGHLIGHT_BG,
            name: "fennec-warm",
        }
    }

    /// Greyscale variant — colour-blind / accessibility focused.
    /// Same warm dusk background; accents fall back to graduated
    /// off-whites + dim greys so the visual hierarchy still reads
    /// without colour.
    pub fn mono() -> Self {
        Self {
            bg_dusk: Color::Rgb(0x10, 0x10, 0x10),
            text_cream: Color::Rgb(0xE6, 0xE6, 0xE6),
            sand_gold: Color::Rgb(0xC8, 0xC8, 0xC8),
            amber: Color::Rgb(0xB0, 0xB0, 0xB0),
            tool_pink: Color::Rgb(0xA0, 0xA0, 0xA0),
            subdued: Color::Rgb(0x70, 0x70, 0x70),
            terracotta: Color::Rgb(0xFF, 0xFF, 0xFF),
            muted_green: Color::Rgb(0x9B, 0x9B, 0x9B),
            shortcut_bg: Color::Rgb(0x18, 0x18, 0x18),
            panel_border: Color::Rgb(0x4A, 0x4A, 0x4A),
            highlight_bg: Color::Rgb(0x2A, 0x2A, 0x2A),
            name: "mono",
        }
    }

    /// Light-terminal variant — dark text on cream background.
    /// Mirrors a Solarized-light-ish balance while keeping
    /// Fennec's warm sand-gold accent for the speaker label.
    pub fn light() -> Self {
        Self {
            bg_dusk: Color::Rgb(0xFA, 0xF4, 0xE6),
            text_cream: Color::Rgb(0x2A, 0x24, 0x18),
            sand_gold: Color::Rgb(0x9B, 0x68, 0x1C),
            amber: Color::Rgb(0xB7, 0x69, 0x0E),
            tool_pink: Color::Rgb(0x96, 0x3E, 0x6F),
            subdued: Color::Rgb(0x6D, 0x60, 0x4F),
            terracotta: Color::Rgb(0x9F, 0x35, 0x25),
            muted_green: Color::Rgb(0x46, 0x6A, 0x33),
            shortcut_bg: Color::Rgb(0xF0, 0xE6, 0xD0),
            panel_border: Color::Rgb(0xB8, 0xA6, 0x88),
            highlight_bg: Color::Rgb(0xE8, 0xDB, 0xC0),
            name: "light",
        }
    }

    /// Cool-blue alternative. Keeps the dark background but
    /// swaps the warm-fox accents for ocean blues + seafoam.
    /// For users who want a colourful dark mode that isn't
    /// warm-toned.
    pub fn cool() -> Self {
        Self {
            bg_dusk: Color::Rgb(0x0A, 0x0E, 0x16),
            text_cream: Color::Rgb(0xD8, 0xE6, 0xF0),
            sand_gold: Color::Rgb(0x6B, 0xB6, 0xFF),
            amber: Color::Rgb(0x70, 0xC8, 0xE0),
            tool_pink: Color::Rgb(0x9F, 0x7A, 0xC8),
            subdued: Color::Rgb(0x6A, 0x7A, 0x8F),
            terracotta: Color::Rgb(0xE3, 0x6F, 0x6F),
            muted_green: Color::Rgb(0x7A, 0xC8, 0x9F),
            shortcut_bg: Color::Rgb(0x12, 0x18, 0x22),
            panel_border: Color::Rgb(0x3A, 0x4A, 0x5F),
            highlight_bg: Color::Rgb(0x1F, 0x2C, 0x3D),
            name: "cool",
        }
    }

    /// Look up a built-in by name. `None` means the name
    /// isn't one of the bundled variants — caller should try
    /// the user-skin loader before erroring.
    pub fn builtin(name: &str) -> Option<Self> {
        match name {
            "" | "fennec-warm" | "default" => Some(Self::fennec_warm()),
            "mono" => Some(Self::mono()),
            "light" => Some(Self::light()),
            "cool" => Some(Self::cool()),
            _ => None,
        }
    }

    /// Built-in variant names in registration order. Used by
    /// `/skin list`.
    pub fn builtin_names() -> &'static [&'static str] {
        &["fennec-warm", "mono", "light", "cool"]
    }

    /// Resolve a name to a `Skin`. Tries built-ins first, then
    /// `~/.fennec/skins/<name>.toml` (or whatever
    /// `fennec_home/skins/` resolves to). Returns
    /// `Some(Skin::fennec_warm())` on miss — never panics; the
    /// `/skin` command surfaces the miss explicitly.
    pub fn resolve(name: &str, fennec_home: &Path) -> Result<Self, SkinError> {
        if let Some(b) = Self::builtin(name) {
            return Ok(b);
        }
        let path = fennec_home.join("skins").join(format!("{name}.toml"));
        if !path.exists() {
            return Err(SkinError::NotFound(name.to_string()));
        }
        let body = std::fs::read_to_string(&path).map_err(|e| {
            SkinError::Io {
                path: path.clone(),
                source: e,
            }
        })?;
        let raw: RawSkin = toml::from_str(&body).map_err(|e| SkinError::Parse {
            path: path.clone(),
            source: e,
        })?;
        Ok(raw.into_skin(name))
    }

    /// Snapshot the field values into a small map for
    /// inspection / `/skin status`. Useful for tests too.
    pub fn fields(&self) -> HashMap<&'static str, Color> {
        let mut m = HashMap::new();
        m.insert("bg_dusk", self.bg_dusk);
        m.insert("text_cream", self.text_cream);
        m.insert("sand_gold", self.sand_gold);
        m.insert("amber", self.amber);
        m.insert("tool_pink", self.tool_pink);
        m.insert("subdued", self.subdued);
        m.insert("terracotta", self.terracotta);
        m.insert("muted_green", self.muted_green);
        m.insert("shortcut_bg", self.shortcut_bg);
        m.insert("panel_border", self.panel_border);
        m.insert("highlight_bg", self.highlight_bg);
        m
    }
}

impl Default for Skin {
    fn default() -> Self {
        Self::fennec_warm()
    }
}

/// Wire shape for user TOML files. Every colour is optional; an
/// omitted field inherits from the fennec-warm default. Strings
/// are hex `"#RRGGBB"` so users don't have to remember TOML's
/// 0x-syntax for ints.
#[derive(Debug, Default, Deserialize, Serialize)]
struct RawSkin {
    bg_dusk: Option<String>,
    text_cream: Option<String>,
    sand_gold: Option<String>,
    amber: Option<String>,
    tool_pink: Option<String>,
    subdued: Option<String>,
    terracotta: Option<String>,
    muted_green: Option<String>,
    shortcut_bg: Option<String>,
    panel_border: Option<String>,
    highlight_bg: Option<String>,
}

impl RawSkin {
    fn into_skin(self, name: &str) -> Skin {
        let base = Skin::fennec_warm();
        let pick = |hex: Option<String>, fallback: Color| match hex {
            Some(s) => parse_hex(&s).unwrap_or(fallback),
            None => fallback,
        };
        Skin {
            bg_dusk: pick(self.bg_dusk, base.bg_dusk),
            text_cream: pick(self.text_cream, base.text_cream),
            sand_gold: pick(self.sand_gold, base.sand_gold),
            amber: pick(self.amber, base.amber),
            tool_pink: pick(self.tool_pink, base.tool_pink),
            subdued: pick(self.subdued, base.subdued),
            terracotta: pick(self.terracotta, base.terracotta),
            muted_green: pick(self.muted_green, base.muted_green),
            shortcut_bg: pick(self.shortcut_bg, base.shortcut_bg),
            panel_border: pick(self.panel_border, base.panel_border),
            highlight_bg: pick(self.highlight_bg, base.highlight_bg),
            // The user-supplied name is short-lived — we keep the
            // built-in's `&'static str` for fennec-warm and use a
            // generic label for user skins so the type stays Copy.
            name: leak_name_or_static(name),
        }
    }
}

/// Avoid leaking a `String` for every user-skin load — for
/// well-known names we return a `'static` literal; for arbitrary
/// names we fall back to a generic label. The `name` field is
/// metadata only; `/skin status` reads the *config* value rather
/// than this field for arbitrary user names.
fn leak_name_or_static(name: &str) -> &'static str {
    match name {
        "fennec-warm" | "default" => "fennec-warm",
        "mono" => "mono",
        "light" => "light",
        "cool" => "cool",
        _ => "custom",
    }
}

/// Parse `"#RRGGBB"` (case-insensitive, # optional) into a
/// `Color::Rgb`. Returns `None` on malformed input.
pub fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

#[derive(Debug)]
pub enum SkinError {
    NotFound(String),
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: std::path::PathBuf,
        source: toml::de::Error,
    },
}

impl std::fmt::Display for SkinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(
                f,
                "skin '{name}' not found · try /skin list to see available skins"
            ),
            Self::Io { path, source } => {
                write!(f, "io error reading {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "parse error in {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for SkinError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fennec_warm_matches_theme_constants() {
        let s = Skin::fennec_warm();
        assert_eq!(s.bg_dusk, theme::BG_DUSK);
        assert_eq!(s.sand_gold, theme::SAND_GOLD);
        assert_eq!(s.terracotta, theme::TERRACOTTA);
        assert_eq!(s.name, "fennec-warm");
    }

    #[test]
    fn default_is_fennec_warm() {
        let d = Skin::default();
        assert_eq!(d.name, "fennec-warm");
    }

    #[test]
    fn builtin_resolves_known_names() {
        assert_eq!(Skin::builtin("mono").unwrap().name, "mono");
        assert_eq!(Skin::builtin("light").unwrap().name, "light");
        assert_eq!(Skin::builtin("cool").unwrap().name, "cool");
        assert_eq!(Skin::builtin("fennec-warm").unwrap().name, "fennec-warm");
        // Aliases for the default.
        assert_eq!(Skin::builtin("").unwrap().name, "fennec-warm");
        assert_eq!(Skin::builtin("default").unwrap().name, "fennec-warm");
        // Unknown name → None.
        assert!(Skin::builtin("nonexistent").is_none());
    }

    #[test]
    fn parse_hex_round_trip() {
        assert_eq!(parse_hex("#ff8800"), Some(Color::Rgb(0xFF, 0x88, 0x00)));
        assert_eq!(parse_hex("FF8800"), Some(Color::Rgb(0xFF, 0x88, 0x00)));
        assert_eq!(parse_hex("  #00ff00  "), Some(Color::Rgb(0x00, 0xFF, 0x00)));
        assert_eq!(parse_hex("#bad"), None); // too short
        assert_eq!(parse_hex("#zzzzzz"), None); // not hex
    }

    #[test]
    fn user_skin_overrides_individual_fields_only() {
        let dir = TempDir::new().unwrap();
        let skins = dir.path().join("skins");
        std::fs::create_dir_all(&skins).unwrap();
        let body = r##"
sand_gold = "#ff0000"
amber = "#00ff00"
        "##;
        std::fs::write(skins.join("redgold.toml"), body).unwrap();
        let s = Skin::resolve("redgold", dir.path()).unwrap();
        assert_eq!(s.sand_gold, Color::Rgb(0xFF, 0, 0));
        assert_eq!(s.amber, Color::Rgb(0, 0xFF, 0));
        // Untouched fields fall back to fennec-warm.
        assert_eq!(s.bg_dusk, theme::BG_DUSK);
        assert_eq!(s.terracotta, theme::TERRACOTTA);
    }

    #[test]
    fn resolve_unknown_name_returns_not_found() {
        let dir = TempDir::new().unwrap();
        let err = Skin::resolve("missing", dir.path()).unwrap_err();
        assert!(matches!(err, SkinError::NotFound(_)));
    }

    #[test]
    fn resolve_builtin_short_circuits_disk_lookup() {
        let dir = TempDir::new().unwrap();
        // No skins/ directory at all — but mono is built-in so
        // resolution should still succeed.
        let s = Skin::resolve("mono", dir.path()).unwrap();
        assert_eq!(s.name, "mono");
    }
}
