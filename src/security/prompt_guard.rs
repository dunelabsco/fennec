use std::sync::OnceLock;

use regex::{Regex, RegexSet};

/// Action to take when a prompt injection is detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardAction {
    Warn,
    Block,
    Sanitize,
}

/// Result of scanning an input for prompt injection attempts.
#[derive(Debug, Clone)]
pub enum ScanResult {
    /// Input appears safe.
    Safe,
    /// Input matches suspicious patterns with associated category names and a
    /// normalised confidence score.
    Suspicious(Vec<String>, f64),
    /// Input is blocked outright (only when action is [`GuardAction::Block`]
    /// and the score exceeds the sensitivity threshold).
    Blocked(String),
}

/// Guard that detects prompt injection patterns in user input.
pub struct PromptGuard {
    action: GuardAction,
    sensitivity: f64,
}

// ---------------------------------------------------------------------------
// Pattern categories and compiled regex sets
// ---------------------------------------------------------------------------

/// A category of patterns. Most categories use a `RegexSet` for speed; the
/// role_confusion category uses individual `Regex` objects because one pattern
/// needs a manual negative check (Rust regex doesn't support look-ahead).
enum CategoryMatcher {
    Set(RegexSet),
    /// Role confusion needs special handling for the "you are now" pattern,
    /// which must exclude "going", "about", "ready" as the next word.
    RoleConfusion {
        you_are_now: Regex,
        you_are_now_safe: Regex,
        others: RegexSet,
    },
}

struct PatternCategory {
    name: &'static str,
    score: f64,
    matcher: CategoryMatcher,
}

impl PatternCategory {
    fn is_match(&self, input: &str) -> bool {
        match &self.matcher {
            CategoryMatcher::Set(set) => set.is_match(input),
            CategoryMatcher::RoleConfusion {
                you_are_now,
                you_are_now_safe,
                others,
            } => {
                // Check the "you are now" pattern: match if it fires AND
                // the safe continuation pattern does NOT match.
                if you_are_now.is_match(input) && !you_are_now_safe.is_match(input) {
                    return true;
                }
                others.is_match(input)
            }
        }
    }
}

struct PatternSet {
    categories: Vec<PatternCategory>,
}

/// Compile all pattern categories once.
fn compile_patterns() -> PatternSet {
    let categories = vec![
        PatternCategory {
            name: "system_override",
            score: 1.0,
            matcher: CategoryMatcher::Set(
                RegexSet::new([
                    r"(?i)ignore\s+previous\s+instructions",
                    r"(?i)disregard\b.*\b(instructions|rules|guidelines|system|prompt)",
                    r"(?i)forget\s+(your\s+)?instructions",
                    r"(?i)new\s+instructions\s*:",
                    r"(?i)override\s+system\s+prompt",
                    r"(?i)reset\s+instructions",
                ])
                .expect("system_override patterns"),
            ),
        },
        PatternCategory {
            name: "secret_extraction",
            score: 0.95,
            matcher: CategoryMatcher::Set(
                RegexSet::new([
                    r"(?i)show\s+me\s+all\s+(api\s+keys|secrets)",
                    r"(?i)(dump|reveal|expose)\s+(the\s+)?vault",
                    r"(?i)what\s+is\s+your\s+(api\s+key|secret|password)",
                ])
                .expect("secret_extraction patterns"),
            ),
        },
        PatternCategory {
            name: "role_confusion",
            score: 0.9,
            matcher: CategoryMatcher::RoleConfusion {
                you_are_now: Regex::new(r"(?i)you\s+are\s+now\s+\w+")
                    .expect("you_are_now pattern"),
                you_are_now_safe: Regex::new(r"(?i)you\s+are\s+now\s+(going|about|ready)\b")
                    .expect("you_are_now_safe pattern"),
                others: RegexSet::new([
                    r"(?i)act\s+as\s+(an?\s+)?(unrestricted|unfiltered|evil)",
                    r"(?i)pretend\s+you'?re",
                    r"(?i)from\s+now\s+on\s+you\s+(are|will|must|should)",
                ])
                .expect("role_confusion other patterns"),
            },
        },
        PatternCategory {
            name: "jailbreak",
            score: 0.85,
            matcher: CategoryMatcher::Set(
                RegexSet::new([
                    r"(?i)DAN\s+(mode|prompt|jailbreak)",
                    r"(?i)developer\s+mode\s+(bypass|override|disable)",
                    r"(?i)in\s+this\s+hypothetical",
                    r"(?i)base64\s+decode\s+execute",
                ])
                .expect("jailbreak patterns"),
            ),
        },
        PatternCategory {
            name: "tool_injection",
            score: 0.8,
            matcher: CategoryMatcher::Set(
                RegexSet::new([
                    r#"(?i)"tool_calls"\s*:\s*\["#,
                    r#"(?i)"type"\s*:\s*"function""#,
                ])
                .expect("tool_injection patterns"),
            ),
        },
    ];

    PatternSet { categories }
}

static PATTERNS: OnceLock<PatternSet> = OnceLock::new();

fn patterns() -> &'static PatternSet {
    PATTERNS.get_or_init(compile_patterns)
}

impl PromptGuard {
    /// Create a new prompt guard with the given action and sensitivity threshold.
    pub fn new(action: GuardAction, sensitivity: f64) -> Self {
        // Force pattern compilation eagerly.
        let _ = patterns();
        Self { action, sensitivity }
    }

    /// Scan `input` for prompt injection patterns.
    pub fn scan(&self, input: &str) -> ScanResult {
        let pats = patterns();

        let mut detected: Vec<String> = Vec::new();
        let mut max_score: f64 = 0.0;

        for cat in &pats.categories {
            if cat.is_match(input) {
                detected.push(cat.name.to_string());
                if cat.score > max_score {
                    max_score = cat.score;
                }
            }
        }

        if detected.is_empty() {
            return ScanResult::Safe;
        }

        if self.action == GuardAction::Block && max_score > self.sensitivity {
            return ScanResult::Blocked(format!(
                "blocked: detected {} (score {:.2})",
                detected.join(", "),
                max_score
            ));
        }

        ScanResult::Suspicious(detected, max_score)
    }
}
