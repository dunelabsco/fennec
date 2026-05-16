//! Static safety scanner for skill content.
//!
//! Runs three classes of check on skill markdown (and supporting
//! files) before they are accepted from an external source or, when
//! configured, before they are written by the agent itself:
//!
//!   - **regex patterns** — categorized rules that match known-
//!     dangerous shapes (credential exfil, prompt injection, reverse
//!     shells, supply-chain pulls, etc.). See [`patterns`].
//!   - **structural** — total file count, file sizes, binary
//!     extensions, symlinks pointing outside the skill, files with
//!     the executable bit set in unexpected places. See [`structural`].
//!   - **invisible unicode** — characters used in known prompt-
//!     injection jailbreaks (zero-width spaces, directional
//!     overrides, BOM markers in body text). See [`unicode`].
//!
//! Findings carry a severity (`Low`/`Medium`/`High`/`Critical`); the
//! aggregate verdict is `Safe` (no findings), `Caution` (any High),
//! or `Dangerous` (any Critical). The verdict is then mapped to an
//! install decision via the trust-policy matrix in [`policy`]: the
//! same `Caution` verdict is allowed for `Builtin` skills, blocked
//! for `Community`, and gates on the agent's `guard_agent_created`
//! config flag for `AgentCreated`.

pub mod patterns;
pub mod policy;
pub mod structural;
pub mod unicode;

use std::path::{Path, PathBuf};

pub use patterns::{Category, Rule};
pub use policy::{InstallDecision, TrustLevel, should_allow_install};
pub use structural::StructuralIssue;

/// How serious a finding is. Aggregates roll up into the verdict:
/// any `Critical` ⇒ `Dangerous`, any `High` ⇒ at least `Caution`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Cosmetic or low-risk. Reported for visibility but never
    /// downgrades the verdict below `Safe`.
    Low,
    /// Suspicious but not by itself enough to refuse a write. Worth
    /// surfacing in a review.
    Medium,
    /// One step from `Critical`. Most install policies escalate
    /// these to a user prompt.
    High,
    /// Definitely-malicious shape (credential exfil, ignore-previous
    /// jailbreak, reverse shell, `rm -rf /`). Default community
    /// policy blocks any skill containing one.
    Critical,
}

/// Top-level scan verdict, derived from the worst severity present
/// in a findings list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No findings, or only `Low`-severity findings.
    Safe,
    /// At least one `High`-severity finding.
    Caution,
    /// At least one `Critical`-severity finding.
    Dangerous,
}

impl Verdict {
    /// Roll up a list of findings into a single verdict.
    pub fn from_findings(findings: &[Finding]) -> Verdict {
        let mut worst = Verdict::Safe;
        for f in findings {
            match f.severity {
                Severity::Critical => return Verdict::Dangerous,
                Severity::High => worst = worst.max(Verdict::Caution),
                _ => {}
            }
        }
        worst
    }
}

impl PartialOrd for Verdict {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Verdict {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let rank = |v: &Verdict| match v {
            Verdict::Safe => 0,
            Verdict::Caution => 1,
            Verdict::Dangerous => 2,
        };
        rank(self).cmp(&rank(other))
    }
}

/// Where a finding came from: which file, which rule, which
/// byte-range inside that file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The file (relative or absolute) the finding came from.
    pub path: PathBuf,
    /// Line number where the match started (1-indexed). `None` for
    /// structural findings that aren't tied to a single line.
    pub line: Option<u32>,
}

/// One thing the scanner found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub category: Category,
    pub severity: Severity,
    /// Short stable name of the rule. Used to disable individual
    /// rules in config (`disabled_rules: ["exfil_curl_env"]`).
    pub rule: String,
    /// Human-readable description for surfacing in the
    /// `should_allow_install` rejection reason.
    pub description: String,
    /// Where the rule fired.
    pub location: Location,
    /// First ~120 chars of the matched span, for log lines and
    /// rejection messages. Truncated to avoid blowing up logs on
    /// huge matches.
    pub snippet: String,
}

impl Finding {
    /// Construct a finding for a regex hit. The snippet is truncated
    /// to 120 chars and any `\n` in it is replaced with a literal
    /// `\\n` for single-line log output.
    pub fn from_match(rule: &Rule, path: PathBuf, line: u32, matched: &str) -> Self {
        let snippet = sanitize_snippet(matched);
        Finding {
            category: rule.category,
            severity: rule.severity,
            rule: rule.name.to_string(),
            description: rule.description.to_string(),
            location: Location {
                path,
                line: Some(line),
            },
            snippet,
        }
    }
}

fn sanitize_snippet(s: &str) -> String {
    let trimmed: String = s.chars().take(120).collect();
    trimmed
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Tunable knobs for the scanner. Wired into `skill_manage` (when
/// `guard_agent_created` is true the scanner runs before every
/// agent-created skill write) and into the future skills-hub
/// installer (always runs, regardless of trust level).
#[derive(Debug, Clone, Default)]
pub struct GuardConfig {
    /// When true, agent-created skills are scanned before write.
    /// Default false — the agent already has a terminal/code-exec
    /// tool, so scanning agent writes is partial mitigation at best;
    /// the operator opts in.
    pub guard_agent_created: bool,
    /// Categories to skip entirely. Useful when a category produces
    /// false positives in a specific deployment (e.g. `Persistence`
    /// for skills that legitimately edit `.bashrc`).
    pub disabled_categories: Vec<Category>,
    /// Individual rule names to skip. Finer-grained than category-
    /// level disable. The rule name is the `Rule.name` field.
    pub disabled_rules: Vec<String>,
}

impl GuardConfig {
    fn is_rule_active(&self, rule: &Rule) -> bool {
        if self
            .disabled_categories
            .iter()
            .any(|c| *c == rule.category)
        {
            return false;
        }
        !self
            .disabled_rules
            .iter()
            .any(|r| r == rule.name)
    }

    /// Build a runtime `GuardConfig` from the TOML-shaped config.
    /// Category strings that don't match any known category are
    /// logged at warn level and ignored.
    pub fn from_toml(toml: &crate::config::SkillsGuardConfigToml) -> Self {
        let mut disabled_categories = Vec::new();
        for raw in &toml.disabled_categories {
            let parsed = match raw.as_str() {
                "exfiltration" => Category::Exfiltration,
                "prompt_injection" => Category::PromptInjection,
                "destructive" => Category::Destructive,
                "persistence" => Category::Persistence,
                "network" => Category::Network,
                "obfuscation" => Category::Obfuscation,
                "process_exec" => Category::ProcessExec,
                "path_traversal" => Category::PathTraversal,
                "crypto_mining" => Category::CryptoMining,
                "supply_chain" => Category::SupplyChain,
                "privilege_escalation" => Category::PrivilegeEscalation,
                "credential_exposure" => Category::CredentialExposure,
                other => {
                    tracing::warn!(
                        category = other,
                        "skills.guard.disabled_categories: unknown category, ignoring"
                    );
                    continue;
                }
            };
            disabled_categories.push(parsed);
        }
        Self {
            guard_agent_created: toml.guard_agent_created,
            disabled_categories,
            disabled_rules: toml.disabled_rules.clone(),
        }
    }
}

/// Scan a single in-memory string. `path` is used only for the
/// `Location` field — the content is whatever `content` is.
pub fn scan_content(content: &str, path: &Path, config: &GuardConfig) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(patterns::scan(content, path, config));
    findings.extend(unicode::scan(content, path, config));
    findings
}

/// Scan a skill directory recursively: every file under it goes
/// through the regex + unicode passes, plus a structural check on
/// the directory as a whole.
pub fn scan_skill_dir(skill_dir: &Path, config: &GuardConfig) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(structural::scan_dir(skill_dir, config));
    walk_files(skill_dir, &mut |file_path| {
        if let Ok(bytes) = std::fs::read(file_path) {
            // Pure-text scan only: skip files that look binary
            // (NUL byte in the first 4KB) since regex patterns are
            // designed for source / markdown / config content.
            if looks_binary(&bytes) {
                return;
            }
            let content = String::from_utf8_lossy(&bytes);
            findings.extend(scan_content(&content, file_path, config));
        }
    });
    findings
}

/// Quick "is this binary" heuristic: any NUL byte in the first 4 KB
/// flips the file out of the text-scan path. Avoids feeding the
/// regex engine random PNG bytes.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(4096).any(|b| *b == 0)
}

fn walk_files(dir: &Path, on_file: &mut dyn FnMut(&Path)) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk_files(&p, on_file);
        } else if ft.is_file() {
            on_file(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_findings_yields_safe_verdict() {
        assert_eq!(Verdict::from_findings(&[]), Verdict::Safe);
    }

    #[test]
    fn low_severity_only_is_safe() {
        let f = Finding {
            category: Category::Exfiltration,
            severity: Severity::Low,
            rule: "x".into(),
            description: String::new(),
            location: Location {
                path: PathBuf::from("/tmp/x"),
                line: Some(1),
            },
            snippet: String::new(),
        };
        assert_eq!(Verdict::from_findings(&[f]), Verdict::Safe);
    }

    #[test]
    fn high_severity_yields_caution() {
        let f = Finding {
            category: Category::Exfiltration,
            severity: Severity::High,
            rule: "x".into(),
            description: String::new(),
            location: Location {
                path: PathBuf::from("/tmp/x"),
                line: Some(1),
            },
            snippet: String::new(),
        };
        assert_eq!(Verdict::from_findings(&[f]), Verdict::Caution);
    }

    #[test]
    fn critical_severity_yields_dangerous() {
        let f = Finding {
            category: Category::Exfiltration,
            severity: Severity::Critical,
            rule: "x".into(),
            description: String::new(),
            location: Location {
                path: PathBuf::from("/tmp/x"),
                line: Some(1),
            },
            snippet: String::new(),
        };
        assert_eq!(Verdict::from_findings(&[f]), Verdict::Dangerous);
    }

    #[test]
    fn worst_severity_dominates() {
        let high = Finding {
            category: Category::Exfiltration,
            severity: Severity::High,
            rule: "h".into(),
            description: String::new(),
            location: Location {
                path: PathBuf::from("/tmp/x"),
                line: Some(1),
            },
            snippet: String::new(),
        };
        let critical = Finding {
            category: Category::Exfiltration,
            severity: Severity::Critical,
            rule: "c".into(),
            description: String::new(),
            location: Location {
                path: PathBuf::from("/tmp/x"),
                line: Some(2),
            },
            snippet: String::new(),
        };
        assert_eq!(
            Verdict::from_findings(&[high.clone(), critical.clone()]),
            Verdict::Dangerous
        );
        assert_eq!(
            Verdict::from_findings(&[critical, high]),
            Verdict::Dangerous
        );
    }

    #[test]
    fn snippet_is_sanitized_and_truncated() {
        let long = "a".repeat(200);
        let s = sanitize_snippet(&format!("{}\nB\rC\tD", long));
        assert!(s.len() <= 120);
        // newline replacement only matters if it survives the truncation
        // window — confirm the function doesn't panic on a multi-line
        // input.
        let s2 = sanitize_snippet("x\ny\rz\tw");
        assert_eq!(s2, "x\\ny\\rz\\tw");
    }

    #[test]
    fn looks_binary_detects_nul_byte() {
        assert!(looks_binary(b"abc\0def"));
        assert!(!looks_binary(b"plain text content"));
    }

    #[test]
    fn from_toml_maps_category_strings() {
        let toml = crate::config::SkillsGuardConfigToml {
            guard_agent_created: true,
            disabled_categories: vec![
                "exfiltration".into(),
                "prompt_injection".into(),
                "credential_exposure".into(),
                "not_a_real_category".into(), // logged + skipped
            ],
            disabled_rules: vec!["pi_dan_mode".into()],
        };
        let cfg = GuardConfig::from_toml(&toml);
        assert!(cfg.guard_agent_created);
        assert_eq!(cfg.disabled_categories.len(), 3);
        assert!(
            cfg.disabled_categories
                .contains(&Category::Exfiltration)
        );
        assert!(
            cfg.disabled_categories
                .contains(&Category::PromptInjection)
        );
        assert!(
            cfg.disabled_categories
                .contains(&Category::CredentialExposure)
        );
        assert_eq!(cfg.disabled_rules, vec!["pi_dan_mode"]);
    }

    #[test]
    fn from_toml_default_is_off() {
        let cfg = GuardConfig::from_toml(&Default::default());
        assert!(!cfg.guard_agent_created);
        assert!(cfg.disabled_categories.is_empty());
        assert!(cfg.disabled_rules.is_empty());
    }

    #[test]
    fn config_disables_categories_and_rules() {
        let r = Rule {
            name: "x",
            category: Category::Exfiltration,
            severity: Severity::High,
            pattern: ".*",
            description: "x",
        };
        let c1 = GuardConfig {
            disabled_categories: vec![Category::Exfiltration],
            ..Default::default()
        };
        assert!(!c1.is_rule_active(&r));
        let c2 = GuardConfig {
            disabled_rules: vec!["x".into()],
            ..Default::default()
        };
        assert!(!c2.is_rule_active(&r));
        let c3 = GuardConfig::default();
        assert!(c3.is_rule_active(&r));
    }
}
