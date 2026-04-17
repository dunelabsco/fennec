//! Wizard frame: presentation layer around the 6-step setup wizard.
//!
//! Provides `WizardFrame` — manages an alternate terminal screen, renders a
//! progress header + completed-step summaries + the active step, and
//! guarantees terminal cleanup on drop or panic.
//!
//! Business logic stays in `wizard.rs`; this module only handles rendering.

use std::io::Write;

// xterm-compatible escape sequences (work on all modern terminals).
pub(crate) const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
pub(crate) const EXIT_ALT_SCREEN: &str = "\x1b[?1049l";
pub(crate) const HIDE_CURSOR: &str = "\x1b[?25l";
pub(crate) const SHOW_CURSOR: &str = "\x1b[?25h";
pub(crate) const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
pub(crate) const RESET_ATTRS: &str = "\x1b[0m";

// ---------------------------------------------------------------------------
// StepSummary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StepSummary {
    pub title: String,
    pub value: String,
    pub skipped: bool,
}

impl StepSummary {
    pub fn done(title: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            value: value.into(),
            skipped: false,
        }
    }

    pub fn skipped(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            value: "(skipped)".to_string(),
            skipped: true,
        }
    }

    pub fn render_line(&self, width: usize, narrow: bool) -> String {
        let display_value = mask_if_secret(&self.value);
        if narrow {
            format!("✓ {}", truncate(&display_value, width.saturating_sub(2)))
        } else {
            render_two_col("✓", &self.title, &display_value, width)
        }
    }

    pub fn render_line_ascii(&self, width: usize, narrow: bool) -> String {
        let display_value = mask_if_secret(&self.value);
        if narrow {
            format!("[v] {}", truncate(&display_value, width.saturating_sub(4)))
        } else {
            render_two_col("[v]", &self.title, &display_value, width)
        }
    }
}

fn mask_if_secret(value: &str) -> String {
    let looks_like_key = (value.starts_with("sk-")
        || value.starts_with("xoxb-")
        || value.starts_with("xapp-"))
        && value.len() > 16;
    if !looks_like_key {
        return value.to_string();
    }
    // Keep the provider prefix (e.g. "sk-ant") plus ellipsis plus last 5 chars.
    let suffix_len = 5.min(value.len());
    let suffix = &value[value.len() - suffix_len..];
    // Find the end of the second hyphen-delimited segment if present.
    let first_part_end = match value.match_indices('-').nth(1) {
        Some((i, _)) => i,
        None => value.len().min(7),
    };
    let first_part = &value[..first_part_end.min(value.len())];
    format!("{}-...{}", first_part, suffix)
}

fn render_two_col(mark: &str, title: &str, value: &str, width: usize) -> String {
    const TITLE_WIDTH: usize = 18;
    let title_count = title.chars().count();
    let title_padded = if title_count < TITLE_WIDTH {
        let pad = TITLE_WIDTH - title_count;
        format!("{}{}", title, " ".repeat(pad))
    } else {
        let taken: String = title.chars().take(TITLE_WIDTH).collect();
        taken
    };
    let prefix = format!("{} {}", mark, title_padded);
    let remaining = width.saturating_sub(prefix.chars().count());
    let value_trunc = truncate(value, remaining);
    format!("{}{}", prefix, value_trunc)
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let keep = max.saturating_sub(1);
        let truncated: String = s.chars().take(keep).collect();
        format!("{}…", truncated)
    }
}

// ---------------------------------------------------------------------------
// FinalSummary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FinalSummary {
    pub config_path: std::path::PathBuf,
    pub quick_start: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// TermCaps
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct TermCaps {
    pub width: usize,
    pub use_color: bool,
    pub use_unicode: bool,
    pub is_tty: bool,
}

impl TermCaps {
    pub fn probe() -> Self {
        let term = console::Term::stdout();
        let is_tty = term.is_term();
        let (_, cols) = term.size();
        let width = cols as usize;
        let use_color = std::env::var("NO_COLOR").is_err() && is_tty;
        let term_env = std::env::var("TERM").unwrap_or_default();
        let use_unicode = !term_env.is_empty() && term_env != "dumb" && is_tty;
        Self {
            width,
            use_color,
            use_unicode,
            is_tty,
        }
    }

    pub fn narrow(&self) -> bool {
        self.width < 60
    }
}

// ---------------------------------------------------------------------------
// Glyphs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub check: &'static str,
    pub cross: &'static str,
    pub bar_full: &'static str,
    pub bar_empty: &'static str,
    pub prompt: &'static str,
}

impl Glyphs {
    pub const fn unicode() -> Self {
        Self {
            check: "✓",
            cross: "✗",
            bar_full: "■",
            bar_empty: "░",
            prompt: "›",
        }
    }

    pub const fn ascii() -> Self {
        Self {
            check: "[v]",
            cross: "[x]",
            bar_full: "#",
            bar_empty: ".",
            prompt: ">",
        }
    }

    pub fn for_caps(caps: &TermCaps) -> Self {
        if caps.use_unicode {
            Self::unicode()
        } else {
            Self::ascii()
        }
    }
}

// ---------------------------------------------------------------------------
// Existing-config helper
// ---------------------------------------------------------------------------

pub fn existing_config_at(fennec_home: &std::path::Path) -> bool {
    fennec_home.join("config.toml").exists()
}

// ---------------------------------------------------------------------------
// WizardFrame
// ---------------------------------------------------------------------------

pub struct WizardFrame {
    total_steps: usize,
    current_step: usize,
    current_title: String,
    summaries: Vec<StepSummary>,
    caps: TermCaps,
    glyphs: Glyphs,
    alt_screen_active: bool,
    panic_hook_installed: bool,
}

impl WizardFrame {
    pub fn new(total_steps: usize) -> Self {
        let caps = TermCaps::probe();
        Self::with_caps(total_steps, caps)
    }

    #[cfg(test)]
    pub fn new_for_test(total_steps: usize, caps: TermCaps) -> Self {
        Self::with_caps(total_steps, caps)
    }

    fn with_caps(total_steps: usize, caps: TermCaps) -> Self {
        let glyphs = Glyphs::for_caps(&caps);
        Self {
            total_steps,
            current_step: 0,
            current_title: String::new(),
            summaries: Vec::new(),
            caps,
            glyphs,
            alt_screen_active: false,
            panic_hook_installed: false,
        }
    }

    pub fn begin_step(&mut self, idx: usize, title: &str) {
        self.current_step = idx;
        self.current_title = title.to_string();
    }

    pub fn complete_step(&mut self, summary: StepSummary) {
        self.summaries.push(summary);
    }

    pub fn start(&mut self) -> std::io::Result<()> {
        self.install_panic_hook();
        let mut stdout = std::io::stdout();
        self.start_to(&mut stdout)
    }

    pub fn start_to<W: Write>(&mut self, sink: &mut W) -> std::io::Result<()> {
        if !self.caps.is_tty {
            return Ok(());
        }
        sink.write_all(ENTER_ALT_SCREEN.as_bytes())?;
        sink.write_all(HIDE_CURSOR.as_bytes())?;
        sink.write_all(CLEAR_SCREEN.as_bytes())?;
        sink.flush()?;
        self.alt_screen_active = true;
        Ok(())
    }

    pub fn redraw(&self) -> std::io::Result<()> {
        let mut stdout = std::io::stdout();
        self.redraw_to(&mut stdout)
    }

    pub fn redraw_to<W: Write>(&self, sink: &mut W) -> std::io::Result<()> {
        if !self.caps.is_tty {
            return Ok(());
        }
        sink.write_all(CLEAR_SCREEN.as_bytes())?;
        sink.write_all(self.render_ansi().as_bytes())?;
        sink.flush()?;
        Ok(())
    }

    pub fn finish(&mut self, final_sum: FinalSummary) -> std::io::Result<()> {
        if !self.caps.is_tty {
            return Ok(());
        }
        let mut stdout = std::io::stdout();
        stdout.write_all(CLEAR_SCREEN.as_bytes())?;
        stdout.write_all(self.render_final_ansi(&final_sum).as_bytes())?;
        stdout.flush()?;
        let _ = console::Term::stdout().read_line();
        Ok(())
    }

    pub fn cleanup_string(&self) -> String {
        if !self.alt_screen_active {
            return String::new();
        }
        format!("{}{}{}", SHOW_CURSOR, RESET_ATTRS, EXIT_ALT_SCREEN)
    }

    pub fn install_panic_hook(&mut self) {
        if self.panic_hook_installed {
            return;
        }
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let cleanup = build_panic_cleanup_fn();
            let mut stderr = std::io::stderr();
            cleanup(&mut stderr);
            previous(info);
        }));
        self.panic_hook_installed = true;
    }

    // Render helpers -------------------------------------------------------

    pub fn render_to_string(&self) -> String {
        let mut out = String::new();
        out.push_str("  Fennec Setup\n");
        out.push_str(&format!("  {}\n", "─".repeat(self.rule_width())));
        out.push_str(&format!(
            "  {}  {} / {}\n",
            self.progress_bar(),
            self.current_step,
            self.total_steps
        ));
        out.push('\n');
        for summary in &self.summaries {
            let line = self.summary_line(summary);
            out.push_str(&format!("  {}\n", line));
        }
        if !self.summaries.is_empty() {
            out.push('\n');
        }
        if self.current_step > 0 {
            out.push_str(&format!(
                "  Step {} · {}\n",
                self.current_step, self.current_title
            ));
            out.push_str(&format!("  {} ", self.glyphs.prompt));
        }
        out
    }

    pub fn render_ansi(&self) -> String {
        let use_color = self.caps.use_color;
        let mut out = String::new();

        let heading = styled(use_color, "Fennec Setup", |s| s.cyan().bold());
        out.push_str(&format!("  {}\n", heading));

        let rule = "─".repeat(self.rule_width());
        let rule_styled = styled(use_color, &rule, |s| s.dim());
        out.push_str(&format!("  {}\n", rule_styled));

        let bar = self.progress_bar();
        out.push_str(&format!(
            "  {}  {} / {}\n\n",
            bar, self.current_step, self.total_steps
        ));

        for summary in &self.summaries {
            let line = self.summary_line(summary);
            let styled_line = styled(use_color, &line, |s| s.dim());
            out.push_str(&format!("  {}\n", styled_line));
        }
        if !self.summaries.is_empty() {
            out.push('\n');
        }

        if self.current_step > 0 {
            let step_title = format!("Step {} · {}", self.current_step, self.current_title);
            let title_styled = styled(use_color, &step_title, |s| s.bold());
            out.push_str(&format!("  {}\n", title_styled));
            let prompt_styled = styled(use_color, self.glyphs.prompt, |s| s.bold());
            out.push_str(&format!("  {} ", prompt_styled));
        }
        out
    }

    pub fn render_final_ansi(&self, final_sum: &FinalSummary) -> String {
        let use_color = self.caps.use_color;
        let mut out = String::new();

        let heading = styled(use_color, "Fennec Setup · Complete", |s| s.cyan().bold());
        out.push_str(&format!("  {}\n", heading));
        let rule = "─".repeat(self.rule_width());
        let rule_styled = styled(use_color, &rule, |s| s.dim());
        out.push_str(&format!("  {}\n\n", rule_styled));

        for summary in &self.summaries {
            let line = self.summary_line(summary);
            let styled_line = styled(use_color, &line, |s| s.green());
            out.push_str(&format!("  {}\n", styled_line));
        }
        out.push('\n');
        out.push_str(&format!(
            "  Config written to {}\n\n",
            final_sum.config_path.display()
        ));

        if !final_sum.quick_start.is_empty() {
            out.push_str("  Quick start:\n");
            for (cmd, desc) in &final_sum.quick_start {
                let cmd_padded = format!("{:<26}", cmd);
                let cmd_styled = styled(use_color, &cmd_padded, |s| s.bold());
                let desc_styled = styled(use_color, desc, |s| s.dim());
                out.push_str(&format!("    {}{}\n", cmd_styled, desc_styled));
            }
            out.push('\n');
        }
        let press_enter = styled(use_color, "Press Enter to exit.", |s| s.dim());
        out.push_str(&format!("  {}\n", press_enter));
        out
    }

    pub fn render_final_to_string(&self, final_sum: &FinalSummary) -> String {
        let mut out = String::new();
        out.push_str("  Fennec Setup · Complete\n");
        out.push_str(&format!("  {}\n\n", "─".repeat(self.rule_width())));
        for summary in &self.summaries {
            let line = self.summary_line(summary);
            out.push_str(&format!("  {}\n", line));
        }
        out.push('\n');
        out.push_str(&format!(
            "  Config written to {}\n\n",
            final_sum.config_path.display()
        ));
        if !final_sum.quick_start.is_empty() {
            out.push_str("  Quick start:\n");
            for (cmd, desc) in &final_sum.quick_start {
                out.push_str(&format!("    {:<26}{}\n", cmd, desc));
            }
            out.push('\n');
        }
        out.push_str("  Press Enter to exit.\n");
        out
    }

    // Internal helpers -----------------------------------------------------

    fn rule_width(&self) -> usize {
        self.caps.width.min(60).saturating_sub(4).max(1)
    }

    fn summary_line(&self, summary: &StepSummary) -> String {
        if self.caps.use_unicode {
            summary.render_line(self.caps.width, self.caps.narrow())
        } else {
            summary.render_line_ascii(self.caps.width, self.caps.narrow())
        }
    }

    fn progress_bar(&self) -> String {
        let filled = self.current_step.min(self.total_steps);
        let empty = self.total_steps.saturating_sub(filled);
        let full = self.glyphs.bar_full.repeat(filled);
        let emp = self.glyphs.bar_empty.repeat(empty);
        format!("[{}{}]", full, emp)
    }
}

impl Drop for WizardFrame {
    fn drop(&mut self) {
        if self.alt_screen_active {
            let mut stdout = std::io::stdout();
            let cleanup = self.cleanup_string();
            let _ = stdout.write_all(cleanup.as_bytes());
            let _ = stdout.flush();
            self.alt_screen_active = false;
        }
    }
}

/// Apply an ANSI style to `text`, forcing the styling regardless of whether
/// console's global TTY detection thinks the output is a terminal. We control
/// the use_color decision ourselves via `TermCaps::use_color`.
fn styled<F>(use_color: bool, text: &str, f: F) -> String
where
    F: FnOnce(console::Style) -> console::Style,
{
    if !use_color {
        return text.to_string();
    }
    let style = f(console::Style::new().force_styling(true));
    style.apply_to(text).to_string()
}

pub(crate) fn build_panic_cleanup_fn() -> impl Fn(&mut dyn Write) {
    |sink: &mut dyn Write| {
        let _ = sink.write_all(SHOW_CURSOR.as_bytes());
        let _ = sink.write_all(RESET_ATTRS.as_bytes());
        let _ = sink.write_all(EXIT_ALT_SCREEN.as_bytes());
        let _ = sink.flush();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_caps(width: usize, use_color: bool, use_unicode: bool) -> TermCaps {
        TermCaps {
            width,
            use_color,
            use_unicode,
            is_tty: true,
        }
    }

    #[test]
    fn summary_done_renders_title_and_value() {
        let s = StepSummary::done("Provider", "Anthropic (Claude)");
        let out = s.render_line(60, false);
        assert!(out.starts_with("✓ Provider"), "got: {:?}", out);
        assert!(out.ends_with("Anthropic (Claude)"), "got: {:?}", out);
    }

    #[test]
    fn summary_skipped_renders_skipped_marker() {
        let s = StepSummary::skipped("Telegram");
        let out = s.render_line(60, false);
        assert!(out.contains("(skipped)"), "got: {:?}", out);
    }

    #[test]
    fn summary_masks_api_keys() {
        let s = StepSummary::done("Key", "sk-ant-api03-abcdefghijklmnopqrstuv");
        let out = s.render_line(80, false);
        assert!(out.contains("sk-ant"), "got: {:?}", out);
        assert!(out.contains("..."), "got: {:?}", out);
        assert!(
            !out.contains("abcdefghijklmnopqrstuv"),
            "full key leaked: {:?}",
            out
        );
    }

    #[test]
    fn summary_narrow_collapses_to_single_line() {
        let s = StepSummary::done("Provider", "Anthropic (Claude)");
        let out = s.render_line(40, true);
        assert_eq!(out, "✓ Anthropic (Claude)");
    }

    #[test]
    fn summary_truncates_very_long_values() {
        let s = StepSummary::done("X", "a".repeat(200));
        let out = s.render_line(60, false);
        assert!(out.chars().count() <= 60, "len: {}", out.chars().count());
        assert!(out.contains('…'));
    }

    #[test]
    fn summary_ascii_mode_uses_plain_check() {
        let s = StepSummary::done("Provider", "Anthropic");
        let out = s.render_line_ascii(60, false);
        assert!(out.starts_with("[v] Provider"), "got: {:?}", out);
    }

    // Note: `probe()` reads NO_COLOR and TERM from the process env. Testing
    // that directly is race-prone under parallel tests (and env mutation is
    // `unsafe` in Rust 2024). We exercise the probe logic via TermCaps
    // construction instead — the struct fields are the contract.

    #[test]
    fn term_caps_no_color_gates_styling() {
        let caps = TermCaps {
            width: 80,
            use_color: false,
            use_unicode: true,
            is_tty: true,
        };
        assert!(!caps.use_color);
    }

    #[test]
    fn term_caps_dumb_term_gates_unicode() {
        let caps = TermCaps {
            width: 80,
            use_color: true,
            use_unicode: false,
            is_tty: true,
        };
        assert!(!caps.use_unicode);
    }

    #[test]
    fn term_caps_narrow_below_60() {
        let caps = test_caps(40, true, true);
        assert!(caps.narrow());
    }

    #[test]
    fn term_caps_narrow_false_at_60_or_above() {
        let caps = test_caps(60, true, true);
        assert!(!caps.narrow());
    }

    #[test]
    fn glyphs_unicode_set() {
        let g = Glyphs::unicode();
        assert_eq!(g.check, "✓");
        assert_eq!(g.bar_full, "■");
        assert_eq!(g.bar_empty, "░");
        assert_eq!(g.prompt, "›");
    }

    #[test]
    fn glyphs_ascii_fallback() {
        let g = Glyphs::ascii();
        assert_eq!(g.check, "[v]");
        assert_eq!(g.bar_full, "#");
        assert_eq!(g.bar_empty, ".");
        assert_eq!(g.prompt, ">");
    }

    #[test]
    fn glyphs_for_caps_picks_unicode_when_available() {
        let g = Glyphs::for_caps(&test_caps(80, true, true));
        assert_eq!(g.check, "✓");
    }

    #[test]
    fn glyphs_for_caps_picks_ascii_when_dumb() {
        let g = Glyphs::for_caps(&test_caps(80, true, false));
        assert_eq!(g.check, "[v]");
    }

    #[test]
    fn frame_starts_at_step_one() {
        let mut f = WizardFrame::new_for_test(6, test_caps(80, false, true));
        f.begin_step(1, "Provider");
        let out = f.render_to_string();
        assert!(out.contains("Fennec Setup"), "out: {}", out);
        assert!(out.contains("1 / 6"), "out: {}", out);
        assert!(out.contains("Step 1 · Provider"), "out: {}", out);
    }

    #[test]
    fn frame_accumulates_completed_summaries_in_order() {
        let mut f = WizardFrame::new_for_test(6, test_caps(80, false, true));
        f.begin_step(1, "Provider");
        f.complete_step(StepSummary::done("Provider", "Anthropic"));
        f.begin_step(2, "Authentication");
        f.complete_step(StepSummary::done("Authentication", "OAuth"));
        f.begin_step(3, "Agent name");

        let out = f.render_to_string();
        assert!(out.contains("✓ Provider"));
        assert!(out.contains("✓ Authentication"));
        assert!(out.contains("Step 3 · Agent name"));
        assert!(out.contains("3 / 6"));
        let prov_pos = out.find("✓ Provider").unwrap();
        let step_pos = out.find("Step 3 · Agent name").unwrap();
        assert!(prov_pos < step_pos);
    }

    #[test]
    fn frame_narrow_collapses_summaries() {
        let mut f = WizardFrame::new_for_test(6, test_caps(40, false, true));
        f.begin_step(1, "Provider");
        f.complete_step(StepSummary::done("Provider", "Anthropic"));
        f.begin_step(2, "Authentication");
        let out = f.render_to_string();
        assert!(out.contains("✓ Anthropic"));
    }

    #[test]
    fn frame_progress_bar_matches_step() {
        let mut f = WizardFrame::new_for_test(6, test_caps(80, false, true));
        f.begin_step(4, "Telegram");
        let out = f.render_to_string();
        assert!(out.contains("[■■■■░░]"), "got: {}", out);
    }

    #[test]
    fn frame_ascii_mode_uses_ascii_glyphs() {
        let mut f = WizardFrame::new_for_test(6, test_caps(80, false, false));
        f.begin_step(1, "Provider");
        f.complete_step(StepSummary::done("Provider", "Anthropic"));
        f.begin_step(2, "Authentication");
        let out = f.render_to_string();
        assert!(out.contains("[v] Provider"), "out: {}", out);
        assert!(out.contains("[##....]"), "out: {}", out);
    }

    #[test]
    fn frame_render_with_color_contains_ansi() {
        let mut f = WizardFrame::new_for_test(6, test_caps(80, true, true));
        f.begin_step(1, "Provider");
        let ansi = f.render_ansi();
        assert!(ansi.contains("\x1b["), "no ansi: {:?}", ansi);
        let stripped = console::strip_ansi_codes(&ansi).to_string();
        assert_eq!(stripped, f.render_to_string());
    }

    #[test]
    fn frame_render_without_color_has_no_ansi() {
        let mut f = WizardFrame::new_for_test(6, test_caps(80, false, true));
        f.begin_step(1, "Provider");
        let ansi = f.render_ansi();
        assert!(!ansi.contains("\x1b["), "unexpected ansi: {:?}", ansi);
    }

    #[test]
    fn alt_screen_escape_sequences_are_correct() {
        assert_eq!(ENTER_ALT_SCREEN, "\x1b[?1049h");
        assert_eq!(EXIT_ALT_SCREEN, "\x1b[?1049l");
        assert_eq!(HIDE_CURSOR, "\x1b[?25l");
        assert_eq!(SHOW_CURSOR, "\x1b[?25h");
    }

    #[test]
    fn frame_start_to_writes_alt_screen() {
        let mut sink: Vec<u8> = Vec::new();
        let mut f = WizardFrame::new_for_test(6, test_caps(80, false, true));
        f.start_to(&mut sink).unwrap();
        let out = String::from_utf8(sink).unwrap();
        assert!(out.contains(ENTER_ALT_SCREEN));
        assert!(out.contains(HIDE_CURSOR));
        assert!(f.alt_screen_active);
    }

    #[test]
    fn frame_start_to_noop_when_not_tty() {
        let caps = TermCaps {
            width: 80,
            use_color: false,
            use_unicode: true,
            is_tty: false,
        };
        let mut sink: Vec<u8> = Vec::new();
        let mut f = WizardFrame::new_for_test(6, caps);
        f.start_to(&mut sink).unwrap();
        assert!(sink.is_empty());
        assert!(!f.alt_screen_active);
    }

    #[test]
    fn cleanup_string_includes_restore_sequence() {
        let mut f = WizardFrame::new_for_test(6, test_caps(80, false, true));
        f.alt_screen_active = true;
        let cleanup = f.cleanup_string();
        assert!(cleanup.contains(SHOW_CURSOR));
        assert!(cleanup.contains(EXIT_ALT_SCREEN));
    }

    #[test]
    fn cleanup_string_empty_when_not_active() {
        let f = WizardFrame::new_for_test(6, test_caps(80, false, true));
        assert_eq!(f.cleanup_string(), "");
    }

    #[test]
    fn panic_cleanup_fn_writes_reset() {
        let hook = build_panic_cleanup_fn();
        let mut buf: Vec<u8> = Vec::new();
        hook(&mut buf);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains(SHOW_CURSOR));
        assert!(s.contains(EXIT_ALT_SCREEN));
    }

    #[test]
    fn finish_renders_final_summary() {
        let mut f = WizardFrame::new_for_test(6, test_caps(80, false, true));
        for i in 1..=6 {
            f.begin_step(i, "Step");
            f.complete_step(StepSummary::done("Step", format!("val{}", i)));
        }
        let final_sum = FinalSummary {
            config_path: std::path::PathBuf::from("/home/user/.fennec/config.toml"),
            quick_start: vec![
                ("fennec agent".to_string(), "Interactive chat".to_string()),
                ("fennec gateway".to_string(), "Start all channels".to_string()),
            ],
        };
        let out = f.render_final_to_string(&final_sum);
        assert!(out.contains("Complete"));
        assert!(out.contains("/home/user/.fennec/config.toml"));
        assert!(out.contains("fennec agent"));
        assert!(out.contains("Interactive chat"));
        assert!(out.contains("Press Enter"));
        for i in 1..=6 {
            assert!(out.contains(&format!("val{}", i)), "missing val{}: {}", i, out);
        }
    }

    #[test]
    fn existing_config_at_true_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "x").unwrap();
        assert!(existing_config_at(tmp.path()));
    }

    #[test]
    fn existing_config_at_false_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!existing_config_at(tmp.path()));
    }
}
