//! Shared delegation state — the pause flag, concurrency caps,
//! active sub-agent registry, sibling-index counter, and depth
//! lookup that every `DelegateTool` invocation reads and mutates.
//!
//! Lives one layer above [`crate::agent::subagent::SubagentManager`]
//! (which is constructed fresh per invocation) so a single
//! `Arc<DelegationRegistry>` can be shared across the whole
//! process: the parent `DelegateTool`, every nested sub-agent's
//! own `DelegateTool`, and the TUI's `/agents` overlay.
//!
//! Inspired by the upstream's module-level `_spawn_paused` /
//! `_active_subagents` globals (`tools/delegate_tool.py:140-220`)
//! but realised as an explicit owned value so tests can construct
//! isolated registries.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

/// Per-process delegation caps. Numbers track upstream defaults
/// so behaviour is predictable for users coming from there.
#[derive(Debug, Clone, Copy)]
pub struct DelegationCaps {
    /// Maximum spawn depth (0 = main agent only, 1 = main may
    /// spawn children, 2 = grandchildren allowed, etc.). Default
    /// 2 = parent + child + grandchild.
    pub max_spawn_depth: u32,
    /// Maximum concurrent children per parent. Default 3.
    pub max_concurrent_children: u32,
}

impl Default for DelegationCaps {
    fn default() -> Self {
        Self {
            max_spawn_depth: 2,
            max_concurrent_children: 3,
        }
    }
}

/// A live sub-agent tracked in the registry. Used by the `/agents
/// status` RPC, the `x` / `X` kill paths, and the depth-lookup
/// that nested spawns consult.
#[derive(Debug, Clone)]
pub struct ActiveSubagent {
    pub id: String,
    pub parent_id: Option<String>,
    pub depth: u32,
    pub goal: String,
    pub model: Option<String>,
    /// Per-agent interrupt flag — set by `interrupt()` /
    /// `interrupt_subtree()` and polled by the inner agent loop
    /// (`Agent::turn`) at each tool-iteration boundary.
    pub interrupt_flag: Arc<AtomicBool>,
}

/// Why a `try_register` call returned an error. Mapped to a
/// human-readable error message via [`Self::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnRefusal {
    Paused,
    DepthExceeded { current: u32, cap: u32 },
    ConcurrencyExceeded { current: u32, cap: u32 },
}

impl SpawnRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::Paused => {
                "delegation is paused — call /agents resume to resume spawning".to_string()
            }
            Self::DepthExceeded { current, cap } => {
                format!("max spawn depth exceeded: requested depth {current} > cap {cap}")
            }
            Self::ConcurrencyExceeded { current, cap } => format!(
                "max concurrent children exceeded for this parent: {current} active >= cap {cap}"
            ),
        }
    }
}

#[derive(Debug)]
struct Inner {
    paused: bool,
    caps: DelegationCaps,
    active: HashMap<String, ActiveSubagent>,
    /// Next sibling index per parent_id (`None` key = root spawns).
    sibling_index: HashMap<Option<String>, u32>,
    /// Depth lookup so a nested spawn can ask "what was my parent's
    /// depth?" — needed because `SubagentManager` is constructed
    /// fresh per invocation and doesn't keep its own depth field.
    depth_lookup: HashMap<String, u32>,
}

/// Process-wide delegation state shared via `Arc`.
#[derive(Debug, Clone)]
pub struct DelegationRegistry {
    inner: Arc<Mutex<Inner>>,
}

impl Default for DelegationRegistry {
    fn default() -> Self {
        Self::new(DelegationCaps::default())
    }
}

impl DelegationRegistry {
    pub fn new(caps: DelegationCaps) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                paused: false,
                caps,
                active: HashMap::new(),
                sibling_index: HashMap::new(),
                depth_lookup: HashMap::new(),
            })),
        }
    }

    pub fn set_paused(&self, paused: bool) -> bool {
        let mut g = self.inner.lock();
        g.paused = paused;
        g.paused
    }

    pub fn is_paused(&self) -> bool {
        self.inner.lock().paused
    }

    pub fn caps(&self) -> DelegationCaps {
        self.inner.lock().caps
    }

    /// Snapshot of (paused, caps) for `/agents status`.
    pub fn status(&self) -> (bool, DelegationCaps) {
        let g = self.inner.lock();
        (g.paused, g.caps)
    }

    /// Try to register a new sub-agent. Returns the assigned
    /// `(depth, index, interrupt_flag)` on success; on refusal
    /// returns a [`SpawnRefusal`] explaining why.
    pub fn try_register(
        &self,
        id: String,
        parent_id: Option<String>,
        goal: String,
        model: Option<String>,
    ) -> Result<(u32, u32, Arc<AtomicBool>), SpawnRefusal> {
        let mut g = self.inner.lock();
        if g.paused {
            return Err(SpawnRefusal::Paused);
        }
        let depth = match &parent_id {
            None => 0,
            Some(pid) => g.depth_lookup.get(pid).copied().unwrap_or(0) + 1,
        };
        if depth > g.caps.max_spawn_depth {
            return Err(SpawnRefusal::DepthExceeded {
                current: depth,
                cap: g.caps.max_spawn_depth,
            });
        }
        let sibling_count = g
            .active
            .values()
            .filter(|s| s.parent_id == parent_id)
            .count() as u32;
        if sibling_count >= g.caps.max_concurrent_children {
            return Err(SpawnRefusal::ConcurrencyExceeded {
                current: sibling_count,
                cap: g.caps.max_concurrent_children,
            });
        }
        let entry = g.sibling_index.entry(parent_id.clone()).or_insert(0);
        let index = *entry;
        *entry += 1;
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        g.depth_lookup.insert(id.clone(), depth);
        g.active.insert(
            id.clone(),
            ActiveSubagent {
                id,
                parent_id,
                depth,
                goal,
                model,
                interrupt_flag: Arc::clone(&interrupt_flag),
            },
        );
        Ok((depth, index, interrupt_flag))
    }

    /// Remove a sub-agent from the active registry. Idempotent;
    /// no-op if `id` isn't present. The `depth_lookup` entry is
    /// preserved so any in-flight grandchild spawn that still
    /// references this parent can resolve its depth correctly.
    pub fn unregister(&self, id: &str) {
        let mut g = self.inner.lock();
        g.active.remove(id);
    }

    /// Set the interrupt flag for `id`. Returns `true` if a match
    /// was found. The flag is polled by `Agent::turn` at each
    /// tool-iteration boundary; this is cooperative — the
    /// sub-agent's current API call still completes before the
    /// interrupt takes effect.
    pub fn interrupt(&self, id: &str) -> bool {
        let g = self.inner.lock();
        match g.active.get(id) {
            Some(s) => {
                s.interrupt_flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Interrupt `root_id` and every descendant currently active.
    /// Returns the count of subagents signalled.
    pub fn interrupt_subtree(&self, root_id: &str) -> usize {
        let g = self.inner.lock();
        let mut signalled = 0usize;
        let mut to_visit = vec![root_id.to_string()];
        while let Some(id) = to_visit.pop() {
            if let Some(s) = g.active.get(&id) {
                s.interrupt_flag.store(true, Ordering::SeqCst);
                signalled += 1;
                for child in g.active.values() {
                    if child.parent_id.as_deref() == Some(&id) {
                        to_visit.push(child.id.clone());
                    }
                }
            }
        }
        signalled
    }

    /// Snapshot of every active sub-agent for `/agents status` /
    /// the overlay's caps badge.
    pub fn active_snapshot(&self) -> Vec<ActiveSubagent> {
        self.inner.lock().active.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_root_at_depth_zero() {
        let r = DelegationRegistry::default();
        let (depth, idx, _flag) = r
            .try_register("a".into(), None, "do x".into(), None)
            .unwrap();
        assert_eq!(depth, 0);
        assert_eq!(idx, 0);
    }

    #[test]
    fn child_inherits_parent_depth_plus_one() {
        let r = DelegationRegistry::default();
        r.try_register("a".into(), None, "do x".into(), None).unwrap();
        let (depth, _idx, _flag) = r
            .try_register("b".into(), Some("a".into()), "sub task".into(), None)
            .unwrap();
        assert_eq!(depth, 1);
    }

    #[test]
    fn sibling_indices_increment_per_parent() {
        let r = DelegationRegistry::default();
        let (_, i0, _) = r
            .try_register("a".into(), None, "x".into(), None)
            .unwrap();
        let (_, i1, _) = r
            .try_register("b".into(), None, "y".into(), None)
            .unwrap();
        r.try_register("c".into(), Some("a".into()), "sub".into(), None)
            .unwrap();
        let (_, i_d, _) = r
            .try_register("d".into(), Some("a".into()), "sub2".into(), None)
            .unwrap();
        assert_eq!((i0, i1, i_d), (0, 1, 1));
    }

    #[test]
    fn paused_registry_refuses_registration() {
        let r = DelegationRegistry::default();
        r.set_paused(true);
        let err = r
            .try_register("a".into(), None, "x".into(), None)
            .unwrap_err();
        assert_eq!(err, SpawnRefusal::Paused);
    }

    #[test]
    fn depth_cap_refuses_too_deep() {
        let r = DelegationRegistry::new(DelegationCaps {
            max_spawn_depth: 1,
            max_concurrent_children: 5,
        });
        r.try_register("a".into(), None, "x".into(), None).unwrap();
        r.try_register("b".into(), Some("a".into()), "sub".into(), None)
            .unwrap();
        let err = r
            .try_register("c".into(), Some("b".into()), "deeper".into(), None)
            .unwrap_err();
        assert!(matches!(err, SpawnRefusal::DepthExceeded { current: 2, cap: 1 }));
    }

    #[test]
    fn concurrency_cap_refuses_extra_sibling() {
        let r = DelegationRegistry::new(DelegationCaps {
            max_spawn_depth: 5,
            max_concurrent_children: 2,
        });
        r.try_register("a".into(), None, "1".into(), None).unwrap();
        r.try_register("b".into(), None, "2".into(), None).unwrap();
        let err = r
            .try_register("c".into(), None, "3".into(), None)
            .unwrap_err();
        assert!(matches!(
            err,
            SpawnRefusal::ConcurrencyExceeded { current: 2, cap: 2 }
        ));
    }

    #[test]
    fn unregister_frees_concurrency_slot() {
        let r = DelegationRegistry::new(DelegationCaps {
            max_spawn_depth: 5,
            max_concurrent_children: 1,
        });
        r.try_register("a".into(), None, "x".into(), None).unwrap();
        assert!(r
            .try_register("b".into(), None, "y".into(), None)
            .is_err());
        r.unregister("a");
        // Now a new sibling can spawn under the same parent.
        assert!(r
            .try_register("b".into(), None, "y".into(), None)
            .is_ok());
    }

    #[test]
    fn interrupt_sets_flag_for_target_only() {
        let r = DelegationRegistry::default();
        let (_, _, fa) = r
            .try_register("a".into(), None, "x".into(), None)
            .unwrap();
        let (_, _, fb) = r
            .try_register("b".into(), None, "y".into(), None)
            .unwrap();
        assert!(r.interrupt("a"));
        assert!(fa.load(Ordering::SeqCst));
        assert!(!fb.load(Ordering::SeqCst));
    }

    #[test]
    fn interrupt_subtree_signals_root_and_descendants() {
        let r = DelegationRegistry::default();
        let (_, _, fa) = r
            .try_register("a".into(), None, "x".into(), None)
            .unwrap();
        let (_, _, fb) = r
            .try_register("b".into(), Some("a".into()), "y".into(), None)
            .unwrap();
        let (_, _, fc) = r
            .try_register("c".into(), Some("b".into()), "z".into(), None)
            .unwrap();
        let (_, _, fd) = r
            .try_register("d".into(), None, "unrelated".into(), None)
            .unwrap();
        let count = r.interrupt_subtree("a");
        assert_eq!(count, 3);
        assert!(fa.load(Ordering::SeqCst));
        assert!(fb.load(Ordering::SeqCst));
        assert!(fc.load(Ordering::SeqCst));
        assert!(!fd.load(Ordering::SeqCst));
    }

    #[test]
    fn status_reports_paused_and_caps() {
        let r = DelegationRegistry::new(DelegationCaps {
            max_spawn_depth: 4,
            max_concurrent_children: 7,
        });
        r.set_paused(true);
        let (paused, caps) = r.status();
        assert!(paused);
        assert_eq!(caps.max_spawn_depth, 4);
        assert_eq!(caps.max_concurrent_children, 7);
    }
}
