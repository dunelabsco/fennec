//! TUI rendering — the V2 three-pane layout.
//!
//! Pure render functions: take `&mut Frame` + `&App`, draw. No
//! mutable state is touched here; all input is via `App`. Reading
//! these from top to bottom is the way to understand the visual
//! contract.
//!
//! Layout proportions:
//!   - sessions left: 38 cols (fixed)
//!   - chat center: flexible (`Min(40)`)
//!   - tool live + channels right: 38 cols (fixed)
//!   - status bar: bottom 1 row
//!   - shortcuts row: bottom 1 row below status
//!
//! Below 120 cols of total width, the right column collapses;
//! the running-tool indicator folds into a single-line hint above
//! the input box, channels-up indicator folds into the status bar.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap};

use super::app::{App, ChannelConnState, ChatLine, Focus, SessionRow};
use super::theme::*;

pub fn draw(f: &mut Frame, app: &mut App) {
    // Whole-screen background. Each panel paints over it.
    let bg = Block::default().style(Style::default().bg(BG_DUSK).fg(TEXT_CREAM));
    f.render_widget(bg, f.area());

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(1), // status bar
            Constraint::Length(1), // shortcuts row
        ])
        .split(f.area());

    let main_area = outer[0];
    let status_area = outer[1];
    let shortcut_area = outer[2];

    let narrow = main_area.width < 120;
    let cols = if narrow {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(36), Constraint::Min(40)])
            .split(main_area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(38),
                Constraint::Min(40),
                Constraint::Length(38),
            ])
            .split(main_area)
    };

    draw_sessions(f, cols[0], app);
    draw_chat(f, cols[1], app, narrow);
    if !narrow {
        draw_right_column(f, cols[2], app);
    }
    draw_status(f, status_area, app, narrow);
    draw_shortcuts(f, shortcut_area, app);
}

// -- Sessions panel ---------------------------------------------

fn draw_sessions(f: &mut Frame, area: Rect, app: &App) {
    let border_color = if app.focus == Focus::Sessions {
        SAND_GOLD
    } else {
        PANEL_BORDER
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // controls row
            Constraint::Length(1), // divider
            Constraint::Length(1), // + new session
            Constraint::Length(1), // divider
            Constraint::Min(1),    // session list
        ])
        .split(inner);

    let controls = Line::from(vec![
        Span::styled(" [f]", Style::default().fg(SAND_GOLD)),
        Span::styled("all ", Style::default().fg(TEXT_CREAM)),
        Span::styled("[s]", Style::default().fg(SAND_GOLD)),
        Span::styled("recent ", Style::default().fg(TEXT_CREAM)),
        Span::styled("[/]", Style::default().fg(SAND_GOLD)),
        Span::styled("find", Style::default().fg(TEXT_CREAM)),
    ]);
    f.render_widget(Paragraph::new(controls), layout[0]);

    f.render_widget(divider(inner.width), layout[1]);

    let new_session = Line::from(vec![
        Span::styled(
            " + ",
            Style::default().fg(MUTED_GREEN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "new session",
            Style::default().fg(MUTED_GREEN).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  [n]", Style::default().fg(SUBDUED)),
    ]);
    f.render_widget(Paragraph::new(new_session), layout[2]);

    f.render_widget(divider(inner.width), layout[3]);

    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|s| session_row(s, area.width))
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(HIGHLIGHT_BG)
                .fg(TEXT_CREAM)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" ");
    let mut state = ListState::default();
    state.select(Some(app.selected_session));
    f.render_stateful_widget(list, layout[4], &mut state);
}

fn session_row<'a>(s: &'a SessionRow, panel_width: u16) -> ListItem<'a> {
    let count_color = if s.has_unread { AMBER } else { SUBDUED };
    let count_modifier = if s.has_unread {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let code_color = code_color_for(&s.code);
    let subject_max = (panel_width as usize).saturating_sub(18 + s.who.chars().count());
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{} ", s.code),
            Style::default().fg(code_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} ", s.who),
            Style::default().fg(TEXT_CREAM),
        ),
        Span::styled(
            truncate(&s.subject, subject_max),
            Style::default().fg(SUBDUED),
        ),
        Span::styled(
            format!("  {}", s.count),
            Style::default().fg(count_color).add_modifier(count_modifier),
        ),
    ]))
}

/// Source-code color mapping. Mirrors the prototype's palette.
fn code_color_for(code: &str) -> Color {
    match code.trim() {
        "TG" => Color::Rgb(0x6B, 0xA8, 0xC7),
        "SL" => TOOL_PINK,
        "SG" => MUTED_GREEN,
        "DC" => Color::Rgb(0x90, 0x9B, 0xC7),
        "MX" => TOOL_PINK,
        "@" => SAND_GOLD,
        "$" => AMBER,
        _ => TEXT_CREAM,
    }
}

// -- Chat panel -------------------------------------------------

fn draw_chat(f: &mut Frame, area: Rect, app: &App, narrow: bool) {
    let border_color = if app.focus == Focus::Chat || app.focus == Focus::Input {
        SAND_GOLD
    } else {
        PANEL_BORDER
    };
    let session_label = current_session_label(app);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(PANEL_BORDER)),
            Span::styled("chat ", Style::default().fg(SAND_GOLD)),
            Span::styled(session_label, Style::default().fg(SUBDUED)),
            Span::styled(" ├", Style::default().fg(PANEL_BORDER)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let bottom_height = if narrow { 5 } else { 4 };
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(bottom_height)])
        .split(inner);

    draw_chat_scrollback(f, split[0], app);
    draw_chat_bottom(f, split[1], app, narrow);
}

fn current_session_label(app: &App) -> String {
    if app.sessions.is_empty() {
        return "no session".into();
    }
    let s = &app.sessions[app.selected_session.min(app.sessions.len() - 1)];
    let lines = app.chat.len();
    format!(
        "{} · {} · {} {}",
        s.code.trim().to_lowercase(),
        s.who.to_lowercase(),
        lines,
        if lines == 1 { "msg" } else { "msgs" }
    )
}

fn draw_chat_scrollback(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();
    for entry in &app.chat {
        match entry {
            ChatLine::System { time, body } => {
                lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(SUBDUED)),
                    Span::styled(
                        "sys ",
                        Style::default().fg(SUBDUED).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(time.clone(), Style::default().fg(SUBDUED)),
                ]));
                lines.push(Line::from(Span::styled(
                    body.clone(),
                    Style::default().fg(SUBDUED),
                )));
            }
            ChatLine::User { time, body } => {
                lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(AMBER)),
                    Span::styled(
                        "you ",
                        Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(time.clone(), Style::default().fg(SUBDUED)),
                ]));
                lines.push(Line::from(Span::styled(
                    body.clone(),
                    Style::default().fg(TEXT_CREAM),
                )));
            }
            ChatLine::Bot { time, body } => {
                lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(SAND_GOLD)),
                    Span::styled(
                        "fennec ",
                        Style::default().fg(SAND_GOLD).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(time.clone(), Style::default().fg(SUBDUED)),
                ]));
                lines.push(Line::from(Span::styled(
                    body.clone(),
                    Style::default().fg(TEXT_CREAM),
                )));
            }
            ChatLine::ToolCall { call } => {
                lines.push(Line::from(vec![
                    Span::styled("    ▸ ", Style::default().fg(TOOL_PINK)),
                    Span::styled(
                        "tool",
                        Style::default().fg(TOOL_PINK).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" · ", Style::default().fg(SUBDUED)),
                    Span::styled(call.clone(), Style::default().fg(TOOL_PINK)),
                ]));
            }
            ChatLine::ToolResult { summary } => {
                lines.push(Line::from(vec![
                    Span::styled("    ↳ ", Style::default().fg(SUBDUED)),
                    Span::styled(summary.clone(), Style::default().fg(SUBDUED)),
                ]));
            }
            ChatLine::ToolRunning { label, started_at } => {
                let elapsed = started_at.elapsed().as_millis();
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("    {} ", app.spinner_glyph()),
                        Style::default().fg(TOOL_PINK),
                    ),
                    Span::styled(
                        format!("{label} ({elapsed}ms)"),
                        Style::default().fg(TOOL_PINK).add_modifier(Modifier::DIM),
                    ),
                ]));
            }
        }
        lines.push(Line::raw(""));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_chat_bottom(f: &mut Frame, area: Rect, app: &App, narrow: bool) {
    let layout = if narrow {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // narrow tool hint
                Constraint::Length(1), // spacer
                Constraint::Length(1), // input box (border top)
                Constraint::Length(1), // input row
                Constraint::Length(1), // hint
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // spacer
                Constraint::Length(1), // input border top
                Constraint::Length(1), // input row
                Constraint::Length(1), // hint
            ])
            .split(area)
    };

    let mut idx = 0;
    if narrow {
        let folded = if let Some(t) = &app.live_tool {
            Line::from(vec![
                Span::styled(
                    format!(" {} ", app.spinner_glyph()),
                    Style::default().fg(TOOL_PINK),
                ),
                Span::styled("running ", Style::default().fg(SUBDUED)),
                Span::styled(t.name.clone(), Style::default().fg(TOOL_PINK)),
                Span::styled(
                    format!(" ({}ms)  ", t.started_at.elapsed().as_millis()),
                    Style::default().fg(SUBDUED),
                ),
                Span::styled("[t] ", Style::default().fg(SAND_GOLD)),
                Span::styled("tool detail", Style::default().fg(SUBDUED)),
            ])
        } else if let Some((msg, _)) = &app.transient_status {
            Line::from(Span::styled(
                format!(" {}", msg),
                Style::default().fg(SUBDUED),
            ))
        } else {
            Line::raw("")
        };
        f.render_widget(Paragraph::new(folded), layout[idx]);
        idx += 1;
    }

    // Spacer above input.
    f.render_widget(Paragraph::new(""), layout[idx]);
    idx += 1;

    // Input box (Block + cursor).
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(SAND_GOLD));
    // The block borders consume rows 0 and 2 of the next 3 rows;
    // we use the inner of a manually-split 3-row box.
    let box_top = layout[idx];
    let box_mid = layout[idx + 1];
    let box_outer = Rect {
        x: box_top.x,
        y: box_top.y,
        width: box_top.width,
        height: 3.min(area.bottom().saturating_sub(box_top.y)),
    };
    f.render_widget(input_block.clone(), box_outer);
    let inner = input_block.inner(box_outer);
    let cursor = if app.cursor_visible { "█" } else { " " };
    let prompt = if app.input.is_empty() {
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(SAND_GOLD)),
            Span::styled(cursor, Style::default().fg(SAND_GOLD)),
            Span::styled("  type a message…", Style::default().fg(SUBDUED)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(SAND_GOLD)),
            Span::styled(app.input.text(), Style::default().fg(TEXT_CREAM)),
            Span::styled(cursor, Style::default().fg(SAND_GOLD)),
        ])
    };
    f.render_widget(Paragraph::new(prompt), inner);
    let _ = box_mid;
    idx += 2;

    // Hint row below the input.
    if idx < layout.len() {
        let hint = Line::from(vec![
            Span::styled(" [enter] ", Style::default().fg(SAND_GOLD)),
            Span::styled("send  ", Style::default().fg(SUBDUED)),
            Span::styled("[shift-enter] ", Style::default().fg(SAND_GOLD)),
            Span::styled("newline  ", Style::default().fg(SUBDUED)),
            Span::styled("[ctrl-c] ", Style::default().fg(SAND_GOLD)),
            Span::styled("cancel turn", Style::default().fg(SUBDUED)),
        ]);
        f.render_widget(Paragraph::new(hint), layout[idx]);
    }
}

// -- Right column (TOOL LIVE + CHANNELS) ------------------------

fn draw_right_column(f: &mut Frame, area: Rect, app: &App) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(13), Constraint::Min(8)])
        .split(area);
    draw_tool_live(f, split[0], app);
    draw_channels(f, split[1], app);
}

fn draw_tool_live(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(TOOL_PINK));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    if let Some(t) = &app.live_tool {
        let header = Line::from(vec![
            Span::styled(
                format!("{} ", app.spinner_glyph()),
                Style::default().fg(TOOL_PINK),
            ),
            Span::styled("running ", Style::default().fg(SUBDUED)),
            Span::styled(
                t.name.clone(),
                Style::default().fg(TOOL_PINK).add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(Paragraph::new(header), layout[0]);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("args ", Style::default().fg(SUBDUED)),
                Span::styled(t.args_preview.clone(), Style::default().fg(TEXT_CREAM)),
            ])),
            layout[1],
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("started ", Style::default().fg(SUBDUED)),
                Span::styled(
                    format!("{}ms ago", t.started_at.elapsed().as_millis()),
                    Style::default().fg(TEXT_CREAM),
                ),
            ])),
            layout[3],
        );
        let progress = t.progress.unwrap_or_else(|| {
            // No explicit progress — show a slow visual sweep.
            let cycle = 5_000u128;
            let elapsed = app.started_at.elapsed().as_millis() % cycle;
            ((elapsed * 100) / cycle) as u8
        });
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(SAND_GOLD).bg(HIGHLIGHT_BG))
            .percent(u16::from(progress))
            .label(Span::styled(
                format!("{progress}%"),
                Style::default().fg(TEXT_CREAM),
            ));
        f.render_widget(gauge, layout[5]);
    } else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no tool running",
                Style::default().fg(SUBDUED),
            ))),
            layout[0],
        );
    }

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "recent",
            Style::default().fg(SUBDUED).add_modifier(Modifier::BOLD),
        ))),
        layout[6],
    );

    let recent: Vec<Line> = app
        .recent_tools
        .iter()
        .map(|t| {
            let (mark, mark_color) = if t.ok {
                ("✓ ", MUTED_GREEN)
            } else {
                ("✗ ", TERRACOTTA)
            };
            Line::from(vec![
                Span::styled(mark, Style::default().fg(mark_color)),
                Span::styled(
                    format!("{} ", t.name),
                    Style::default().fg(TEXT_CREAM),
                ),
                Span::styled(t.note.clone(), Style::default().fg(SUBDUED)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(recent), layout[7]);
}

fn draw_channels(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(PANEL_BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = app
        .channels
        .iter()
        .map(|c| {
            let state_color = match c.state {
                ChannelConnState::Connected => MUTED_GREEN,
                ChannelConnState::Polling => AMBER,
                ChannelConnState::Attached => SAND_GOLD,
                ChannelConnState::Idle => SUBDUED,
                ChannelConnState::Disconnected => SUBDUED,
                ChannelConnState::Error => TERRACOTTA,
            };
            let state_text = match c.state {
                ChannelConnState::Connected => "connected",
                ChannelConnState::Polling => "polling",
                ChannelConnState::Attached => "attached",
                ChannelConnState::Idle => "idle",
                ChannelConnState::Disconnected => "down",
                ChannelConnState::Error => "error",
            };
            Line::from(vec![
                Span::styled(
                    format!("{} ", c.code),
                    Style::default()
                        .fg(code_color_for(&c.code))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:<8}", c.name), Style::default().fg(TEXT_CREAM)),
                Span::styled("● ", Style::default().fg(state_color)),
                Span::styled(format!("{:<10}", state_text), Style::default().fg(state_color)),
                Span::styled(c.detail.clone(), Style::default().fg(SUBDUED)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

// -- Status + shortcuts row -------------------------------------

fn draw_status(f: &mut Frame, area: Rect, app: &App, narrow: bool) {
    let connected = app
        .channels
        .iter()
        .filter(|c| c.state == ChannelConnState::Connected || c.state == ChannelConnState::Attached)
        .count();
    let total = app.channels.len();
    let mut spans = vec![
        Span::styled(
            " fennec ",
            Style::default()
                .bg(SAND_GOLD)
                .fg(BG_DUSK)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{}  ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(SUBDUED),
        ),
        Span::styled("● ", Style::default().fg(MUTED_GREEN)),
        Span::styled(
            format!("agent ready · {connected}/{total} channels"),
            Style::default().fg(MUTED_GREEN),
        ),
    ];
    if narrow {
        spans.push(Span::styled("  ● ", Style::default().fg(MUTED_GREEN)));
        spans.push(Span::styled(
            format!("channels {connected}/{total}"),
            Style::default().fg(MUTED_GREEN),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(SHORTCUT_BG)),
        area,
    );

    // Right-align a clock.
    let clock = chrono::Local::now().format("%H:%M:%S").to_string();
    let clock_line = Line::from(vec![Span::styled(
        format!("{clock} "),
        Style::default().fg(SUBDUED),
    )])
    .alignment(Alignment::Right);
    f.render_widget(
        Paragraph::new(clock_line).style(Style::default().bg(SHORTCUT_BG)),
        area,
    );
}

fn draw_shortcuts(f: &mut Frame, area: Rect, _app: &App) {
    let line = Line::from(vec![
        Span::styled(" [q] ", Style::default().fg(SAND_GOLD)),
        Span::styled("quit  ", Style::default().fg(SUBDUED)),
        Span::styled("[↑↓] ", Style::default().fg(SAND_GOLD)),
        Span::styled("navigate  ", Style::default().fg(SUBDUED)),
        Span::styled("[↵] ", Style::default().fg(SAND_GOLD)),
        Span::styled("send  ", Style::default().fg(SUBDUED)),
        Span::styled("[/] ", Style::default().fg(SAND_GOLD)),
        Span::styled("command  ", Style::default().fg(SUBDUED)),
        Span::styled("[tab] ", Style::default().fg(SAND_GOLD)),
        Span::styled("next pane  ", Style::default().fg(SUBDUED)),
        Span::styled("[?] ", Style::default().fg(SAND_GOLD)),
        Span::styled("help", Style::default().fg(SUBDUED)),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(SHORTCUT_BG)),
        area,
    );
}

// -- Helpers ----------------------------------------------------

fn divider(width: u16) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::default().fg(PANEL_BORDER),
    )))
}

fn truncate(s: &str, max: usize) -> String {
    let max = max.max(4);
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}
