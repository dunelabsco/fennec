//! TUI application state.
//!
//! Holds the data the renderer reads and the input handlers
//! mutate: sessions list + selection, chat scrollback, current
//! tool execution, channel statuses, input buffer, modal state,
//! ticker for animations.
//!
//! Real wiring to the agent / session store / channel manager
//! comes in a follow-up commit. This is the state shape and the
//! event-handling skeleton — enough for the renderer to draw
//! against and for keyboard navigation to feel right.

use std::collections::VecDeque;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyModifiers};

/// One row in the sessions panel.
#[derive(Debug, Clone)]
pub struct SessionRow {
    /// Two-character source code: `TG` (telegram), `SL` (slack),
    /// `DC` (discord), `SG` (signal), `MX` (matrix), `@ ` (email),
    /// `$ ` (cli).
    pub code: String,
    /// Display name (sender / channel / user).
    pub who: String,
    /// One-line subject preview.
    pub subject: String,
    /// Render-ready count text ("47" or "2/31" with unread).
    pub count: String,
    /// Whether this row counts as having unread activity (drives
    /// the amber-bold render style).
    pub has_unread: bool,
}

/// One displayable line in the chat panel.
#[derive(Debug, Clone)]
pub enum ChatLine {
    /// `sys` line — session info, model, context window.
    System { time: String, body: String },
    /// User-typed message.
    User { time: String, body: String },
    /// Assistant message (possibly streaming).
    Bot { time: String, body: String },
    /// Inline `▸ tool · name(args)` line.
    ToolCall { call: String },
    /// Inline `↳ {result}` line under a tool call.
    ToolResult { summary: String },
    /// Inline running-tool marker with a spinner the renderer
    /// animates on tick. Replaced with `ToolResult` once the
    /// tool completes.
    ToolRunning { label: String, started_at: Instant },
}

/// Currently-executing tool, surfaced in the right column's
/// TOOL LIVE panel. `None` between tool calls.
#[derive(Debug, Clone)]
pub struct LiveTool {
    pub name: String,
    pub args_preview: String,
    pub started_at: Instant,
    /// 0..=100 progress hint. Tools that don't report progress
    /// just leave this at the animated default (looping pulse
    /// the renderer overlays).
    pub progress: Option<u8>,
}

/// Recent tool history shown under the live tool indicator.
#[derive(Debug, Clone)]
pub struct ToolHistoryEntry {
    pub ok: bool,
    pub name: String,
    pub note: String,
}

/// One channel + its current connection state. Drives the right
/// column's CHANNELS panel.
#[derive(Debug, Clone)]
pub struct ChannelState {
    pub code: String,
    pub name: String,
    pub state: ChannelConnState,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelConnState {
    Connected,
    Polling,
    Attached,
    Idle,
    Disconnected,
    Error,
}

/// Pane focus — drives keyboard routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sessions,
    Chat,
    Input,
}

/// Per-input editor state — multi-line buffer with cursor.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    /// Recently submitted messages. ↑↓ cycles through them.
    pub history: VecDeque<String>,
    /// Index into `history` while cycling (None = at the live
    /// buffer, not a history snapshot).
    pub history_cursor: Option<usize>,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            history: VecDeque::with_capacity(64),
            history_cursor: None,
        }
    }

    /// Current buffer rendered as a single string with `\n`
    /// separators.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Replace the buffer with `s` and reset the cursor to the
    /// end. Used by history navigation.
    pub fn set(&mut self, s: &str) {
        self.lines = if s.is_empty() {
            vec![String::new()]
        } else {
            s.split('\n').map(String::from).collect()
        };
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.lines[self.row].chars().count();
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.row = 0;
        self.col = 0;
        self.history_cursor = None;
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    pub fn push_history(&mut self, s: String) {
        if s.is_empty() {
            return;
        }
        if self.history.front().map(|h| h == &s).unwrap_or(false) {
            return;
        }
        self.history.push_front(s);
        while self.history.len() > 64 {
            self.history.pop_back();
        }
    }
}

/// Overall TUI app state.
pub struct App {
    pub focus: Focus,
    pub sessions: Vec<SessionRow>,
    pub selected_session: usize,
    pub chat: Vec<ChatLine>,
    /// Scroll offset from the bottom of the chat. 0 = pinned to
    /// latest; positive = scrolled up.
    pub chat_scroll: u16,
    pub live_tool: Option<LiveTool>,
    pub recent_tools: Vec<ToolHistoryEntry>,
    pub channels: Vec<ChannelState>,
    pub input: InputState,
    pub started_at: Instant,
    /// Whether the input is "ready" (cursor visible). Toggled by
    /// the tick handler so the cursor blinks.
    pub cursor_visible: bool,
    /// Brief status message shown at the bottom of the chat panel
    /// for ~3 seconds (e.g. "session resumed", "command not found").
    pub transient_status: Option<(String, Instant)>,
    /// Set to true when the event loop should exit.
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            focus: Focus::Input,
            sessions: Vec::new(),
            selected_session: 0,
            chat: Vec::new(),
            chat_scroll: 0,
            live_tool: None,
            recent_tools: Vec::new(),
            channels: Vec::new(),
            input: InputState::new(),
            started_at: Instant::now(),
            cursor_visible: true,
            transient_status: None,
            should_quit: false,
        }
    }

    /// Handle a key press. Routes to the focused pane; some keys
    /// are global (Tab cycles focus, Ctrl-C/D exits — handled in
    /// the event loop).
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Tab => self.cycle_focus(),
            KeyCode::Up if self.focus == Focus::Sessions => self.prev_session(),
            KeyCode::Down if self.focus == Focus::Sessions => self.next_session(),
            KeyCode::PageUp if self.focus == Focus::Chat => {
                self.chat_scroll = self.chat_scroll.saturating_add(10);
            }
            KeyCode::PageDown if self.focus == Focus::Chat => {
                self.chat_scroll = self.chat_scroll.saturating_sub(10);
            }
            // Input handling — full editor in a follow-up commit.
            // For now: type chars, backspace, enter clears, no
            // multi-line yet.
            KeyCode::Char(c) if self.focus == Focus::Input => {
                let _ = modifiers;
                self.input.lines[self.input.row].push(c);
                self.input.col += 1;
            }
            KeyCode::Backspace if self.focus == Focus::Input => {
                let line = &mut self.input.lines[self.input.row];
                if line.pop().is_some() {
                    self.input.col = self.input.col.saturating_sub(1);
                }
            }
            KeyCode::Enter if self.focus == Focus::Input => {
                let text = self.input.text().trim().to_string();
                if !text.is_empty() {
                    self.input.push_history(text.clone());
                    self.input.clear();
                    self.set_status(&format!("submitted: {}", truncate(&text, 40)));
                }
            }
            _ => {}
        }
    }

    /// Called every ~100ms by the event loop. Drives cursor blink
    /// and any time-based UI state changes.
    pub fn on_tick(&mut self) {
        let elapsed_500 = (self.started_at.elapsed().as_millis() / 500) % 2;
        self.cursor_visible = elapsed_500 == 0;
        // Auto-clear transient status after 3 seconds.
        if let Some((_, t)) = &self.transient_status {
            if t.elapsed().as_secs() >= 3 {
                self.transient_status = None;
            }
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sessions => Focus::Chat,
            Focus::Chat => Focus::Input,
            Focus::Input => Focus::Sessions,
        };
    }

    pub fn next_session(&mut self) {
        if self.selected_session + 1 < self.sessions.len() {
            self.selected_session += 1;
        }
    }

    pub fn prev_session(&mut self) {
        self.selected_session = self.selected_session.saturating_sub(1);
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.transient_status = Some((msg.into(), Instant::now()));
    }

    /// Animated spinner glyph for live tool indicators. Cycles at
    /// ~10Hz using the started_at clock.
    pub fn spinner_glyph(&self) -> &'static str {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let elapsed = self.started_at.elapsed().as_millis();
        FRAMES[(elapsed / 100) as usize % FRAMES.len()]
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_state_starts_empty() {
        let s = InputState::new();
        assert!(s.is_empty());
        assert_eq!(s.text(), "");
    }

    #[test]
    fn input_state_set_and_clear() {
        let mut s = InputState::new();
        s.set("hello\nworld");
        assert_eq!(s.text(), "hello\nworld");
        assert_eq!(s.row, 1);
        assert_eq!(s.col, 5);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn input_state_dedups_history() {
        let mut s = InputState::new();
        s.push_history("hi".into());
        s.push_history("hi".into()); // duplicate of front
        s.push_history("there".into());
        assert_eq!(s.history.len(), 2);
        assert_eq!(s.history.front().map(String::as_str), Some("there"));
    }

    #[test]
    fn input_state_caps_history_at_64() {
        let mut s = InputState::new();
        for i in 0..100 {
            s.push_history(format!("msg{i}"));
        }
        assert_eq!(s.history.len(), 64);
    }

    #[test]
    fn cycle_focus_round_trip() {
        let mut app = App::new();
        assert_eq!(app.focus, Focus::Input);
        app.cycle_focus();
        assert_eq!(app.focus, Focus::Sessions);
        app.cycle_focus();
        assert_eq!(app.focus, Focus::Chat);
        app.cycle_focus();
        assert_eq!(app.focus, Focus::Input);
    }

    #[test]
    fn navigation_clamps_at_bounds() {
        let mut app = App::new();
        app.sessions = vec![
            SessionRow {
                code: "TG".into(),
                who: "Alice".into(),
                subject: "x".into(),
                count: "1".into(),
                has_unread: false,
            },
            SessionRow {
                code: "SL".into(),
                who: "#a".into(),
                subject: "y".into(),
                count: "2".into(),
                has_unread: false,
            },
        ];
        app.focus = Focus::Sessions;
        app.next_session();
        assert_eq!(app.selected_session, 1);
        app.next_session(); // clamped
        assert_eq!(app.selected_session, 1);
        app.prev_session();
        assert_eq!(app.selected_session, 0);
        app.prev_session(); // clamped
        assert_eq!(app.selected_session, 0);
    }

    #[test]
    fn submitting_input_records_history_and_clears() {
        let mut app = App::new();
        app.focus = Focus::Input;
        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('i'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.input.is_empty());
        assert_eq!(app.input.history.front().map(String::as_str), Some("hi"));
    }
}
