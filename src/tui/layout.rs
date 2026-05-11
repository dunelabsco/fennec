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

use super::app::{App, ChannelConnState, ChatLine, DetailsMode, Focus, SessionRow};
use super::skin::Skin;

pub fn draw(f: &mut Frame, app: &mut App) {
    let s = &app.skin;
    // Whole-screen background. Each panel paints over it.
    let bg = Block::default().style(Style::default().bg(s.bg_dusk).fg(s.text_cream));
    f.render_widget(bg, f.area());

    // Status-bar position controls how the outer vertical layout
    // is composed. `Bottom` (default) keeps the legacy
    // panes/status/shortcuts stack. `Top` pulls the status bar
    // above the panes (shortcuts stay at the bottom). `Off`
    // drops the status bar entirely; shortcuts still render.
    use crate::tui::app::StatusBarPosition;
    let (outer, status_idx_opt, panes_idx, shortcut_idx) = match app.statusbar_position {
        StatusBarPosition::Bottom => {
            let o = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(10),
                    Constraint::Length(1), // status bar
                    Constraint::Length(1), // shortcuts row
                ])
                .split(f.area());
            (o, Some(1usize), 0usize, 2usize)
        }
        StatusBarPosition::Top => {
            let o = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // status bar
                    Constraint::Min(10),
                    Constraint::Length(1), // shortcuts row
                ])
                .split(f.area());
            (o, Some(0usize), 1usize, 2usize)
        }
        StatusBarPosition::Off => {
            let o = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(10),
                    Constraint::Length(1), // shortcuts row
                ])
                .split(f.area());
            (o, None, 0usize, 1usize)
        }
    };

    let main_area = outer[panes_idx];
    let shortcut_area = outer[shortcut_idx];

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
    if let Some(idx) = status_idx_opt {
        draw_status(f, outer[idx], app, narrow);
    }
    draw_shortcuts(f, shortcut_area, app);

    // Fullscreen overlays render last so they paint over the
    // three-pane layout below. Like the modal layer in F1-2-A,
    // the underlying scrollback stays visible behind the
    // overlay's centered Clear-widget block.
    if app.show_agents_overlay {
        super::agents_overlay::draw_agents_overlay(f, f.area(), app);
    }
}

// -- Sessions panel ---------------------------------------------

fn draw_sessions(f: &mut Frame, area: Rect, app: &App) {
    let s = &app.skin;
    let border_color = if app.focus == Focus::Sessions {
        s.sand_gold
    } else {
        s.panel_border
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
        Span::styled(" [f]", Style::default().fg(s.sand_gold)),
        Span::styled("all ", Style::default().fg(s.text_cream)),
        Span::styled("[s]", Style::default().fg(s.sand_gold)),
        Span::styled("recent ", Style::default().fg(s.text_cream)),
        Span::styled("[/]", Style::default().fg(s.sand_gold)),
        Span::styled("find", Style::default().fg(s.text_cream)),
    ]);
    f.render_widget(Paragraph::new(controls), layout[0]);

    f.render_widget(divider(inner.width, s), layout[1]);

    let new_session = Line::from(vec![
        Span::styled(
            " + ",
            Style::default().fg(s.muted_green).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "new session",
            Style::default().fg(s.muted_green).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  [n]", Style::default().fg(s.subdued)),
    ]);
    f.render_widget(Paragraph::new(new_session), layout[2]);

    f.render_widget(divider(inner.width, s), layout[3]);

    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|row| session_row(row, area.width, s))
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(s.highlight_bg)
                .fg(s.text_cream)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" ");
    let mut state = ListState::default();
    state.select(Some(app.selected_session));
    f.render_stateful_widget(list, layout[4], &mut state);
}

fn session_row<'a>(row: &'a SessionRow, panel_width: u16, s: &Skin) -> ListItem<'a> {
    let count_color = if row.has_unread { s.amber } else { s.subdued };
    let count_modifier = if row.has_unread {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let code_color = code_color_for(&row.code, s);
    let subject_max = (panel_width as usize).saturating_sub(18 + row.who.chars().count());
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{} ", row.code),
            Style::default().fg(code_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} ", row.who),
            Style::default().fg(s.text_cream),
        ),
        Span::styled(
            truncate(&row.subject, subject_max),
            Style::default().fg(s.subdued),
        ),
        Span::styled(
            format!("  {}", row.count),
            Style::default().fg(count_color).add_modifier(count_modifier),
        ),
    ]))
}

/// Source-code color mapping. Mirrors the prototype's palette.
fn code_color_for(code: &str, s: &Skin) -> Color {
    match code.trim() {
        "TG" => Color::Rgb(0x6B, 0xA8, 0xC7),
        "SL" => s.tool_pink,
        "SG" => s.muted_green,
        "DC" => Color::Rgb(0x90, 0x9B, 0xC7),
        "MX" => s.tool_pink,
        "@" => s.sand_gold,
        "$" => s.amber,
        _ => s.text_cream,
    }
}

// -- Chat panel -------------------------------------------------

fn draw_chat(f: &mut Frame, area: Rect, app: &App, narrow: bool) {
    let s = &app.skin;
    let border_color = if app.focus == Focus::Chat || app.focus == Focus::Input {
        s.sand_gold
    } else {
        s.panel_border
    };
    let session_label = current_session_label(app);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(s.panel_border)),
            Span::styled("chat ", Style::default().fg(s.sand_gold)),
            Span::styled(session_label, Style::default().fg(s.subdued)),
            Span::styled(" ├", Style::default().fg(s.panel_border)),
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
    let s = &app.skin;
    let mut lines: Vec<Line> = Vec::new();
    let compact = app.compact_mode;
    for entry in &app.chat {
        match entry {
            ChatLine::System { time, body } => {
                if !compact {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(s.subdued)),
                        Span::styled(
                            "sys ",
                            Style::default().fg(s.subdued).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(time.clone(), Style::default().fg(s.subdued)),
                    ]));
                }
                lines.push(Line::from(Span::styled(
                    body.clone(),
                    Style::default().fg(s.subdued),
                )));
            }
            ChatLine::User { time, body } => {
                if !compact {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(s.amber)),
                        Span::styled(
                            "you ",
                            Style::default().fg(s.amber).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(time.clone(), Style::default().fg(s.subdued)),
                    ]));
                }
                let prefix = if compact { "› " } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(s.amber)),
                    Span::styled(body.clone(), Style::default().fg(s.text_cream)),
                ]));
            }
            ChatLine::Bot { time, body } => {
                if !compact {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(s.sand_gold)),
                        Span::styled(
                            "fennec ",
                            Style::default().fg(s.sand_gold).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(time.clone(), Style::default().fg(s.subdued)),
                    ]));
                }
                let prefix = if compact { "↳ " } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(s.sand_gold)),
                    Span::styled(body.clone(), Style::default().fg(s.text_cream)),
                ]));
            }
            ChatLine::ToolCall { call } => match app.details_mode {
                DetailsMode::Hidden => {
                    // Skip entirely — but /verbose forces tool
                    // calls back into view even when Hidden, on
                    // the assumption that "verbose" is the user
                    // saying "yes, show me more, not less."
                    if app.verbosity
                        == crate::tui::app::VerbosityMode::Verbose
                    {
                        lines.push(Line::from(vec![
                            Span::styled("    ▸ ", Style::default().fg(s.tool_pink)),
                            Span::styled(
                                call.clone(),
                                Style::default().fg(s.tool_pink),
                            ),
                        ]));
                    }
                }
                DetailsMode::Collapsed => {
                    // Header only — drop the args portion (the
                    // text after "name(...)") so a long path or
                    // SQL string doesn't wrap. We split at the
                    // first '(' to extract the bare tool name.
                    //
                    // /verbose flips this: in Verbose mode the
                    // collapsed view shows the full `call`
                    // (name + args) so the user can scan the
                    // arguments without expanding /details.
                    let show_full = app.verbosity
                        == crate::tui::app::VerbosityMode::Verbose;
                    let label = if show_full {
                        call.clone()
                    } else {
                        call.split_once('(')
                            .map(|(name, _rest)| name.to_string())
                            .unwrap_or_else(|| call.clone())
                    };
                    let bare = call
                        .split_once('(')
                        .map(|(name, _rest)| name)
                        .unwrap_or(call.as_str());
                    lines.push(Line::from(vec![
                        Span::styled("    ▸ ", Style::default().fg(s.tool_pink)),
                        Span::styled(
                            "tool",
                            Style::default()
                                .fg(s.tool_pink)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" · ", Style::default().fg(s.subdued)),
                        Span::styled(label, Style::default().fg(s.tool_pink)),
                    ]));
                    if bare == "delegate" {
                        append_inline_spawn_tree(&mut lines, app);
                    }
                }
                DetailsMode::Expanded => {
                    let bare = call
                        .split_once('(')
                        .map(|(name, _rest)| name)
                        .unwrap_or(call.as_str());
                    lines.push(Line::from(vec![
                        Span::styled("    ▸ ", Style::default().fg(s.tool_pink)),
                        Span::styled(
                            "tool",
                            Style::default()
                                .fg(s.tool_pink)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" · ", Style::default().fg(s.subdued)),
                        Span::styled(call.clone(), Style::default().fg(s.tool_pink)),
                    ]));
                    if bare == "delegate" {
                        append_inline_spawn_tree(&mut lines, app);
                    }
                }
            },
            ChatLine::ToolResult { summary } => match app.details_mode {
                DetailsMode::Hidden | DetailsMode::Collapsed => {
                    // Hidden: skip. Collapsed: skip too (the
                    // header in the matching ToolCall row is
                    // enough; the result body is the "detail"
                    // we're collapsing). Verbose mode overrides
                    // both — the user explicitly asked for more,
                    // not less.
                    if app.verbosity
                        == crate::tui::app::VerbosityMode::Verbose
                    {
                        lines.push(Line::from(vec![
                            Span::styled("    ↳ ", Style::default().fg(s.subdued)),
                            Span::styled(
                                summary.clone(),
                                Style::default().fg(s.subdued),
                            ),
                        ]));
                    }
                }
                DetailsMode::Expanded => {
                    lines.push(Line::from(vec![
                        Span::styled("    ↳ ", Style::default().fg(s.subdued)),
                        Span::styled(
                            summary.clone(),
                            Style::default().fg(s.subdued),
                        ),
                    ]));
                }
            },
            ChatLine::ToolRunning { label, started_at } => match app.details_mode {
                DetailsMode::Hidden => {
                    // Skip the live spinner too; the right
                    // panel still shows it for situational
                    // awareness.
                }
                _ => {
                    let elapsed = started_at.elapsed().as_millis();
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("    {} ", app.spinner_glyph()),
                            Style::default().fg(s.tool_pink),
                        ),
                        Span::styled(
                            format!("{label} ({elapsed}ms)"),
                            Style::default()
                                .fg(s.tool_pink)
                                .add_modifier(Modifier::DIM),
                        ),
                    ]));
                }
            },
        }
        // Spacer between entries — only in non-compact mode.
        // Compact view drops the blank rows so more turns fit
        // on screen.
        if !compact {
            lines.push(Line::raw(""));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// Compact inline rendering of the live spawn-tree, anchored
/// under a `delegate` tool call in chat scrollback. Only fires
/// when there's exactly one delegate call active (mirrors
/// Hermes' `thinking.tsx:889-890` rule). Multiple parallel
/// delegates skip the inline view and rely on `/agents` to surface.
fn append_inline_spawn_tree(lines: &mut Vec<Line<'static>>, app: &App) {
    let s = &app.skin;
    let tree = if !app.spawn_tree.is_empty() {
        &app.spawn_tree
    } else if let Some(snap) = app.spawn_history.get(0) {
        &snap.tree
    } else {
        return;
    };
    if tree.is_empty() || tree.root_ids.len() != 1 {
        return;
    }
    let peak = tree.peak_hotness();
    // Render each node depth-first, capped at 6 rows to keep
    // chat scrollback readable. Open /agents for the full tree.
    let mut flat: Vec<String> = Vec::new();
    for root in &tree.root_ids {
        push_inline_descendants(tree, root, &mut flat);
    }
    let total = flat.len();
    for id in flat.iter().take(6) {
        if let Some(node) = tree.nodes.get(id) {
            let indent = "  ".repeat(node.depth as usize + 2);
            let metrics = tree.aggregate(id);
            let bucket = tree.hot_bucket(id, peak);
            let heat = if bucket >= 2 {
                Span::styled(
                    "▍ ",
                    Style::default().fg(match bucket {
                        2 => s.amber,
                        3 => s.tool_pink,
                        _ => s.terracotta,
                    }),
                )
            } else {
                Span::styled("  ", Style::default())
            };
            let glyph_color = match node.status {
                super::spawn_tree::SubagentStatus::Completed => s.muted_green,
                super::spawn_tree::SubagentStatus::Failed
                | super::spawn_tree::SubagentStatus::Interrupted => s.terracotta,
                _ => s.amber,
            };
            let goal_preview: String = node
                .goal
                .chars()
                .take(40)
                .collect::<String>();
            let suffix = if metrics.local_tools > 0 {
                format!(" · {}t", metrics.local_tools)
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{indent}└─ "), Style::default().fg(s.subdued)),
                heat,
                Span::styled(
                    format!("{} ", node.status.glyph()),
                    Style::default().fg(glyph_color),
                ),
                Span::styled(goal_preview, Style::default().fg(s.text_cream)),
                Span::styled(suffix, Style::default().fg(s.subdued)),
            ]));
        }
    }
    if total > 6 {
        lines.push(Line::from(vec![
            Span::styled("        ", Style::default()),
            Span::styled(
                format!("…+{} more · /agents for full tree", total - 6),
                Style::default().fg(s.subdued).add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
}

fn push_inline_descendants(
    tree: &super::spawn_tree::SpawnTree,
    id: &str,
    out: &mut Vec<String>,
) {
    out.push(id.to_string());
    if let Some(node) = tree.nodes.get(id) {
        for child in &node.children {
            push_inline_descendants(tree, child, out);
        }
    }
}

fn draw_chat_bottom(f: &mut Frame, area: Rect, app: &App, narrow: bool) {
    let s = &app.skin;
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
                    Style::default().fg(s.tool_pink),
                ),
                Span::styled("running ", Style::default().fg(s.subdued)),
                Span::styled(t.name.clone(), Style::default().fg(s.tool_pink)),
                Span::styled(
                    format!(" ({}ms)  ", t.started_at.elapsed().as_millis()),
                    Style::default().fg(s.subdued),
                ),
                Span::styled("[t] ", Style::default().fg(s.sand_gold)),
                Span::styled("tool detail", Style::default().fg(s.subdued)),
            ])
        } else if let Some((msg, _)) = &app.transient_status {
            Line::from(Span::styled(
                format!(" {}", msg),
                Style::default().fg(s.subdued),
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
        .border_style(Style::default().fg(s.sand_gold));
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
            Span::styled(" › ", Style::default().fg(s.sand_gold)),
            Span::styled(cursor, Style::default().fg(s.sand_gold)),
            Span::styled("  type a message…", Style::default().fg(s.subdued)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(s.sand_gold)),
            Span::styled(app.input.text(), Style::default().fg(s.text_cream)),
            Span::styled(cursor, Style::default().fg(s.sand_gold)),
        ])
    };
    f.render_widget(Paragraph::new(prompt), inner);
    let _ = box_mid;
    idx += 2;

    // Hint row below the input.
    if idx < layout.len() {
        let hint = Line::from(vec![
            Span::styled(" [enter] ", Style::default().fg(s.sand_gold)),
            Span::styled("send  ", Style::default().fg(s.subdued)),
            Span::styled("[shift-enter] ", Style::default().fg(s.sand_gold)),
            Span::styled("newline  ", Style::default().fg(s.subdued)),
            Span::styled("[ctrl-c] ", Style::default().fg(s.sand_gold)),
            Span::styled("cancel turn", Style::default().fg(s.subdued)),
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
    let s = &app.skin;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(s.tool_pink));
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
                Style::default().fg(s.tool_pink),
            ),
            Span::styled("running ", Style::default().fg(s.subdued)),
            Span::styled(
                t.name.clone(),
                Style::default().fg(s.tool_pink).add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(Paragraph::new(header), layout[0]);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("args ", Style::default().fg(s.subdued)),
                Span::styled(t.args_preview.clone(), Style::default().fg(s.text_cream)),
            ])),
            layout[1],
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("started ", Style::default().fg(s.subdued)),
                Span::styled(
                    format!("{}ms ago", t.started_at.elapsed().as_millis()),
                    Style::default().fg(s.text_cream),
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
            .gauge_style(Style::default().fg(s.sand_gold).bg(s.highlight_bg))
            .percent(u16::from(progress))
            .label(Span::styled(
                format!("{progress}%"),
                Style::default().fg(s.text_cream),
            ));
        f.render_widget(gauge, layout[5]);
    } else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no tool running",
                Style::default().fg(s.subdued),
            ))),
            layout[0],
        );
    }

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "recent",
            Style::default().fg(s.subdued).add_modifier(Modifier::BOLD),
        ))),
        layout[6],
    );

    let recent: Vec<Line> = app
        .recent_tools
        .iter()
        .map(|t| {
            let (mark, mark_color) = if t.ok {
                ("✓ ", s.muted_green)
            } else {
                ("✗ ", s.terracotta)
            };
            Line::from(vec![
                Span::styled(mark, Style::default().fg(mark_color)),
                Span::styled(
                    format!("{} ", t.name),
                    Style::default().fg(s.text_cream),
                ),
                Span::styled(t.note.clone(), Style::default().fg(s.subdued)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(recent), layout[7]);
}

fn draw_channels(f: &mut Frame, area: Rect, app: &App) {
    let s = &app.skin;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(s.panel_border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = app
        .channels
        .iter()
        .map(|c| {
            let state_color = match c.state {
                ChannelConnState::Connected => s.muted_green,
                ChannelConnState::Polling => s.amber,
                ChannelConnState::Attached => s.sand_gold,
                ChannelConnState::Idle => s.subdued,
                ChannelConnState::Disconnected => s.subdued,
                ChannelConnState::Error => s.terracotta,
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
                        .fg(code_color_for(&c.code, s))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:<8}", c.name), Style::default().fg(s.text_cream)),
                Span::styled("● ", Style::default().fg(state_color)),
                Span::styled(format!("{:<10}", state_text), Style::default().fg(state_color)),
                Span::styled(c.detail.clone(), Style::default().fg(s.subdued)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

// -- Status + shortcuts row -------------------------------------

fn draw_status(f: &mut Frame, area: Rect, app: &App, narrow: bool) {
    let s = &app.skin;
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
                .bg(s.sand_gold)
                .fg(s.bg_dusk)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{}  ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(s.subdued),
        ),
        Span::styled("● ", Style::default().fg(s.muted_green)),
        Span::styled(
            format!("agent ready · {connected}/{total} channels"),
            Style::default().fg(s.muted_green),
        ),
    ];
    if narrow {
        spans.push(Span::styled("  ● ", Style::default().fg(s.muted_green)));
        spans.push(Span::styled(
            format!("channels {connected}/{total}"),
            Style::default().fg(s.muted_green),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(s.shortcut_bg)),
        area,
    );

    // Right-align a clock.
    let clock = chrono::Local::now().format("%H:%M:%S").to_string();
    let clock_line = Line::from(vec![Span::styled(
        format!("{clock} "),
        Style::default().fg(s.subdued),
    )])
    .alignment(Alignment::Right);
    f.render_widget(
        Paragraph::new(clock_line).style(Style::default().bg(s.shortcut_bg)),
        area,
    );
}

fn draw_shortcuts(f: &mut Frame, area: Rect, _app: &App) {
    let s = &_app.skin;
    let line = Line::from(vec![
        Span::styled(" [q] ", Style::default().fg(s.sand_gold)),
        Span::styled("quit  ", Style::default().fg(s.subdued)),
        Span::styled("[↑↓] ", Style::default().fg(s.sand_gold)),
        Span::styled("navigate  ", Style::default().fg(s.subdued)),
        Span::styled("[↵] ", Style::default().fg(s.sand_gold)),
        Span::styled("send  ", Style::default().fg(s.subdued)),
        Span::styled("[/] ", Style::default().fg(s.sand_gold)),
        Span::styled("command  ", Style::default().fg(s.subdued)),
        Span::styled("[tab] ", Style::default().fg(s.sand_gold)),
        Span::styled("next pane  ", Style::default().fg(s.subdued)),
        Span::styled("[?] ", Style::default().fg(s.sand_gold)),
        Span::styled("help", Style::default().fg(s.subdued)),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(s.shortcut_bg)),
        area,
    );
}

// -- Helpers ----------------------------------------------------

fn divider(width: u16, s: &Skin) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::default().fg(s.panel_border),
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
