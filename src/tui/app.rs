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
    /// Extended-thinking / reasoning block from the model
    /// (Anthropic `thinking` content blocks; OpenAI o1/o3
    /// `message.reasoning` field). Renders dim above the next
    /// bot reply. `is_streaming = true` while deltas are still
    /// arriving so the renderer shows a live cursor; flipped
    /// to false when the bot's text reply starts (or the turn
    /// ends without one).
    ReasoningBlock {
        body: String,
        started_at: Instant,
        is_streaming: bool,
    },
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

/// Visibility mode for inline tool blocks (and, once F1-2 lands
/// reasoning rendering, thinking blocks). Settable via
/// `/details hidden|collapsed|expanded`. Default is `Expanded`,
/// matching the F1-1 renderer's existing behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailsMode {
    /// Skip tool/reasoning blocks entirely.
    Hidden,
    /// Show only the header (`▸ tool · name`) without args /
    /// result body / spinner.
    Collapsed,
    /// Show everything (current F1-1 behavior).
    #[default]
    Expanded,
}

impl DetailsMode {
    /// Parse a string the user typed at the command line.
    /// Returns `None` for unknown values so callers can surface
    /// "unknown mode" error rather than silently defaulting.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "hidden" | "off" | "h" => Some(DetailsMode::Hidden),
            "collapsed" | "c" => Some(DetailsMode::Collapsed),
            "expanded" | "on" | "full" | "e" => Some(DetailsMode::Expanded),
            _ => None,
        }
    }

    /// Inverse of `parse` — used for config.toml persistence
    /// and `/details` (no arg) read-back.
    pub fn as_str(&self) -> &'static str {
        match self {
            DetailsMode::Hidden => "hidden",
            DetailsMode::Collapsed => "collapsed",
            DetailsMode::Expanded => "expanded",
        }
    }
}

/// Per-input editor state — multi-line buffer with cursor,
/// undo/redo stack, and history navigation.
///
/// Cursor coordinates are `(row, col)` in **char** units (not
/// bytes). All editing operations preserve this invariant. The
/// undo/redo stack records snapshots before each mutation; cap
/// is 200 to keep memory bounded.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    /// Recently submitted messages. ↑↓ at the top/bottom edges
    /// of the buffer cycle through these.
    pub history: VecDeque<String>,
    /// Cursor index into `history`. `None` = editing live buffer;
    /// `Some(i)` = previewing `history[i]`.
    pub history_cursor: Option<usize>,
    /// Snapshot stack for Ctrl-Z. Each entry is the full
    /// `(lines, row, col)` triple captured before a mutation.
    undo_stack: Vec<EditSnapshot>,
    /// Snapshots popped by Ctrl-Z and re-applicable via Ctrl-Y.
    /// Cleared on any new mutation.
    redo_stack: Vec<EditSnapshot>,
    /// Saved live-buffer when the user starts cycling history;
    /// restored when they cycle back past the most-recent entry.
    saved_live_buffer: Option<EditSnapshot>,
}

#[derive(Debug, Clone)]
struct EditSnapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

const UNDO_STACK_CAP: usize = 200;
const HISTORY_CAP: usize = 64;

impl InputState {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            history: VecDeque::with_capacity(HISTORY_CAP),
            history_cursor: None,
            undo_stack: Vec::with_capacity(32),
            redo_stack: Vec::new(),
            saved_live_buffer: None,
        }
    }

    /// Current buffer rendered as a single string with `\n`
    /// separators.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Replace the buffer with `s` and place the cursor at the
    /// end. Records an undo snapshot.
    pub fn set(&mut self, s: &str) {
        self.snapshot_for_undo();
        self.lines = if s.is_empty() {
            vec![String::new()]
        } else {
            s.split('\n').map(String::from).collect()
        };
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.lines[self.row].chars().count();
    }

    pub fn clear(&mut self) {
        self.snapshot_for_undo();
        self.lines = vec![String::new()];
        self.row = 0;
        self.col = 0;
        self.history_cursor = None;
        self.saved_live_buffer = None;
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
        while self.history.len() > HISTORY_CAP {
            self.history.pop_back();
        }
    }

    // -- Editor operations ---------------------------------------

    /// Insert one char at the cursor. Records an undo snapshot.
    pub fn insert_char(&mut self, c: char) {
        self.snapshot_for_undo();
        let line = &mut self.lines[self.row];
        let byte_idx = char_index_to_byte(line, self.col);
        line.insert(byte_idx, c);
        self.col += 1;
    }

    /// Insert a newline (Shift+Enter on most terminals).
    pub fn insert_newline(&mut self) {
        self.snapshot_for_undo();
        let line = self.lines[self.row].clone();
        let byte_idx = char_index_to_byte(&line, self.col);
        let (left, right) = line.split_at(byte_idx);
        self.lines[self.row] = left.to_string();
        self.lines.insert(self.row + 1, right.to_string());
        self.row += 1;
        self.col = 0;
    }

    /// Backspace — delete one char to the left of the cursor.
    pub fn backspace(&mut self) {
        self.snapshot_for_undo();
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let prev = char_index_to_byte(line, self.col - 1);
            let cur = char_index_to_byte(line, self.col);
            line.replace_range(prev..cur, "");
            self.col -= 1;
        } else if self.row > 0 {
            // Join with previous line.
            let cur_line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&cur_line);
        }
    }

    /// Delete the word backward (Ctrl-W).
    pub fn delete_word_backward(&mut self) {
        self.snapshot_for_undo();
        let line = &self.lines[self.row].clone();
        let mut new_col = self.col;
        let chars: Vec<char> = line.chars().collect();
        // Skip trailing whitespace before the cursor.
        while new_col > 0 && chars[new_col - 1].is_whitespace() {
            new_col -= 1;
        }
        // Skip the word.
        while new_col > 0 && !chars[new_col - 1].is_whitespace() {
            new_col -= 1;
        }
        let from = char_index_to_byte(line, new_col);
        let to = char_index_to_byte(line, self.col);
        self.lines[self.row].replace_range(from..to, "");
        self.col = new_col;
    }

    /// Delete from cursor to start of line (Ctrl-U).
    pub fn delete_to_line_start(&mut self) {
        self.snapshot_for_undo();
        let line = &self.lines[self.row].clone();
        let to = char_index_to_byte(line, self.col);
        self.lines[self.row].replace_range(..to, "");
        self.col = 0;
    }

    /// Delete from cursor to end of line (Ctrl-K).
    pub fn delete_to_line_end(&mut self) {
        self.snapshot_for_undo();
        let line = &self.lines[self.row].clone();
        let from = char_index_to_byte(line, self.col);
        self.lines[self.row].replace_range(from.., "");
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    pub fn move_right(&mut self) {
        let line_len = self.lines[self.row].chars().count();
        if self.col < line_len {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn move_word_left(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        if self.col == 0 {
            self.move_left();
            return;
        }
        let mut c = self.col;
        while c > 0 && chars[c - 1].is_whitespace() {
            c -= 1;
        }
        while c > 0 && !chars[c - 1].is_whitespace() {
            c -= 1;
        }
        self.col = c;
    }

    pub fn move_word_right(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let len = chars.len();
        if self.col >= len {
            self.move_right();
            return;
        }
        let mut c = self.col;
        while c < len && !chars[c].is_whitespace() {
            c += 1;
        }
        while c < len && chars[c].is_whitespace() {
            c += 1;
        }
        self.col = c;
    }

    pub fn move_line_start(&mut self) {
        self.col = 0;
    }

    pub fn move_line_end(&mut self) {
        self.col = self.lines[self.row].chars().count();
    }

    pub fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            let len = self.lines[self.row].chars().count();
            self.col = self.col.min(len);
        }
    }

    pub fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            let len = self.lines[self.row].chars().count();
            self.col = self.col.min(len);
        }
    }

    /// Whether the cursor is on the topmost row — for ↑ to flip
    /// into history navigation.
    pub fn at_first_row(&self) -> bool {
        self.row == 0
    }

    /// Whether the cursor is on the bottom row.
    pub fn at_last_row(&self) -> bool {
        self.row + 1 == self.lines.len()
    }

    // -- History navigation --------------------------------------

    /// Cycle one step backward in history (older). Saves the live
    /// buffer on first invocation so cycling forward past the
    /// most-recent entry restores it.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_cursor.is_none() {
            // Save the live buffer before overwriting it.
            self.saved_live_buffer = Some(self.snapshot());
        }
        let next = match self.history_cursor {
            None => 0,
            Some(i) => (i + 1).min(self.history.len() - 1),
        };
        self.history_cursor = Some(next);
        let snapshot = self.history[next].clone();
        self.lines = if snapshot.is_empty() {
            vec![String::new()]
        } else {
            snapshot.split('\n').map(String::from).collect()
        };
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.lines[self.row].chars().count();
    }

    /// Cycle one step forward in history (newer). When already at
    /// the most-recent entry, restores the saved live buffer.
    pub fn history_next(&mut self) {
        match self.history_cursor {
            None => {}
            Some(0) => {
                self.history_cursor = None;
                if let Some(saved) = self.saved_live_buffer.take() {
                    self.restore(saved);
                }
            }
            Some(i) => {
                self.history_cursor = Some(i - 1);
                let snapshot = self.history[i - 1].clone();
                self.lines = if snapshot.is_empty() {
                    vec![String::new()]
                } else {
                    snapshot.split('\n').map(String::from).collect()
                };
                self.row = self.lines.len().saturating_sub(1);
                self.col = self.lines[self.row].chars().count();
            }
        }
    }

    // -- Undo / redo --------------------------------------------

    pub fn undo(&mut self) {
        if let Some(snapshot) = self.undo_stack.pop() {
            self.redo_stack.push(self.snapshot());
            self.restore(snapshot);
        }
    }

    pub fn redo(&mut self) {
        if let Some(snapshot) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.restore(snapshot);
        }
    }

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        }
    }

    fn restore(&mut self, snapshot: EditSnapshot) {
        self.lines = snapshot.lines;
        self.row = snapshot.row;
        self.col = snapshot.col;
    }

    fn snapshot_for_undo(&mut self) {
        self.redo_stack.clear();
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_STACK_CAP {
            self.undo_stack.remove(0);
        }
    }
}

/// Translate a char index to a byte index within `s`. Cursor
/// positions are stored as char indices because Unicode-aware
/// editing is the default on modern terminals (emoji, CJK,
/// combining marks).
fn char_index_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
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
    /// `/compact` toggle — hides metadata + blank lines in the
    /// chat panel for tighter rendering.
    pub compact_mode: bool,
    /// `/details` mode — controls visibility of inline tool
    /// details (call args, running spinner, result summary) and
    /// reasoning blocks when those land in F1-2. Three settings
    /// matching Hermes (`ui-tui/src/app/slash/commands/ops.ts`):
    ///   - Hidden: skip tool blocks entirely
    ///   - Collapsed: render only `▸ tool · name` (no args / no
    ///     result body)
    ///   - Expanded: full detail (current F1-1 behavior)
    pub details_mode: DetailsMode,
    /// Per-section overrides set via `/details <section> <mode>`.
    /// When a key (e.g. `"thinking"`) is present, the renderer
    /// uses that mode for the matching section instead of the
    /// global `details_mode`. Mirrors Hermes' `ui.sections`
    /// (`domain/details.ts:51-74`).
    pub details_section_overrides: std::collections::HashMap<String, DetailsMode>,
    /// `/mouse` toggle — drives crossterm's mouse-tracking
    /// enable/disable on the next frame. Off by default to avoid
    /// stealing the user's terminal-native scroll wheel.
    pub mouse_enabled: bool,
    /// Index into `chat` of the assistant message currently being
    /// streamed. `None` between turns; `Some(i)` while text deltas
    /// are arriving so they accumulate into one growing message
    /// instead of pushing a new line per delta.
    pub in_flight_bot_idx: Option<usize>,
    /// Index of the in-flight `ChatLine::ReasoningBlock` for
    /// streaming thinking deltas. Same pattern as
    /// `in_flight_bot_idx` — `None` between turns.
    pub in_flight_reasoning_idx: Option<usize>,
    /// Voice subsystem handle. The `/voice` command flips its
    /// state; the tick handler polls it for transcriptions /
    /// errors and updates the UI accordingly.
    pub voice: super::voice::VoiceController,
    /// Identifier of the SessionStore row backing the current
    /// chat. Set when `run_tui` provisions or resumes a session;
    /// `None` if the store init failed (the TUI still works,
    /// just without persistence). `/title` and `/resume` mutate
    /// this through the submit-loop's `AgentAction` plumbing.
    pub current_session_id: Option<String>,
    /// User-visible label of the current session. Mirrors the
    /// `title` column of `current_session_id`'s row, kept on
    /// `App` so the chat header can show it without re-querying
    /// the store every frame.
    pub current_session_title: Option<String>,
    /// Filesystem location of `~/.fennec/skills/` (or override).
    /// `/skills` reads this lazily via SkillsLoader so the user
    /// sees a fresh list every time without a config reload.
    pub skills_dir: Option<std::path::PathBuf>,
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
            compact_mode: false,
            details_mode: DetailsMode::Expanded,
            details_section_overrides: std::collections::HashMap::new(),
            mouse_enabled: false,
            in_flight_bot_idx: None,
            in_flight_reasoning_idx: None,
            voice: super::voice::VoiceController::new(),
            current_session_id: None,
            current_session_title: None,
            skills_dir: None,
        }
    }

    /// Append a streaming text delta to the in-flight assistant
    /// message. Pushes a new `Bot` line if there isn't one yet
    /// for this turn (or if a tool call broke the streaming
    /// continuity by setting `in_flight_bot_idx` back to `None`).
    pub fn append_bot_delta(&mut self, delta: &str) {
        if let Some(idx) = self.in_flight_bot_idx {
            if let Some(ChatLine::Bot { body, .. }) = self.chat.get_mut(idx) {
                body.push_str(delta);
                return;
            }
            // Fall through to "fresh push" if the index points
            // at a non-Bot line (shouldn't happen, but be safe).
            self.in_flight_bot_idx = None;
        }
        self.chat.push(ChatLine::Bot {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            body: delta.to_string(),
        });
        self.in_flight_bot_idx = Some(self.chat.len() - 1);
    }

    /// Mark the in-flight assistant message complete (a tool call
    /// is starting, or the turn is ending). Subsequent
    /// `append_bot_delta` will start a fresh message.
    pub fn finalize_bot_message(&mut self) {
        self.in_flight_bot_idx = None;
    }

    /// Append a streaming reasoning delta to the in-flight
    /// thinking block. Mirrors `append_bot_delta`'s pattern but
    /// for `ChatLine::ReasoningBlock`. The block stays
    /// `is_streaming = true` until [`Self::finalize_reasoning_block`]
    /// is called (typically when the bot's text reply starts or
    /// the turn ends).
    pub fn append_reasoning_delta(&mut self, delta: &str) {
        if let Some(idx) = self.in_flight_reasoning_idx {
            if let Some(ChatLine::ReasoningBlock { body, .. }) =
                self.chat.get_mut(idx)
            {
                body.push_str(delta);
                return;
            }
            self.in_flight_reasoning_idx = None;
        }
        self.chat.push(ChatLine::ReasoningBlock {
            body: delta.to_string(),
            started_at: Instant::now(),
            is_streaming: true,
        });
        self.in_flight_reasoning_idx = Some(self.chat.len() - 1);
    }

    /// Stop the live cursor on the in-flight reasoning block —
    /// usually called when the assistant's text reply begins
    /// (thinking is done streaming).
    pub fn finalize_reasoning_block(&mut self) {
        if let Some(idx) = self.in_flight_reasoning_idx.take() {
            if let Some(ChatLine::ReasoningBlock { is_streaming, .. }) =
                self.chat.get_mut(idx)
            {
                *is_streaming = false;
            }
        }
    }

    /// Effective `DetailsMode` for the `thinking` section.
    /// Per-section override (`details_section_overrides["thinking"]`)
    /// wins; otherwise falls back to the global
    /// `details_mode`. Mirrors `domain/details.ts:69-74`'s
    /// `sectionMode()`.
    pub fn reasoning_mode(&self) -> DetailsMode {
        self.details_section_overrides
            .get("thinking")
            .copied()
            .unwrap_or(self.details_mode)
    }

    /// Set or clear a per-section override.
    pub fn set_section_override(
        &mut self,
        section: impl Into<String>,
        mode: Option<DetailsMode>,
    ) {
        match mode {
            Some(m) => {
                self.details_section_overrides.insert(section.into(), m);
            }
            None => {
                self.details_section_overrides.remove(&section.into());
            }
        }
    }

    /// Handle a key press. Routes to the focused pane; some keys
    /// are global (Tab cycles focus, Ctrl-C/D exits — handled in
    /// the event loop).
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        // Global hotkeys.
        if matches!(code, KeyCode::Tab) {
            self.cycle_focus();
            return;
        }

        match self.focus {
            Focus::Sessions => self.handle_key_sessions(code),
            Focus::Chat => self.handle_key_chat(code),
            Focus::Input => self.handle_key_input(code, modifiers),
        }
    }

    fn handle_key_sessions(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.prev_session(),
            KeyCode::Down | KeyCode::Char('j') => self.next_session(),
            _ => {}
        }
    }

    fn handle_key_chat(&mut self, code: KeyCode) {
        match code {
            KeyCode::PageUp => {
                self.chat_scroll = self.chat_scroll.saturating_add(10);
            }
            KeyCode::PageDown => {
                self.chat_scroll = self.chat_scroll.saturating_sub(10);
            }
            KeyCode::Up => {
                self.chat_scroll = self.chat_scroll.saturating_add(1);
            }
            KeyCode::Down => {
                self.chat_scroll = self.chat_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn handle_key_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let alt = modifiers.contains(KeyModifiers::ALT);
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            // Submit / newline.
            KeyCode::Enter if shift || alt => self.input.insert_newline(),
            KeyCode::Enter => {
                let text = self.input.text().trim().to_string();
                if !text.is_empty() {
                    self.input.push_history(text.clone());
                    self.input.clear();
                    self.set_status(format!("submitted: {}", truncate(&text, 40)));
                }
            }

            // Editing — undo / redo.
            KeyCode::Char('z') if ctrl => self.input.undo(),
            KeyCode::Char('y') if ctrl => self.input.redo(),

            // Editing — bulk delete.
            KeyCode::Char('w') if ctrl => self.input.delete_word_backward(),
            KeyCode::Char('u') if ctrl => self.input.delete_to_line_start(),
            KeyCode::Char('k') if ctrl => self.input.delete_to_line_end(),

            // Editing — single-char delete.
            KeyCode::Backspace => self.input.backspace(),

            // Cursor movement — char.
            KeyCode::Left if ctrl || alt => self.input.move_word_left(),
            KeyCode::Right if ctrl || alt => self.input.move_word_right(),
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),

            // Cursor movement — line / vertical / history.
            KeyCode::Home => self.input.move_line_start(),
            KeyCode::End => self.input.move_line_end(),
            KeyCode::Char('a') if ctrl => self.input.move_line_start(),
            KeyCode::Char('e') if ctrl => self.input.move_line_end(),
            KeyCode::Up if self.input.at_first_row() => self.input.history_prev(),
            KeyCode::Up => self.input.move_up(),
            KeyCode::Down if self.input.at_last_row() => self.input.history_next(),
            KeyCode::Down => self.input.move_down(),

            // Char input. Skipping ASCII control chars (0x00-0x1F)
            // so Ctrl-key combos we don't bind don't spew junk.
            KeyCode::Char(c) if !ctrl => self.input.insert_char(c),

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
        // Voice subsystem: a delivered transcription drops into
        // the input box; a delivered error shows as a transient
        // status so the user sees mic-open failures etc.
        if let Some(text) = self.voice.take_transcription() {
            self.input.set(&text);
            self.set_status(format!(
                "transcribed ({} chars) — review + Enter to send",
                text.chars().count()
            ));
        }
        if let Some(err) = self.voice.take_error() {
            self.set_status(format!("voice: {err}"));
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
    fn details_mode_parses_known_aliases() {
        assert_eq!(DetailsMode::parse("hidden"), Some(DetailsMode::Hidden));
        assert_eq!(DetailsMode::parse("h"), Some(DetailsMode::Hidden));
        assert_eq!(DetailsMode::parse("off"), Some(DetailsMode::Hidden));
        assert_eq!(
            DetailsMode::parse("collapsed"),
            Some(DetailsMode::Collapsed)
        );
        assert_eq!(DetailsMode::parse("c"), Some(DetailsMode::Collapsed));
        assert_eq!(
            DetailsMode::parse("expanded"),
            Some(DetailsMode::Expanded)
        );
        assert_eq!(DetailsMode::parse("on"), Some(DetailsMode::Expanded));
        assert_eq!(DetailsMode::parse("FULL"), Some(DetailsMode::Expanded));
    }

    #[test]
    fn details_mode_parse_returns_none_for_unknown() {
        assert!(DetailsMode::parse("foobar").is_none());
        assert!(DetailsMode::parse("").is_none());
    }

    #[test]
    fn details_mode_round_trips_via_as_str() {
        for m in [DetailsMode::Hidden, DetailsMode::Collapsed, DetailsMode::Expanded] {
            let serialized = m.as_str();
            assert_eq!(
                DetailsMode::parse(serialized),
                Some(m),
                "{serialized} did not round-trip"
            );
        }
    }

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

    #[test]
    fn shift_enter_inserts_newline_does_not_submit() {
        let mut app = App::new();
        app.focus = Focus::Input;
        app.handle_key(KeyCode::Char('a'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT);
        app.handle_key(KeyCode::Char('b'), KeyModifiers::NONE);
        assert_eq!(app.input.text(), "a\nb");
        assert_eq!(app.input.history.len(), 0);
    }

    #[test]
    fn ctrl_w_deletes_word_backward() {
        let mut s = InputState::new();
        for c in "hello world ".chars() {
            s.insert_char(c);
        }
        s.delete_word_backward();
        // First Ctrl-W eats trailing whitespace + "world".
        assert_eq!(s.text(), "hello ");
        s.delete_word_backward();
        // Second eats "hello" + the trailing space before it.
        assert_eq!(s.text(), "");
    }

    #[test]
    fn ctrl_u_deletes_to_line_start() {
        let mut s = InputState::new();
        for c in "hello world".chars() {
            s.insert_char(c);
        }
        s.move_line_start();
        s.move_word_right();
        s.delete_to_line_start();
        assert_eq!(s.text(), "world");
    }

    #[test]
    fn ctrl_k_deletes_to_line_end() {
        let mut s = InputState::new();
        for c in "hello world".chars() {
            s.insert_char(c);
        }
        s.col = 5;
        s.delete_to_line_end();
        assert_eq!(s.text(), "hello");
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut s = InputState::new();
        for c in "abc".chars() {
            s.insert_char(c);
        }
        assert_eq!(s.text(), "abc");
        s.undo();
        s.undo();
        s.undo();
        assert_eq!(s.text(), "");
        s.redo();
        s.redo();
        s.redo();
        assert_eq!(s.text(), "abc");
    }

    #[test]
    fn history_navigation_preserves_live_buffer() {
        let mut s = InputState::new();
        s.push_history("first".into());
        s.push_history("second".into());
        // Type something the user hasn't submitted.
        for c in "draft".chars() {
            s.insert_char(c);
        }
        assert_eq!(s.text(), "draft");
        // ↑ goes to most-recent history entry.
        s.history_prev();
        assert_eq!(s.text(), "second");
        // ↑ goes further back.
        s.history_prev();
        assert_eq!(s.text(), "first");
        // ↑ at the oldest pins.
        s.history_prev();
        assert_eq!(s.text(), "first");
        // ↓ steps back forward.
        s.history_next();
        assert_eq!(s.text(), "second");
        // ↓ past most-recent restores the saved live buffer.
        s.history_next();
        assert_eq!(s.text(), "draft");
    }

    #[test]
    fn cursor_word_movement_skips_words_and_whitespace() {
        let mut s = InputState::new();
        for c in "the quick brown".chars() {
            s.insert_char(c);
        }
        s.move_line_start();
        s.move_word_right();
        assert_eq!(s.col, 4); // start of "quick"
        s.move_word_right();
        assert_eq!(s.col, 10); // start of "brown"
        s.move_word_left();
        assert_eq!(s.col, 4);
    }

    #[test]
    fn append_bot_delta_creates_then_extends_in_flight_message() {
        let mut app = App::new();
        app.append_bot_delta("hello");
        app.append_bot_delta(", world");
        assert_eq!(app.chat.len(), 1);
        match &app.chat[0] {
            ChatLine::Bot { body, .. } => assert_eq!(body, "hello, world"),
            other => panic!("expected Bot line, got {:?}", other),
        }
        assert_eq!(app.in_flight_bot_idx, Some(0));
    }

    #[test]
    fn finalize_then_append_starts_fresh_message() {
        let mut app = App::new();
        app.append_bot_delta("first reply");
        app.finalize_bot_message();
        app.append_bot_delta("second reply");
        assert_eq!(app.chat.len(), 2);
        assert_eq!(app.in_flight_bot_idx, Some(1));
    }

    #[test]
    fn reasoning_deltas_accumulate_into_streaming_block() {
        let mut app = App::new();
        app.append_reasoning_delta("let me ");
        app.append_reasoning_delta("think...");
        assert_eq!(app.chat.len(), 1);
        match &app.chat[0] {
            ChatLine::ReasoningBlock {
                body,
                is_streaming,
                ..
            } => {
                assert_eq!(body, "let me think...");
                assert!(*is_streaming);
            }
            other => panic!("expected ReasoningBlock, got {:?}", other),
        }
        assert_eq!(app.in_flight_reasoning_idx, Some(0));
    }

    #[test]
    fn finalize_reasoning_block_stops_streaming_flag() {
        let mut app = App::new();
        app.append_reasoning_delta("draft");
        app.finalize_reasoning_block();
        assert_eq!(app.in_flight_reasoning_idx, None);
        match &app.chat[0] {
            ChatLine::ReasoningBlock { is_streaming, .. } => {
                assert!(!*is_streaming);
            }
            other => panic!("expected ReasoningBlock, got {:?}", other),
        }
    }

    #[test]
    fn reasoning_mode_prefers_section_override_over_global() {
        let mut app = App::new();
        app.details_mode = DetailsMode::Hidden;
        // No override → global wins.
        assert_eq!(app.reasoning_mode(), DetailsMode::Hidden);
        // Set override → wins.
        app.set_section_override("thinking", Some(DetailsMode::Expanded));
        assert_eq!(app.reasoning_mode(), DetailsMode::Expanded);
        // Reset override → fall back to global again.
        app.set_section_override("thinking", None);
        assert_eq!(app.reasoning_mode(), DetailsMode::Hidden);
    }

    #[test]
    fn unicode_aware_editing() {
        let mut s = InputState::new();
        for c in "héllo".chars() {
            s.insert_char(c);
        }
        assert_eq!(s.col, 5);
        s.backspace();
        s.backspace();
        assert_eq!(s.text(), "hél");
        assert_eq!(s.col, 3);
    }
}
