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
//! style, not the upstream's specific UX:
//!   - s.sand_gold title + outer double border
//!   - s.subdued metadata, s.muted_green/s.amber/s.terracotta status dots
//!   - The same `▸ tool · name(args)` glyphs the chat panel uses
//!     for tool calls
//!   - Single-line key-hint footer in s.sand_gold/s.subdued
//!
//! Hot-branch heat marker uses an s.amber → s.terracotta gradient
//! (matching our existing alert palette). the upstream's algorithm is
//! ported verbatim — `tools / sec` normalised to tree peak.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};

use super::app::App;
use super::skin::Skin;
use super::spawn_tree::{SpawnTree, SubagentStatus};

/// Top-level overlay renderer. Sized to most of the screen with
/// a 4-cell margin so the underlying status bar / shortcuts row
/// stay visible behind it.
pub fn draw_agents_overlay(f: &mut Frame, screen: Rect, app: &App) {
    let s = &app.skin;
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
    let agg = tree_totals(tree);
    let replay_badge = if app.agents_replay_mode() {
        format!(
            " · replay {}/{}",
            app.agents_history_index,
            app.spawn_history.len()
        )
    } else if !tree.is_empty() && app.agents_history_index == 0 && app.spawn_tree.is_empty()
    {
        " · finished".to_string()
    } else {
        String::new()
    };
    let caps_badge = app
        .delegation_registry
        .as_ref()
        .map(|reg| {
            let caps = reg.caps();
            let paused_marker = if reg.is_paused() { " · ⏸ paused" } else { "" };
            format!(
                " · caps d{}/{}{paused_marker}",
                caps.max_spawn_depth, caps.max_concurrent_children
            )
        })
        .unwrap_or_default();
    let title = format!(
        " /agents · {total} agent{} · d{} · {}t{replay_badge}{caps_badge} ",
        plural(total),
        agg.max_depth,
        agg.subtree_tools,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(s.sand_gold))
        .style(Style::default().bg(s.bg_dusk))
        .title(Line::from(Span::styled(
            title,
            Style::default().fg(s.sand_gold).add_modifier(Modifier::BOLD),
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

    // /replay-diff mode swaps the two-pane layout for a
    // side-by-side diff of the chosen snapshots.
    if let Some((a, b)) = app.agents_diff_pair {
        draw_diff_view(f, body[0], app, a, b);
    } else {
        draw_tree_pane(f, panes[0], app, tree);
        draw_detail_pane(f, panes[1], app, tree);
    }
    draw_overlay_footer(f, body[1], app, tree);
}

/// Side-by-side comparison of two completed spawn-tree snapshots.
/// Reads `(a, b)` as 1-based indices into `spawn_history`. Renders
/// summary stats for each + a delta row. Modelled on the upstream's
/// `replay-diff` (`agentsOverlay.tsx:573-678`) but trimmed to the
/// metrics Fennec's `AggregateMetrics` actually carries — no
/// per-token graph, no Gantt diff.
fn draw_diff_view(f: &mut Frame, area: Rect, app: &App, a: usize, b: usize) {
    let s = &app.skin;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(s.panel_border))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(s.panel_border)),
            Span::styled("diff", Style::default().fg(s.sand_gold)),
            Span::styled(
                format!(" · {a} → {b} "),
                Style::default().fg(s.subdued),
            ),
            Span::styled("├", Style::default().fg(s.panel_border)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let snap_a = app.spawn_history.get(a.saturating_sub(1));
    let snap_b = app.spawn_history.get(b.saturating_sub(1));
    let (Some(snap_a), Some(snap_b)) = (snap_a, snap_b) else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "snapshot pair out of range",
                Style::default().fg(s.subdued),
            ))),
            inner,
        );
        return;
    };

    // Layout: two snapshot panes side-by-side on top, a single
    // delta row underneath. The delta row makes the side-by-side
    // a real diff (not just two adjacent totals).
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(8)])
        .split(inner);
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(split[0]);
    draw_diff_pane(f, panes[0], "baseline", a, &snap_a.tree, s);
    draw_diff_pane(f, panes[1], "candidate", b, &snap_b.tree, s);
    draw_diff_deltas(f, split[1], &snap_a.tree, &snap_b.tree, s);
}

/// Render the Δ metrics row that makes diff-view a real diff.
/// One line per metric: `tools: 12 → 18 (+6)`. Sign is `+/-/±0`.
fn draw_diff_deltas(
    f: &mut Frame,
    area: Rect,
    a: &SpawnTree,
    b: &SpawnTree,
    s: &Skin,
) {
    let ta = tree_totals(a);
    let tb = tree_totals(b);
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        " Δ metrics",
        Style::default().fg(s.sand_gold).add_modifier(Modifier::BOLD),
    )));
    lines.push(delta_field_u(
        "tools",
        ta.subtree_tools as i64,
        tb.subtree_tools as i64,
        s,
    ));
    lines.push(delta_field_u(
        "duration",
        ta.subtree_duration_ms as i64,
        tb.subtree_duration_ms as i64,
        s,
    ));
    lines.push(delta_field_u(
        "depth",
        ta.max_depth as i64,
        tb.max_depth as i64,
        s,
    ));
    lines.push(delta_field_u(
        "tokens",
        (ta.input_tokens + ta.output_tokens) as i64,
        (tb.input_tokens + tb.output_tokens) as i64,
        s,
    ));
    if ta.cost_usd > 0.0 || tb.cost_usd > 0.0 {
        let delta = tb.cost_usd - ta.cost_usd;
        let sign = if delta > 0.0 {
            "+"
        } else if delta < 0.0 {
            "-"
        } else {
            "±"
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {:<10}", "cost"), Style::default().fg(s.subdued)),
            Span::styled(
                format!("${:.4} → ${:.4}  ({sign}${:.4})", ta.cost_usd, tb.cost_usd, delta.abs()),
                Style::default().fg(s.text_cream),
            ),
        ]));
    }
    if ta.files_touched > 0 || tb.files_touched > 0 {
        lines.push(delta_field_u(
            "files",
            ta.files_touched as i64,
            tb.files_touched as i64,
            s,
        ));
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area,
    );
}

fn delta_field_u(label: &str, a: i64, b: i64, s: &Skin) -> Line<'static> {
    let delta = b - a;
    let sign = match delta.cmp(&0) {
        std::cmp::Ordering::Greater => "+",
        std::cmp::Ordering::Less => "-",
        std::cmp::Ordering::Equal => "±",
    };
    Line::from(vec![
        Span::styled(format!(" {label:<10}"), Style::default().fg(s.subdued)),
        Span::styled(
            format!("{a} → {b}  ({sign}{})", delta.abs()),
            Style::default().fg(s.text_cream),
        ),
    ])
}

fn draw_diff_pane(
    f: &mut Frame,
    area: Rect,
    label: &str,
    index: usize,
    tree: &SpawnTree,
    s: &Skin,
) {
    let totals = tree_totals(tree);
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format!("[{index}] {label} "),
            Style::default().fg(s.sand_gold).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("· {} agent{}", tree.len(), plural(tree.len())),
            Style::default().fg(s.subdued),
        ),
    ]));
    lines.push(Line::raw(""));
    lines.push(diff_field("tools", &totals.subtree_tools.to_string(), s));
    lines.push(diff_field(
        "duration",
        &format!("{} ms", totals.subtree_duration_ms),
        s,
    ));
    lines.push(diff_field("depth", &totals.max_depth.to_string(), s));
    lines.push(diff_field(
        "tokens",
        &format!(
            "{} in · {} out",
            totals.input_tokens, totals.output_tokens
        ),
        s,
    ));
    if totals.cost_usd > 0.0 {
        lines.push(diff_field("cost", &format!("${:.4}", totals.cost_usd), s));
    }
    if totals.files_touched > 0 {
        lines.push(diff_field("files", &totals.files_touched.to_string(), s));
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area,
    );
}

fn diff_field(label: &str, value: &str, s: &Skin) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label:<10}"), Style::default().fg(s.subdued)),
        Span::styled(value.to_string(), Style::default().fg(s.text_cream)),
    ])
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Pick the active tree to display. Honours `agents_history_index`
/// (0 = live, N>0 = Nth-newest history snapshot) and falls back
/// to history[0] when the live tree is empty at index 0 so the
/// user isn't dropped into a blank overlay.
fn active_tree<'a>(app: &'a App) -> &'a SpawnTree {
    let primary = app.agents_effective_tree();
    if primary.is_empty() && app.agents_history_index == 0 {
        if let Some(snap) = app.spawn_history.get(0) {
            return &snap.tree;
        }
    }
    primary
}

/// Compute aggregate totals across every root in `tree`. Used by
/// the header chips (`d{depth} · {tools}t`).
fn tree_totals(tree: &SpawnTree) -> super::spawn_tree::AggregateMetrics {
    let mut totals = super::spawn_tree::AggregateMetrics::default();
    for root in &tree.root_ids {
        let m = tree.aggregate(root);
        totals.subtree_tools += m.subtree_tools;
        totals.subtree_duration_ms += m.subtree_duration_ms;
        totals.descendant_count += m.descendant_count + 1;
        totals.max_depth = totals.max_depth.max(m.max_depth + 1);
    }
    totals
}

/// 8-step unicode bar sparkline derived from agents-per-depth.
/// Zero columns render as a space so a sparse tree doesn't read
/// as uniform activity. Matches the upstream's `SPARK_RAMP`.
fn sparkline(widths: &[u32]) -> String {
    const RAMP: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let max = widths.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return String::new();
    }
    widths
        .iter()
        .map(|w| {
            if *w == 0 {
                " ".to_string()
            } else {
                let idx = ((*w as usize - 1) * (RAMP.len() - 1)) / max.max(1) as usize;
                RAMP[idx.min(RAMP.len() - 1)].to_string()
            }
        })
        .collect()
}

/// Count of nodes at each depth, indexed by depth.
fn width_by_depth(tree: &SpawnTree) -> Vec<u32> {
    let mut widths: Vec<u32> = Vec::new();
    for node in tree.nodes.values() {
        let d = node.depth as usize;
        if widths.len() <= d {
            widths.resize(d + 1, 0);
        }
        widths[d] += 1;
    }
    widths
}

/// Frequency of model names — top 4 entries joined as
/// `"haiku×3 · sonnet×1"`. Returns empty when no models are
/// recorded on the nodes.
fn status_mix(tree: &SpawnTree) -> String {
    use std::collections::HashMap;
    let mut counts: HashMap<String, u32> = HashMap::new();
    for node in tree.nodes.values() {
        if let Some(ref m) = node.model {
            let short = m.split('/').next_back().unwrap_or(m).to_string();
            *counts.entry(short).or_insert(0) += 1;
        }
    }
    if counts.is_empty() {
        return String::new();
    }
    let mut entries: Vec<(String, u32)> = counts.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries
        .into_iter()
        .take(4)
        .map(|(k, v)| format!("{k}×{v}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn draw_tree_pane(f: &mut Frame, area: Rect, app: &App, tree: &SpawnTree) {
    let s = &app.skin;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(s.panel_border))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(s.panel_border)),
            Span::styled("tree", Style::default().fg(s.sand_gold)),
            Span::styled(" ├", Style::default().fg(s.panel_border)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if tree.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no sub-agents — delegate from a turn to populate",
                Style::default().fg(s.subdued),
            ))),
            inner,
        );
        return;
    }

    // Reserve top 3 rows for the gantt strip when there's room.
    let gantt_h: u16 = 3;
    let (gantt_area, body_area) = if inner.height > gantt_h + 4 {
        (
            Some(Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: gantt_h,
            }),
            Rect {
                x: inner.x,
                y: inner.y + gantt_h,
                width: inner.width,
                height: inner.height - gantt_h,
            },
        )
    } else {
        (None, inner)
    };
    if let Some(area) = gantt_area {
        draw_gantt_strip(f, area, app, tree);
    }
    let inner = body_area;

    // Header strip inside the tree pane: sparkline + model mix.
    let widths = width_by_depth(tree);
    let spark = sparkline(&widths);
    let mix = status_mix(tree);
    let mut header_spans = Vec::new();
    if !spark.is_empty() {
        header_spans.push(Span::styled(spark, Style::default().fg(s.sand_gold)));
    }
    if !mix.is_empty() {
        if !header_spans.is_empty() {
            header_spans.push(Span::styled("  ", Style::default().fg(s.subdued)));
        }
        header_spans.push(Span::styled(mix, Style::default().fg(s.subdued)));
    }

    let mut lines: Vec<Line> = Vec::new();
    if !header_spans.is_empty() {
        lines.push(Line::from(header_spans));
        lines.push(Line::from(""));
    }

    let peak = tree.peak_hotness();
    let flat = app.agents_flat_node_ids();
    if flat.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("no rows match filter: {}", app.agents_filter.label()),
            Style::default().fg(s.subdued),
        )));
    } else {
        for id in &flat {
            if let Some(node) = tree.nodes.get(id) {
                lines.push(render_row(
                    tree,
                    node,
                    app.agents_cursor.as_deref(),
                    peak,
                    s,
                ));
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

/// Render a small horizontal time-bar strip for up to 6 nodes
/// (first roots in render order). Each row is `id · ▓▓▓` with
/// the bar showing the node's lifetime relative to the earliest
/// `started_at`. The cursor's row is highlighted. Matches
/// the upstream's `GanttStrip` (`agentsOverlay.tsx:216-348`) without
/// the live-tick refresh — ratatui's full-frame redraw already
/// covers that.
fn draw_gantt_strip(f: &mut Frame, area: Rect, app: &App, tree: &SpawnTree) {
    let s = &app.skin;
    use std::time::Instant;
    // Pick nodes: first 6 roots; if there are fewer than 6
    // roots, pad with their first descendants.
    let mut node_ids: Vec<String> = tree
        .root_ids
        .iter()
        .take(6)
        .cloned()
        .collect();
    if node_ids.len() < 6 {
        for n in tree.nodes.values() {
            if n.parent_id.is_some() && !node_ids.contains(&n.id) {
                node_ids.push(n.id.clone());
                if node_ids.len() == 6 {
                    break;
                }
            }
        }
    }
    if node_ids.is_empty() {
        return;
    }
    // Compute lifetime window.
    let now = Instant::now();
    let earliest = node_ids
        .iter()
        .filter_map(|id| tree.nodes.get(id))
        .map(|n| n.started_at)
        .min()
        .unwrap_or(now);
    let latest = node_ids
        .iter()
        .filter_map(|id| tree.nodes.get(id))
        .map(|n| n.finished_at.unwrap_or(now))
        .max()
        .unwrap_or(now);
    let total_ms = latest
        .duration_since(earliest)
        .as_millis()
        .max(1) as u64;
    let bar_w = area.width.saturating_sub(8) as u64; // leave 8 cells for label
    if bar_w == 0 {
        return;
    }
    let mut lines: Vec<Line> = Vec::with_capacity(node_ids.len());
    for id in node_ids.iter().take(area.height as usize) {
        let Some(node) = tree.nodes.get(id) else { continue };
        let end = node.finished_at.unwrap_or(now);
        let start_offset = node.started_at.duration_since(earliest).as_millis() as u64;
        let dur = end.duration_since(node.started_at).as_millis() as u64;
        let start_col = (start_offset * bar_w / total_ms) as usize;
        let end_col = ((start_offset + dur) * bar_w / total_ms) as usize;
        let bar_len = end_col.saturating_sub(start_col).max(1);
        let pad = " ".repeat(start_col.min(bar_w as usize));
        let bar: String = "▓".repeat(bar_len.min(bar_w as usize - start_col));
        let id_label = if id.len() > 6 {
            format!("{}…", &id[..5])
        } else {
            format!("{:<6}", id)
        };
        let is_selected = app.agents_cursor.as_deref() == Some(id.as_str());
        let bar_color = status_color(node.status, s);
        let label_color = if is_selected { s.sand_gold } else { s.subdued };
        lines.push(Line::from(vec![
            Span::styled(format!("{id_label} "), Style::default().fg(label_color)),
            Span::styled(pad, Style::default()),
            Span::styled(bar, Style::default().fg(bar_color)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// Render a single row for the tree pane. Uses the node's
/// `depth` for indent so the row reads as part of the tree even
/// after sort + filter shuffle the linear order.
fn render_row(
    tree: &SpawnTree,
    node: &super::spawn_tree::SubagentNode,
    cursor_id: Option<&str>,
    peak: f64,
    s: &Skin,
) -> Line<'static> {
    let indent = "  ".repeat(node.depth as usize);
    let is_selected = cursor_id == Some(node.id.as_str());
    let metrics = tree.aggregate(&node.id);
    let bucket = tree.hot_bucket(&node.id, peak);
    let heat_color = heat_color_for(bucket, s);
    let goal_preview =
        truncate_goal(&node.goal, 28usize.saturating_sub(node.depth as usize * 2));
    let row_bg = if is_selected { s.highlight_bg } else { s.bg_dusk };
    let row_style = Style::default().bg(row_bg);
    let glyph_color = status_color(node.status, s);
    let mut spans = Vec::with_capacity(8);
    spans.push(Span::styled(indent, row_style));
    if bucket >= 2 {
        spans.push(Span::styled("▍ ".to_string(), row_style.fg(heat_color)));
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
            .fg(s.text_cream)
            .add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() }),
    ));
    spans.push(Span::styled(
        format!(" · {}t", metrics.local_tools),
        row_style.fg(s.subdued),
    ));
    if metrics.descendant_count > 0 {
        spans.push(Span::styled(
            format!(" ↓{}", metrics.descendant_count),
            row_style.fg(s.subdued),
        ));
    }
    if metrics.active_count > 0 {
        spans.push(Span::styled(
            format!(" ⚡{}", metrics.active_count),
            row_style.fg(s.amber),
        ));
    }
    Line::from(spans)
}


fn draw_detail_pane(f: &mut Frame, area: Rect, app: &App, tree: &SpawnTree) {
    let s = &app.skin;
    let focused = app.agents_detail_focused;
    let border_color = if focused { s.sand_gold } else { s.panel_border };
    let title_label = if focused {
        Span::styled("detail · scrolling", Style::default().fg(s.sand_gold))
    } else {
        Span::styled("detail", Style::default().fg(s.sand_gold))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(s.panel_border)),
            title_label,
            Span::styled(" ├", Style::default().fg(s.panel_border)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(cursor_id) = app.agents_cursor.as_deref() else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no node selected",
                Style::default().fg(s.subdued),
            ))),
            inner,
        );
        return;
    };
    let Some(node) = tree.nodes.get(cursor_id) else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "selected node not in active tree",
                Style::default().fg(s.subdued),
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
            Style::default().fg(s.sand_gold).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} ", node.status.glyph()),
            Style::default().fg(status_color(node.status, s)),
        ),
        Span::styled(
            truncate_goal(&node.goal, inner.width.saturating_sub(18) as usize),
            Style::default().fg(s.text_cream).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::raw(""));

    // ── Header meta row (always visible) ─────────────────────
    lines.push(meta_line(
        "depth",
        &format!("{} · {}", node.depth, node.status.label()),
        status_color(node.status, s),
        s,
    ));
    if let Some(ref m) = node.model {
        lines.push(meta_line("model", m, s.subdued, s));
    }
    if !node.toolsets.is_empty() {
        lines.push(meta_line("toolsets", &node.toolsets.join(", "), s.subdued, s));
    }
    lines.push(meta_line(
        "tools",
        &format!("{} (subtree {})", metrics.local_tools, metrics.subtree_tools),
        s.subdued,
        s,
    ));
    lines.push(meta_line(
        "subtree",
        &format!(
            "{} agent{} · d{} · ⚡{}",
            metrics.descendant_count,
            if metrics.descendant_count == 1 { "" } else { "s" },
            metrics.max_depth,
            metrics.active_count,
        ),
        s.subdued,
        s,
    ));
    if node.duration_ms() > 0 {
        lines.push(meta_line(
            "elapsed",
            &format_duration_ms(node.duration_ms()),
            s.subdued,
            s,
        ));
    }
    if node.iteration > 0 {
        lines.push(meta_line("iteration", &node.iteration.to_string(), s.subdued, s));
    }
    if node.api_calls > 0 {
        lines.push(meta_line(
            "api calls",
            &node.api_calls.to_string(),
            s.subdued,
            s,
        ));
    }
    lines.push(Line::raw(""));

    // ── Budget section ───────────────────────────────────────
    let local_tokens = node.input_tokens + node.output_tokens;
    let subtree_tokens = metrics.input_tokens + metrics.output_tokens - local_tokens;
    let local_cost = node.cost_usd;
    let subtree_cost = metrics.cost_usd - local_cost;
    if local_tokens > 0 || local_cost > 0.0 || subtree_tokens > 0 || subtree_cost > 0.0 {
        push_section_header(&mut lines, "Budget", None, true, s);
        if local_tokens > 0 {
            let mut value = format!(
                "{} in · {} out",
                fmt_tokens(node.input_tokens),
                fmt_tokens(node.output_tokens),
            );
            if node.reasoning_tokens > 0 {
                value.push_str(&format!(" · {} reasoning", fmt_tokens(node.reasoning_tokens)));
            }
            lines.push(field_line("tokens", &value, s));
        }
        if local_cost > 0.0 {
            let mut value = fmt_cost(local_cost);
            if subtree_cost >= 0.01 {
                value.push_str(&format!(" · subtree +{}", fmt_cost(subtree_cost)));
            }
            lines.push(field_line("cost", &value, s));
        }
        if subtree_tokens > 0 {
            lines.push(field_line(
                "subtree tokens",
                &format!("+{}", fmt_tokens(subtree_tokens)),
                s,
            ));
        }
        lines.push(Line::raw(""));
    }

    // ── Files section ────────────────────────────────────────
    if !node.files_read.is_empty() || !node.files_written.is_empty() {
        push_section_header(
            &mut lines,
            "Files",
            Some(node.files_read.len() + node.files_written.len()),
            false,
            s,
        );
        for p in node.files_written.iter().take(8) {
            lines.push(Line::from(Span::styled(
                format!("  +{p}"),
                Style::default().fg(s.muted_green),
            )));
        }
        for p in node.files_read.iter().take(8) {
            lines.push(Line::from(vec![
                Span::styled("  · ", Style::default().fg(s.subdued)),
                Span::styled(p.clone(), Style::default().fg(s.text_cream)),
            ]));
        }
        let overflow = node.files_read.len().saturating_sub(8)
            + node.files_written.len().saturating_sub(8);
        if overflow > 0 {
            lines.push(Line::from(Span::styled(
                format!("  …+{overflow} more"),
                Style::default().fg(s.subdued),
            )));
        }
        lines.push(Line::raw(""));
    }

    // ── Tool calls section ───────────────────────────────────
    let tool_count = if node.tools.is_empty() {
        node.output_tail.len()
    } else {
        node.tools.len()
    };
    if tool_count > 0 {
        push_section_header(&mut lines, "Tool calls", Some(tool_count), true, s);
        if !node.tools.is_empty() {
            for tool in node.tools.iter().take(20) {
                lines.push(Line::from(vec![
                    Span::styled("  ▸ ", Style::default().fg(s.tool_pink)),
                    Span::styled(
                        tool.name.clone(),
                        Style::default().fg(s.tool_pink).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" · ", Style::default().fg(s.subdued)),
                    Span::styled(
                        truncate_inline(&tool.preview, inner.width as usize),
                        Style::default().fg(s.subdued),
                    ),
                ]));
            }
            if node.tools.len() > 20 {
                lines.push(Line::from(Span::styled(
                    format!("  …+{} more", node.tools.len() - 20),
                    Style::default().fg(s.subdued),
                )));
            }
        } else {
            // Archived snapshot fallback: show the output_tail
            // names if no live tools are recorded.
            for entry in node.output_tail.iter().take(20) {
                lines.push(Line::from(vec![
                    Span::styled("  ▸ ", Style::default().fg(s.tool_pink)),
                    Span::styled(
                        entry.tool.clone(),
                        Style::default().fg(s.tool_pink).add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
        }
        lines.push(Line::raw(""));
    }

    // ── Output tail section ──────────────────────────────────
    if !node.output_tail.is_empty() {
        push_section_header(
            &mut lines,
            "Output",
            Some(node.output_tail.len()),
            true,
            s,
        );
        for entry in node.output_tail.iter() {
            let tool_color = if entry.is_error { s.terracotta } else { s.sand_gold };
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    entry.tool.clone(),
                    Style::default().fg(tool_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", Style::default()),
                Span::styled(
                    truncate_inline(&entry.preview, inner.width as usize),
                    if entry.is_error {
                        Style::default().fg(s.terracotta)
                    } else {
                        Style::default().fg(s.text_cream)
                    },
                ),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // ── Progress section (notes) ─────────────────────────────
    if !node.notes.is_empty() {
        push_section_header(&mut lines, "Progress", Some(node.notes.len()), false, s);
        for note in node.notes.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
            lines.push(Line::from(vec![
                Span::styled("  · ", Style::default().fg(s.subdued)),
                Span::styled(
                    truncate_inline(note, inner.width as usize),
                    Style::default().fg(s.subdued),
                ),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // ── Summary section ──────────────────────────────────────
    if let Some(ref summary) = node.summary {
        push_section_header(&mut lines, "Summary", None, true, s);
        lines.push(Line::from(Span::styled(
            format!("  {summary}"),
            Style::default().fg(s.text_cream),
        )));
        lines.push(Line::raw(""));
    } else if let Some(ref out) = node.output {
        push_section_header(&mut lines, "Output text", None, true, s);
        for raw in out.split('\n').take(12) {
            lines.push(Line::from(Span::styled(
                format!("  {raw}"),
                Style::default().fg(s.text_cream),
            )));
        }
    }

    // Clamp the scroll offset to the content size so `G` (which
    // sets MAX/2) parks at the last visible row instead of past
    // the end. Rough heuristic: each line is one row pre-wrap.
    let viewport_h = inner.height.max(1);
    let max_scroll = (lines.len() as u16).saturating_sub(viewport_h);
    let scroll = app.agents_detail_scroll.min(max_scroll);

    f.render_widget(
        Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
    // Right-edge scrollbar: only render when there's something
    // to scroll. Sits in the rightmost column of the inner rect
    // (one cell wide, full height).
    if max_scroll > 0 && inner.width >= 1 {
        let bar_area = Rect {
            x: inner.x + inner.width - 1,
            y: inner.y,
            width: 1,
            height: inner.height,
        };
        let mut state = ScrollbarState::new(max_scroll as usize).position(scroll as usize);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(if app.agents_detail_focused {
                    s.sand_gold
                } else {
                    s.subdued
                })),
            bar_area,
            &mut state,
        );
    }
}

/// Push a section-header line to the detail-pane output. `▾`
/// signals the section is open, `▸` collapsed (rendered for
/// parity with the upstream's visual language — Fennec currently always
/// shows the section body when open, since no key toggle is
/// wired). `count` adds `(N)` when present.
fn push_section_header(
    lines: &mut Vec<Line<'static>>,
    title: &str,
    count: Option<usize>,
    open: bool,
    s: &Skin,
) {
    let glyph = if open { "▾" } else { "▸" };
    let count_suffix = count
        .map(|n| format!(" ({n})"))
        .unwrap_or_default();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{glyph} {title}{count_suffix}"),
            Style::default().fg(s.sand_gold).add_modifier(Modifier::BOLD),
        ),
    ]));
}

fn field_line(label: &str, value: &str, s: &Skin) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label:<10}"), Style::default().fg(s.subdued)),
        Span::styled(value.to_string(), Style::default().fg(s.text_cream)),
    ])
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn fmt_cost(usd: f64) -> String {
    if usd >= 1.0 {
        format!("${usd:.2}")
    } else if usd >= 0.01 {
        format!("${usd:.3}")
    } else if usd > 0.0 {
        format!("${usd:.4}")
    } else {
        "$0".to_string()
    }
}

fn format_duration_ms(ms: u64) -> String {
    let secs = ms as f64 / 1000.0;
    if secs >= 60.0 {
        let m = (secs / 60.0).floor() as u64;
        let s = secs - (m as f64 * 60.0);
        format!("{m}m {s:.1}s")
    } else if secs >= 1.0 {
        format!("{secs:.1}s")
    } else {
        format!("{ms} ms")
    }
}

fn meta_line(label: &str, value: &str, color: Color, s: &Skin) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<12}"),
            Style::default().fg(s.subdued),
        ),
        Span::styled(value.to_string(), Style::default().fg(color)),
    ])
}

fn status_color(status: SubagentStatus, s: &Skin) -> Color {
    match status {
        SubagentStatus::Queued => s.amber,
        SubagentStatus::Running => s.muted_green,
        SubagentStatus::Completed => s.muted_green,
        SubagentStatus::Failed => s.terracotta,
        SubagentStatus::Interrupted => s.subdued,
    }
}

/// Hot-branch palette: amber → terracotta gradient. Buckets 0-1
/// stay un-coloured (no heat marker); 2-4 light up progressively.
fn heat_color_for(bucket: usize, s: &Skin) -> Color {
    match bucket {
        2 => s.amber,
        3 => s.tool_pink,
        4 => s.terracotta,
        _ => s.subdued,
    }
}

fn draw_overlay_footer(f: &mut Frame, area: Rect, app: &App, tree: &SpawnTree) {
    let s = &app.skin;
    // Transient flash takes priority over the legend when set.
    if let Some((body, _)) = app.agents_flash.as_ref() {
        let flash = Paragraph::new(Line::from(Span::styled(
            format!(" ⚑ {body}"),
            Style::default().fg(s.amber).add_modifier(Modifier::BOLD),
        )));
        f.render_widget(flash, area);
        return;
    }
    let mode_marker = if app.agents_replay_mode() {
        format!(
            " · replay {}/{}",
            app.agents_history_index,
            app.spawn_history.len()
        )
    } else if tree.is_empty() {
        " · no tree".to_string()
    } else if tree.is_settled() {
        " · settled".to_string()
    } else {
        " · live".to_string()
    };
    let footer = Line::from(vec![
        Span::styled(" ↑↓/jk ", Style::default().fg(s.sand_gold)),
        Span::styled("nav ", Style::default().fg(s.subdued)),
        Span::styled("[s] ", Style::default().fg(s.sand_gold)),
        Span::styled(
            format!("sort:{} ", app.agents_sort.label()),
            Style::default().fg(s.subdued),
        ),
        Span::styled("[f] ", Style::default().fg(s.sand_gold)),
        Span::styled(
            format!("filter:{} ", app.agents_filter.label()),
            Style::default().fg(s.subdued),
        ),
        Span::styled("[]/<> ", Style::default().fg(s.sand_gold)),
        Span::styled("history ", Style::default().fg(s.subdued)),
        Span::styled("[p] ", Style::default().fg(s.sand_gold)),
        Span::styled("pause ", Style::default().fg(s.subdued)),
        Span::styled("[x/X] ", Style::default().fg(s.sand_gold)),
        Span::styled("kill[+sub] ", Style::default().fg(s.subdued)),
        Span::styled("[q/esc] ", Style::default().fg(s.sand_gold)),
        Span::styled("close", Style::default().fg(s.subdued)),
        Span::styled(
            mode_marker,
            Style::default().fg(s.subdued).add_modifier(Modifier::DIM),
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
