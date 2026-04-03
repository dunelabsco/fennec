use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Status returned by the loop detector after analysing the recent tool call
/// window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopStatus {
    /// No loop detected.
    Ok,
    /// A potential loop pattern was found — the caller should inject a warning
    /// into the conversation but may continue.
    Warning(String),
    /// A definitive loop pattern was found — the caller should break the tool
    /// loop.
    Break(String),
}

/// Detects repetitive tool call patterns that indicate the agent is stuck in a
/// loop.
pub struct LoopDetector {
    /// Sliding window of `(tool_name, args_hash)` pairs.
    window: VecDeque<(String, u64)>,
    /// Maximum number of entries to keep in the window.
    max_window: usize,
}

impl LoopDetector {
    /// Create a new detector with the given window size.
    pub fn new(max_window: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(max_window),
            max_window,
        }
    }

    /// Record a tool invocation.
    pub fn record(&mut self, tool_name: &str, args: &serde_json::Value) {
        let args_hash = {
            let mut hasher = DefaultHasher::new();
            args.to_string().hash(&mut hasher);
            hasher.finish()
        };
        self.window.push_back((tool_name.to_string(), args_hash));
        if self.window.len() > self.max_window {
            self.window.pop_front();
        }
    }

    /// Check the current window for loop patterns.
    pub fn check(&self) -> LoopStatus {
        if self.window.is_empty() {
            return LoopStatus::Ok;
        }

        // --- Exact repeat: consecutive identical (name, hash) pairs from the
        // end. ---
        let last = self.window.back().unwrap();
        let mut exact_count = 0;
        for entry in self.window.iter().rev() {
            if entry == last {
                exact_count += 1;
            } else {
                break;
            }
        }

        if exact_count >= 5 {
            return LoopStatus::Break(format!(
                "Exact repeat loop: tool '{}' called {} times with identical arguments",
                last.0, exact_count
            ));
        }
        if exact_count >= 3 {
            return LoopStatus::Warning(format!(
                "Possible loop: tool '{}' called {} times with identical arguments",
                last.0, exact_count
            ));
        }

        // --- Ping-pong: last 8+ entries alternate between exactly 2 tool
        // names. ---
        if self.window.len() >= 8 {
            let tail: Vec<_> = self.window.iter().rev().take(8).collect();
            let a = &tail[0].0;
            let b = &tail[1].0;
            if a != b {
                let is_pingpong = tail.iter().enumerate().all(|(i, entry)| {
                    if i % 2 == 0 {
                        &entry.0 == a
                    } else {
                        &entry.0 == b
                    }
                });
                if is_pingpong {
                    return LoopStatus::Break(format!(
                        "Ping-pong loop: tools '{}' and '{}' alternating for 8+ calls",
                        a, b
                    ));
                }
            }
        }

        // --- No-progress: same tool name 5+ times (different args). ---
        if self.window.len() >= 5 {
            let tail: Vec<_> = self.window.iter().rev().take(5).collect();
            let name = &tail[0].0;
            let all_same_tool = tail.iter().all(|e| &e.0 == name);
            if all_same_tool {
                // Check that at least some args differ (otherwise exact-repeat
                // would have caught it above).
                let hashes: std::collections::HashSet<u64> =
                    tail.iter().map(|e| e.1).collect();
                if hashes.len() > 1 {
                    return LoopStatus::Warning(format!(
                        "No-progress: tool '{}' called 5+ times with different arguments",
                        name
                    ));
                }
            }
        }

        LoopStatus::Ok
    }
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new(20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_default() {
        let d = LoopDetector::default();
        assert_eq!(d.max_window, 20);
        assert_eq!(d.check(), LoopStatus::Ok);
    }

    #[test]
    fn test_record_limits_window() {
        let mut d = LoopDetector::new(3);
        for i in 0..5 {
            d.record("tool", &json!({"i": i}));
        }
        assert_eq!(d.window.len(), 3);
    }
}
