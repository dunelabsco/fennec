//! Terminal UI for `fennec --tui`.
//!
//! Three-pane k9s-style layout activated by the `--tui` flag.
//! Sessions left, chat center, tool-live + channels right;
//! collapses to two panes (sessions + chat) on terminals
//! narrower than 120 columns. Existing CLI mode is unchanged —
//! `fennec agent` still works exactly as it does today; `fennec
//! agent --tui` opts into the TUI.
//!
//! This module owns:
//!
//! - **Layout & rendering** (`layout`, `theme`) — `ratatui`-based
//!   panel split, fennec-fox warm palette, clean borders.
//! - **App state** (`app`) — sessions, chat scrollback, current
//!   tool execution, channel statuses.
//! - **Event loop** (`run`) — keyboard + tick events, dispatch to
//!   the right pane / command, exit handling.
//! - **Slash commands** (`commands`) — the `/help`, `/clear`,
//!   `/resume`, etc. set lifted from the upstream's TUI.
//! - **Agent callback bridge** (`callbacks`) — implements
//!   `AgentCallbacks` and routes events into the TUI's event loop
//!   so streaming text, tool starts, reasoning all reach the
//!   render path without blocking the agent.
//!
//! The TUI runs **in-process** with the agent — no separate
//! gateway, no IPC. This is the deliberate divergence from the
//! upstream's Node-TUI ↔ Python-gateway split: Rust can hold the
//! agent and the renderer in the same process trivially, and we
//! save a whole RPC layer.

pub mod agents_overlay;
pub mod app;
pub mod callbacks;
pub mod clipboard;
pub mod commands;
pub mod layout;
pub mod spawn_tree;
pub mod theme;
pub mod usage_panel;
pub mod voice;

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use parking_lot::Mutex;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub use app::App;

/// Entry point for the TUI mode. Sets up the alternate-screen
/// terminal, enters raw mode, and runs the event loop until the
/// user quits. Restores the terminal on exit (including on panic
/// — see the panic hook installed below).
pub fn run(app: Arc<Mutex<App>>) -> Result<()> {
    install_panic_hook();
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alt screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal")?;

    let result = event_loop(&mut terminal, app);

    // Always restore the terminal, even on Err — the alternate
    // screen + raw mode would otherwise leave the user's shell
    // in a broken state.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    result
}

/// Panic hook so a crash inside the render path doesn't leave
/// the user's terminal in raw mode + alt screen. We restore
/// before re-panicking so the panic message itself is readable.
fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: Arc<Mutex<App>>,
) -> Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let tick = Duration::from_millis(100);
    let mut last_tick = Instant::now();
    loop {
        terminal
            .draw(|frame| {
                let mut guard = app.lock();
                layout::draw(frame, &mut guard);
            })
            .map_err(io::Error::other)
            .context("draw failed")?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).context("event poll failed")? {
            match event::read().context("event read failed")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if should_quit(&key) {
                        return Ok(());
                    }
                    let mut guard = app.lock();
                    guard.handle_key(key.code, key.modifiers);
                }
                Event::Resize(_, _) => {
                    // Ratatui auto-handles resize on next draw; no
                    // app-state change needed.
                }
                _ => {}
            }
        }
        if last_tick.elapsed() >= tick {
            let mut guard = app.lock();
            guard.on_tick();
            last_tick = Instant::now();
        }
    }
}

fn should_quit(key: &crossterm::event::KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}
