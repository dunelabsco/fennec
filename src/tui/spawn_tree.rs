//! Spawn-tree domain model for the `/agents` overlay.
//!
//! Tracks the live tree of delegated sub-agents (and any nested
//! delegations under them) plus a small FIFO history of completed
//! trees so `/replay` can revisit them. The renderer
//! (`agents_overlay.rs`) reads from these structures; mutations
//! happen in `apply_tui_event` as `TuiEvent::Subagent*` events
//! arrive from the agent's callback bridge.
//!
//! Shape mirrors what the upstream Hermes overlay tracks (per
//! `ui-tui/src/components/agentsOverlay.tsx:391-525`) — status,
//! goal, tool count, duration, hot-branch heat — but the rendering
//! follows Fennec's three-pane palette and side-by-side layout
//! rather than Hermes' mode-switching list↔detail pane.

use std::collections::HashMap;
use std::time::Instant;

use crate::agent::callbacks::{SubagentComplete, SubagentSpawn, ToolStart};

/// Maximum number of completed spawn trees retained in memory
/// for `/replay <N>`. Matches Hermes' HISTORY_LIMIT.
pub const SPAWN_HISTORY_CAP: usize = 10;

/// Lifecycle status of a single sub-agent node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentStatus {
    /// Spawn requested, not yet running.
    Queued,
    /// Active — main loop is executing.
    Running,
    /// Returned successfully.
    Completed,
    /// Returned with an error (max iterations, provider error,
    /// etc.).
    Failed,
    /// User aborted the sub-agent (e.g. via `x` in the overlay).
    Interrupted,
}

impl SubagentStatus {
    /// Visible glyph used by the renderer's tree column.
    pub fn glyph(self) -> &'static str {
        match self {
            SubagentStatus::Queued => "○",
            SubagentStatus::Running => "●",
            SubagentStatus::Completed => "✓",
            SubagentStatus::Failed => "✗",
            SubagentStatus::Interrupted => "■",
        }
    }

    /// Whether this status counts as "this branch has finished
    /// — no more events will land for it."
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            SubagentStatus::Completed | SubagentStatus::Failed | SubagentStatus::Interrupted
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            SubagentStatus::Queued => "queued",
            SubagentStatus::Running => "running",
            SubagentStatus::Completed => "completed",
            SubagentStatus::Failed => "failed",
            SubagentStatus::Interrupted => "interrupted",
        }
    }
}

/// One sub-agent in the spawn tree. `id` is the wire id from
/// `SubagentSpawn`; `parent_id` is `None` for roots (delegations
/// initiated by the main agent) or `Some` for nested spawns
/// (delegate-from-within-a-delegate).
#[derive(Debug, Clone)]
pub struct SubagentNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub goal: String,
    pub status: SubagentStatus,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
    /// Tools the sub-agent has invoked. Each entry mirrors the
    /// `ToolStart` payload — the renderer uses `name(preview)`
    /// to draw the inline `▸` tool list.
    pub tools: Vec<ToolStart>,
    /// Free-text progress notes (status messages from the
    /// sub-agent's own `on_status` calls).
    pub notes: Vec<String>,
    /// Final assistant text on success / error string on failure.
    /// Populated by `SubagentComplete`.
    pub output: Option<String>,
    /// Direct children's ids (depth-1 descendants in the tree).
    pub children: Vec<String>,
}

impl SubagentNode {
    /// Best-effort duration: live nodes use elapsed since
    /// `started_at`; finished nodes use `finished_at - started_at`.
    pub fn duration_ms(&self) -> u64 {
        match self.finished_at {
            Some(end) => end.duration_since(self.started_at).as_millis() as u64,
            None => self.started_at.elapsed().as_millis() as u64,
        }
    }
}

/// Aggregate metrics for a node + its descendants. Used by the
/// detail pane to show "tools 3 (subtree 12)" — local count plus
/// rolled-up subtree count.
#[derive(Debug, Clone, Copy, Default)]
pub struct AggregateMetrics {
    pub local_tools: usize,
    pub subtree_tools: usize,
    pub local_duration_ms: u64,
    /// Sum of every descendant's duration_ms (max of overlapping
    /// runs would be more "human"; this matches Hermes).
    pub subtree_duration_ms: u64,
    pub descendant_count: usize,
    pub max_depth: usize,
}

/// Live spawn tree. New events from the callback bridge mutate
/// this structure via [`Self::ingest_event`]; the renderer reads
/// the current snapshot every frame.
#[derive(Debug, Clone, Default)]
pub struct SpawnTree {
    pub nodes: HashMap<String, SubagentNode>,
    /// Ids with `parent_id == None`, in the order they were
    /// spawned. Renderer iterates these as the top of each
    /// subtree.
    pub root_ids: Vec<String>,
}

impl SpawnTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the tree has any nodes. Empty trees are skipped
    /// when promoting to history (matches Hermes' "drop empty
    /// snapshots").
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Whether every root subtree has reached a terminal status
    /// — used by the drain loop to decide when to push the live
    /// tree onto the history.
    pub fn is_settled(&self) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        self.nodes.values().all(|n| n.status.is_terminal())
    }

    /// Number of nodes total.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Top-level (parent_id is None) nodes.
    pub fn roots(&self) -> impl Iterator<Item = &SubagentNode> {
        self.root_ids
            .iter()
            .filter_map(move |id| self.nodes.get(id))
    }

    /// Apply one [`crate::tui::callbacks::TuiEvent::Subagent*`]
    /// event to the tree. Centralised here so the drain task in
    /// `main.rs` can stay a thin dispatcher.
    pub fn on_spawn(&mut self, spawn: SubagentSpawn) {
        let now = Instant::now();
        let parent_id = spawn.parent_id.clone();
        // Hook child id into parent's children list if the parent
        // exists — late spawns under a parent we never saw stay
        // as roots, since dropping them would lose data.
        if let Some(ref pid) = parent_id {
            if let Some(parent) = self.nodes.get_mut(pid) {
                parent.children.push(spawn.id.clone());
            }
        }
        let is_root = parent_id.is_none() || !self.nodes.contains_key(parent_id.as_deref().unwrap_or(""));
        if is_root {
            self.root_ids.push(spawn.id.clone());
        }
        let node = SubagentNode {
            id: spawn.id.clone(),
            parent_id,
            goal: spawn.goal,
            status: SubagentStatus::Queued,
            started_at: now,
            finished_at: None,
            tools: Vec::new(),
            notes: Vec::new(),
            output: None,
            children: Vec::new(),
        };
        self.nodes.insert(spawn.id, node);
    }

    pub fn on_start(&mut self, id: &str) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = SubagentStatus::Running;
            node.started_at = Instant::now();
        }
    }

    pub fn on_tool(&mut self, id: &str, start: ToolStart) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.tools.push(start);
        }
    }

    pub fn on_progress(&mut self, id: &str, note: String) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.notes.push(note);
        }
    }

    pub fn on_complete(&mut self, complete: SubagentComplete) {
        if let Some(node) = self.nodes.get_mut(&complete.id) {
            node.status = if complete.success {
                SubagentStatus::Completed
            } else {
                SubagentStatus::Failed
            };
            node.finished_at = Some(Instant::now());
            node.output = Some(complete.output);
        }
    }

    /// Mark a node (and its descendants, if `subtree=true`) as
    /// `Interrupted`. Used by the overlay's `x`/`X` keybindings.
    pub fn interrupt(&mut self, id: &str, subtree: bool) {
        let mut targets = vec![id.to_string()];
        if subtree {
            targets.extend(self.descendants(id));
        }
        let now = Instant::now();
        for tid in targets {
            if let Some(node) = self.nodes.get_mut(&tid) {
                if !node.status.is_terminal() {
                    node.status = SubagentStatus::Interrupted;
                    node.finished_at = Some(now);
                }
            }
        }
    }

    /// All descendant ids of `root_id` in depth-first order
    /// (excluding `root_id` itself). Empty if the id is unknown.
    pub fn descendants(&self, root_id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack: Vec<String> = self
            .nodes
            .get(root_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                stack.extend(node.children.iter().cloned());
            }
            out.push(id);
        }
        out
    }

    /// Roll up tool / duration / descendant counts for a node +
    /// its full subtree. Recurses through `children`.
    pub fn aggregate(&self, id: &str) -> AggregateMetrics {
        let Some(root) = self.nodes.get(id) else {
            return AggregateMetrics::default();
        };
        let mut metrics = AggregateMetrics {
            local_tools: root.tools.len(),
            local_duration_ms: root.duration_ms(),
            ..Default::default()
        };
        let mut max_depth = 0usize;
        let mut stack: Vec<(String, usize)> = root
            .children
            .iter()
            .map(|c| (c.clone(), 1))
            .collect();
        let mut subtree_tools = metrics.local_tools;
        let mut subtree_duration = metrics.local_duration_ms;
        let mut descendant_count = 0usize;
        while let Some((child_id, depth)) = stack.pop() {
            descendant_count += 1;
            max_depth = max_depth.max(depth);
            if let Some(child) = self.nodes.get(&child_id) {
                subtree_tools += child.tools.len();
                subtree_duration += child.duration_ms();
                for grand in &child.children {
                    stack.push((grand.clone(), depth + 1));
                }
            }
        }
        metrics.subtree_tools = subtree_tools;
        metrics.subtree_duration_ms = subtree_duration;
        metrics.descendant_count = descendant_count;
        metrics.max_depth = max_depth;
        metrics
    }

    /// "Hotness" = tools/sec for a single node, used to colour
    /// hot branches in the overlay. Returns 0.0 for nodes with
    /// no tools or zero duration.
    pub fn hotness(&self, id: &str) -> f64 {
        let Some(node) = self.nodes.get(id) else {
            return 0.0;
        };
        let dur_s = (node.duration_ms() as f64) / 1000.0;
        if dur_s <= 0.0 || node.tools.is_empty() {
            return 0.0;
        }
        node.tools.len() as f64 / dur_s
    }

    /// Peak hotness across the whole tree. Used as the
    /// normaliser for the hot-branch colour ramp.
    pub fn peak_hotness(&self) -> f64 {
        self.nodes
            .keys()
            .map(|id| self.hotness(id))
            .fold(0.0f64, f64::max)
    }

    /// Bucket index in [0, 4] for `id`'s hotness, normalised to
    /// `peak`. Passed to the renderer's colour palette.
    pub fn hot_bucket(&self, id: &str, peak: f64) -> usize {
        if peak <= 0.0 {
            return 0;
        }
        let h = self.hotness(id);
        let frac = (h / peak).clamp(0.0, 1.0);
        (frac * 4.0) as usize
    }
}

/// Snapshot of a settled spawn tree. Stored in [`SpawnHistory`]
/// for `/replay` to revisit. Plain `SpawnTree` underneath plus a
/// label + finished_at so the history list can render
/// human-readable rows.
#[derive(Debug, Clone)]
pub struct SpawnSnapshot {
    pub tree: SpawnTree,
    pub label: String,
    pub finished_at: chrono::DateTime<chrono::Local>,
}

/// 10-cap FIFO history of completed spawn trees.
#[derive(Debug, Clone, Default)]
pub struct SpawnHistory {
    snapshots: std::collections::VecDeque<SpawnSnapshot>,
}

impl SpawnHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a settled tree onto the history. Empty trees are
    /// dropped (matches Hermes' "drop empty" rule). Evicts the
    /// oldest entry once the cap is reached.
    pub fn push(&mut self, tree: SpawnTree) {
        if tree.is_empty() {
            return;
        }
        let label = label_for(&tree);
        let snapshot = SpawnSnapshot {
            tree,
            label,
            finished_at: chrono::Local::now(),
        };
        self.snapshots.push_front(snapshot);
        while self.snapshots.len() > SPAWN_HISTORY_CAP {
            self.snapshots.pop_back();
        }
    }

    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    /// Newest first — `0` is the most recent.
    pub fn get(&self, idx: usize) -> Option<&SpawnSnapshot> {
        self.snapshots.get(idx)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SpawnSnapshot> {
        self.snapshots.iter()
    }
}

/// Auto-generate a label from the goal of the first root +
/// a node count. Mirrors Hermes' `spawnHistoryStore.ts`
/// fallback heuristic.
fn label_for(tree: &SpawnTree) -> String {
    let count = tree.len();
    let first_goal = tree
        .roots()
        .next()
        .map(|n| truncate_goal(&n.goal, 60))
        .unwrap_or_else(|| "(no roots)".into());
    if count == 1 {
        first_goal
    } else {
        format!("{first_goal} (+{} more)", count - 1)
    }
}

fn truncate_goal(s: &str, max: usize) -> String {
    let collapsed = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect::<String>();
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let mut out: String = collapsed.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::callbacks::ToolStart;

    fn spawn(id: &str, parent: Option<&str>, goal: &str) -> SubagentSpawn {
        SubagentSpawn {
            id: id.to_string(),
            parent_id: parent.map(String::from),
            goal: goal.to_string(),
        }
    }

    fn tool(name: &str) -> ToolStart {
        ToolStart {
            tool_id: format!("tc_{name}"),
            name: name.to_string(),
            preview: format!("{name}(...)"),
        }
    }

    #[test]
    fn ingest_root_spawn_creates_node_and_adds_to_roots() {
        let mut t = SpawnTree::new();
        t.on_spawn(spawn("a", None, "research"));
        assert_eq!(t.len(), 1);
        assert_eq!(t.root_ids, vec!["a".to_string()]);
        assert_eq!(t.nodes["a"].status, SubagentStatus::Queued);
    }

    #[test]
    fn ingest_child_spawn_links_to_parent() {
        let mut t = SpawnTree::new();
        t.on_spawn(spawn("a", None, "root"));
        t.on_spawn(spawn("b", Some("a"), "child"));
        assert_eq!(t.root_ids, vec!["a".to_string()]);
        assert_eq!(t.nodes["a"].children, vec!["b".to_string()]);
        assert_eq!(t.nodes["b"].parent_id.as_deref(), Some("a"));
    }

    #[test]
    fn lifecycle_transitions_status() {
        let mut t = SpawnTree::new();
        t.on_spawn(spawn("a", None, "root"));
        t.on_start("a");
        assert_eq!(t.nodes["a"].status, SubagentStatus::Running);
        t.on_complete(SubagentComplete {
            id: "a".into(),
            output: "done".into(),
            success: true,
            duration_ms: 100,
            tools_used: vec![],
        });
        assert_eq!(t.nodes["a"].status, SubagentStatus::Completed);
        assert_eq!(t.nodes["a"].output.as_deref(), Some("done"));
    }

    #[test]
    fn aggregate_rolls_up_subtree_tool_counts() {
        let mut t = SpawnTree::new();
        t.on_spawn(spawn("a", None, "root"));
        t.on_spawn(spawn("b", Some("a"), "child"));
        t.on_spawn(spawn("c", Some("b"), "grandchild"));
        t.on_tool("a", tool("read"));
        t.on_tool("a", tool("write"));
        t.on_tool("b", tool("search"));
        t.on_tool("c", tool("fetch"));
        let m = t.aggregate("a");
        assert_eq!(m.local_tools, 2);
        assert_eq!(m.subtree_tools, 4);
        assert_eq!(m.descendant_count, 2);
        assert_eq!(m.max_depth, 2);
    }

    #[test]
    fn descendants_returns_all_levels() {
        let mut t = SpawnTree::new();
        t.on_spawn(spawn("a", None, "root"));
        t.on_spawn(spawn("b", Some("a"), "b"));
        t.on_spawn(spawn("c", Some("a"), "c"));
        t.on_spawn(spawn("d", Some("b"), "d"));
        let mut descendants = t.descendants("a");
        descendants.sort();
        assert_eq!(descendants, vec!["b".to_string(), "c".to_string(), "d".to_string()]);
    }

    #[test]
    fn interrupt_subtree_marks_all_descendants() {
        let mut t = SpawnTree::new();
        t.on_spawn(spawn("a", None, "root"));
        t.on_spawn(spawn("b", Some("a"), "child"));
        t.on_spawn(spawn("c", Some("b"), "grand"));
        t.on_start("a");
        t.on_start("b");
        t.on_start("c");
        t.interrupt("a", true);
        for id in ["a", "b", "c"] {
            assert_eq!(
                t.nodes[id].status,
                SubagentStatus::Interrupted,
                "{id}"
            );
        }
    }

    #[test]
    fn is_settled_only_when_every_node_terminal() {
        let mut t = SpawnTree::new();
        t.on_spawn(spawn("a", None, "root"));
        t.on_spawn(spawn("b", Some("a"), "child"));
        assert!(!t.is_settled());
        t.on_complete(SubagentComplete {
            id: "a".into(),
            output: "ok".into(),
            success: true,
            duration_ms: 1,
            tools_used: vec![],
        });
        assert!(!t.is_settled());
        t.on_complete(SubagentComplete {
            id: "b".into(),
            output: "ok".into(),
            success: true,
            duration_ms: 1,
            tools_used: vec![],
        });
        assert!(t.is_settled());
    }

    #[test]
    fn history_evicts_at_cap() {
        let mut h = SpawnHistory::new();
        for i in 0..(SPAWN_HISTORY_CAP + 5) {
            let mut tree = SpawnTree::new();
            tree.on_spawn(spawn(&format!("a{i}"), None, &format!("goal {i}")));
            h.push(tree);
        }
        assert_eq!(h.len(), SPAWN_HISTORY_CAP);
        // Newest first — the last 5 we pushed should still be there.
        let labels: Vec<_> = h.iter().map(|s| s.label.clone()).collect();
        assert!(labels[0].contains("goal 14"));
        assert!(labels[SPAWN_HISTORY_CAP - 1].contains("goal 5"));
    }

    #[test]
    fn history_drops_empty_trees() {
        let mut h = SpawnHistory::new();
        h.push(SpawnTree::new());
        assert!(h.is_empty());
    }

    #[test]
    fn hotness_buckets_low_and_high() {
        let mut t = SpawnTree::new();
        t.on_spawn(spawn("hot", None, "many tools"));
        t.on_spawn(spawn("cold", None, "few tools"));
        for _ in 0..10 {
            t.on_tool("hot", tool("a"));
        }
        t.on_tool("cold", tool("a"));
        // Sleep so duration_ms() crosses ~1 ms; otherwise the
        // sub-ms elapsed window rounds to zero and hotness is 0.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let peak = t.peak_hotness();
        assert!(peak > 0.0);
        // Hot bucket should be > cold bucket.
        let hot_b = t.hot_bucket("hot", peak);
        let cold_b = t.hot_bucket("cold", peak);
        assert!(hot_b >= cold_b);
    }
}
