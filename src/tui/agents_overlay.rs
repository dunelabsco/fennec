//! `/agents` fullscreen overlay — spawn-tree dashboard.
//!
//! Renders the live spawn tree (or the most-recent settled
//! snapshot from history when there's no live tree) as a
//! horizontal split: tree-list pane on the left (38 cols),
//! detail pane on the right (rest). Below 120 cols of overall
//! width the panes stack vertically so narrow terminals stay
//! usable.
//!
//! Visual language follows Fennec's existing palette + border
//! style, not Hermes' specific UX:
//!   - SAND_GOLD title + outer double border
//!   - SUBDUED metadata, MUTED_GREEN/AMBER/TERRACOTTA status dots
//!   - The same `▸ tool · name(args)` glyphs the chat panel uses
//!     for tool calls
//!   - Single-line key-hint footer in SAND_GOLD/SUBDUED
//!
//! Hot-branch heat marker uses an AMBER → TERRACOTTA gradient
//! (matching our existing alert palette). Hermes' algorithm is
//! ported verbatim — `tools / sec` normalised to tree peak.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use super::app::App;
use super::spawn_tree::{SpawnTree, SubagentStatus};
use super::theme::*;

/// Top-level overlay renderer. Sized to most of the screen with
/// a 4-cell margin so the underlying status bar / shortcuts row
/// stay visible behind it.
pub fn draw_agents_overlay(f: &mut Frame, screen: Rect, app: &App) {
    let width = screen.width.saturating_sub(4);
    let height = screen.height.saturating_sub(2);
    let area = Rect {
        x: screen.x + (screen.width.saturating_sub(width)) / 2,
        y: screen.y + (screen.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, area);

    let tree = active_tree(app);
    let total = tree.len();
    let title = format!(" /agents · spawn tree · {total} agent{}", plural(total));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(SAND_GOLD))
        .style(Style::default().bg(BG_DUSK))
        .title(Line::from(Span::styled(
            title,
            Style::default().fg(SAND_GOLD).add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let footer_height: u16 = 1;
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    let narrow = body[0].width < 120;
    let panes = if narrow {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(body[0])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(38), Constraint::Min(40)])
            .split(body[0])
    };

    draw_tree_pane(f, panes[0], app, tree);
    draw_detail_pane(f, panes[1], app, tree);
    draw_overlay_footer(f, body[1], tree);
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Pick the active tree to display. Live tree wins when
/// non-empty; otherwise fall back to the most recent settled
/// snapshot from history. Returns a reference into the App so
/// the renderer can read without cloning.
fn active_tree<'a>(app: &'a App) -> &'a SpawnTree {
    if !app.spawn_tree.is_empty() {
        &app.spawn_tree
    } else if let Some(snap) = app.spawn_history.get(0) {
        &snap.tree
    } else {
        &app.spawn_tree
    }
}

fn draw_tree_pane(f: &mut Frame, area: Rect, app: &App, tree: &SpawnTree) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(PANEL_BORDER))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(PANEL_BORDER)),
            Span::styled("tree", Style::default().fg(SAND_GOLD)),
            Span::styled(" ├", Style::default().fg(PANEL_BORDER)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if tree.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no sub-agents — delegate from a turn to populate",
                Style::default().fg(SUBDUED),
            ))),
            inner,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let peak = tree.peak_hotness();
    for root_id in &tree.root_ids {
        render_subtree(tree, root_id, 0, app.agents_cursor.as_deref(), peak, &mut lines);
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

/// Recursively render a node + its descendants as indented rows.
fn render_subtree(
    tree: &SpawnTree,
    id: &str,
    depth: usize,
    cursor_id: Option<&str>,
    peak: f64,
    out: &mut Vec<Line<'static>>,
) {
    let Some(node) = tree.nodes.get(id) else {
        return;
    };
    let indent = "  ".repeat(depth);
    let is_selected = cursor_id == Some(id);
    let metrics = tree.aggregate(id);
    let bucket = tree.hot_bucket(id, peak);
    let heat_color = heat_color_for(bucket);
    let goal_preview = truncate_goal(&node.goal, 28usize.saturating_sub(depth * 2));

    let row_bg = if is_selected { HIGHLIGHT_BG } else { BG_DUSK };
    let row_style = Style::default().bg(row_bg);

    let glyph_color = status_color(node.status);
    let mut spans = Vec::with_capacity(8);
    spans.push(Span::styled(format!("{indent}"), row_style));
    if bucket >= 2 {
        spans.push(Span::styled(
            "▍ ".to_string(),
            row_style.fg(heat_color),
        ));
    } else {
        spans.push(Span::styled("  ".to_string(), row_style));
    }
    spans.push(Span::styled(
        format!("{} ", node.status.glyph()),
        row_style.fg(glyph_color),
    ));
    spans.push(Span::styled(
        goal_preview,
        row_style
            .fg(TEXT_CREAM)
            .add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() }),
    ));
    spans.push(Span::styled(
        format!(" · {}t", metrics.local_tools),
        row_style.fg(SUBDUED),
    ));
    if metrics.descendant_count > 0 {
        spans.push(Span::styled(
            format!(" ↓{}", metrics.descendant_count),
            row_style.fg(SUBDUED),
        ));
    }
    out.push(Line::from(spans));

    for child_id in node.children.iter() {
        render_subtree(tree, child_id, depth + 1, cursor_id, peak, out);
    }
}

fn draw_detail_pane(f: &mut Frame, area: Rect, app: &App, tree: &SpawnTree) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(PANEL_BORDER))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(PANEL_BORDER)),
            Span::styled("detail", Style::default().fg(SAND_GOLD)),
            Span::styled(" ├", Style::default().fg(PANEL_BORDER)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(cursor_id) = app.agents_cursor.as_deref() else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no node selected",
                Style::default().fg(SUBDUED),
            ))),
            inner,
        );
        return;
    };
    let Some(node) = tree.nodes.get(cursor_id) else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "selected node not in active tree",
                Style::default().fg(SUBDUED),
            ))),
            inner,
        );
        return;
    };
    let metrics = tree.aggregate(cursor_id);
    let mut lines: Vec<Line> = Vec::new();
    let id_short = if node.id.len() > 12 {
        format!("{}…", &node.id[..11])
    } else {
        node.id.clone()
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("#{id_short} "),
            Style::default().fg(SAND_GOLD).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            truncate_goal(&node.goal, inner.width.saturating_sub(15) as usize),
            Style::default().fg(TEXT_CREAM).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::raw(""));
    lines.push(meta_line("status", node.status.label(), status_color(node.status)));
    lines.push(meta_line(
        "duration",
        &format!("{} ms", node.duration_ms()),
        SUBDUED,
    ));
    lines.push(meta_line(
        "tools",
        &format!("{} (subtree {})", metrics.local_tools, metrics.subtree_tools),
        SUBDUED,
    ));
    if metrics.descendant_count > 0 {
        lines.push(meta_line(
            "descendants",
            &format!("{} (max depth {})", metrics.descendant_count, metrics.max_depth),
            SUBDUED,
        ));
    }
    lines.push(Line::raw(""));

    if !node.tools.is_empty() {
        lines.push(Line::from(Span::styled(
            "tools used:",
            Style::default().fg(SAND_GOLD).add_modifier(Modifier::BOLD),
        )));
        for tool in node.tools.iter().take(20) {
            lines.push(Line::from(vec![
                Span::styled("  ▸ ", Style::default().fg(TOOL_PINK)),
                Span::styled(
                    tool.name.clone(),
                    Style::default().fg(TOOL_PINK).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" · ", Style::default().fg(SUBDUED)),
                Span::styled(
                    truncate_inline(&tool.preview, inner.width as usize),
                    Style::default().fg(SUBDUED),
                ),
            ]));
        }
        if node.tools.len() > 20 {
            lines.push(Line::from(Span::styled(
                format!("  …and {} more", node.tools.len() - 20),
                Style::default().fg(SUBDUED),
            )));
        }
        lines.push(Line::raw(""));
    }

    if !node.notes.is_empty() {
        lines.push(Line::from(Span::styled(
            "progress notes:",
            Style::default().fg(SAND_GOLD).add_modifier(Modifier::BOLD),
        )));
        for note in node.notes.iter().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
            lines.push(Line::from(vec![
                Span::styled("  · ", Style::default().fg(SUBDUED)),
                Span::styled(
                    truncate_inline(note, inner.width as usize),
                    Style::default().fg(SUBDUED),
                ),
            ]));
        }
        lines.push(Line::raw(""));
    }

    if let Some(ref out) = node.output {
        lines.push(Line::from(Span::styled(
            "output:",
            Style::default().fg(SAND_GOLD).add_modifier(Modifier::BOLD),
        )));
        // Show the first ~12 lines of output, wrap=trim:false
        // so paragraph takes care of long lines.
        for raw in out.split('\n').take(12) {
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(TEXT_CREAM),
            )));
        }
    }

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

fn meta_line(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<12}"),
            Style::default().fg(SUBDUED),
        ),
        Span::styled(value.to_string(), Style::default().fg(color)),
    ])
}

fn status_color(status: SubagentStatus) -> Color {
    match status {
        SubagentStatus::Queued => AMBER,
        SubagentStatus::Running => MUTED_GREEN,
        SubagentStatus::Completed => MUTED_GREEN,
        SubagentStatus::Failed => TERRACOTTA,
        SubagentStatus::Interrupted => SUBDUED,
    }
}

/// Hot-branch palette: amber → terracotta gradient. Buckets 0-1
/// stay un-coloured (no heat marker); 2-4 light up progressively.
fn heat_color_for(bucket: usize) -> Color {
    match bucket {
        2 => AMBER,
        3 => TOOL_PINK,
        4 => TERRACOTTA,
        _ => SUBDUED,
    }
}

fn draw_overlay_footer(f: &mut Frame, area: Rect, tree: &SpawnTree) {
    let live_marker = if tree.is_settled() && !tree.is_empty() {
        " · finished — inspecting last tree"
    } else if tree.is_empty() {
        " · no tree"
    } else {
        " · live"
    };
    let footer = Line::from(vec![
        Span::styled(" ↑↓/jk ", Style::default().fg(SAND_GOLD)),
        Span::styled("navigate ", Style::default().fg(SUBDUED)),
        Span::styled("[g/G] ", Style::default().fg(SAND_GOLD)),
        Span::styled("top/bottom ", Style::default().fg(SUBDUED)),
        Span::styled("[x] ", Style::default().fg(SAND_GOLD)),
        Span::styled("kill ", Style::default().fg(SUBDUED)),
        Span::styled("[X] ", Style::default().fg(SAND_GOLD)),
        Span::styled("kill subtree ", Style::default().fg(SUBDUED)),
        Span::styled("[q/esc] ", Style::default().fg(SAND_GOLD)),
        Span::styled("close", Style::default().fg(SUBDUED)),
        Span::styled(
            live_marker.to_string(),
            Style::default().fg(SUBDUED).add_modifier(Modifier::DIM),
        ),
    ]);
    f.render_widget(Paragraph::new(footer), area);
}

fn truncate_goal(s: &str, max: usize) -> String {
    let max = max.max(4);
    let collapsed: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let mut out: String = collapsed.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

fn truncate_inline(s: &str, max: usize) -> String {
    let max = max.max(8);
    let single: String = s.replace('\n', " ").chars().collect();
    if single.chars().count() <= max {
        single
    } else {
        let mut out: String = single.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}
