//! Invisible-unicode detector.
//!
//! A skill body containing zero-width spaces or directional override
//! characters can hide instructions from a human reviewer while
//! remaining visible to the LLM. The 17 characters in `INVISIBLE`
//! are the ones cited in published prompt-injection research; each
//! is treated as a `High`-severity finding.
//!
//! Allowed exception: a single BOM (`\u{FEFF}`) at the very start of
//! the file is tolerated because some editors insert it
//! unintentionally — the skill loader strips it during parse
//! anyway.

use std::path::{Path, PathBuf};

use super::patterns::Category;
use super::{Finding, GuardConfig, Location, Severity};

/// Characters considered invisible for the purpose of skill content.
/// Each entry is `(codepoint, friendly_name)`. The order isn't
/// significant.
pub const INVISIBLE: &[(char, &str)] = &[
    ('\u{200B}', "zero-width space"),
    ('\u{200C}', "zero-width non-joiner"),
    ('\u{200D}', "zero-width joiner"),
    ('\u{200E}', "left-to-right mark"),
    ('\u{200F}', "right-to-left mark"),
    ('\u{202A}', "left-to-right embedding"),
    ('\u{202B}', "right-to-left embedding"),
    ('\u{202C}', "pop directional formatting"),
    ('\u{202D}', "left-to-right override"),
    ('\u{202E}', "right-to-left override"),
    ('\u{2060}', "word joiner"),
    ('\u{2061}', "function application"),
    ('\u{2062}', "invisible times"),
    ('\u{2063}', "invisible separator"),
    ('\u{2064}', "invisible plus"),
    ('\u{FEFF}', "byte-order mark / zero-width no-break space"),
    ('\u{180E}', "Mongolian vowel separator"),
];

/// Stable rule name surfaced via `Finding.rule`. Disabling this one
/// name in `GuardConfig.disabled_rules` turns off the unicode check.
pub const RULE_NAME: &str = "unicode_invisible";

/// Scan `content` for any invisible unicode character. Each match
/// produces one finding (one per line per character — multiple
/// invisibles on the same line collapse so we don't drown the
/// findings list).
pub fn scan(content: &str, path: &Path, config: &GuardConfig) -> Vec<Finding> {
    if config.disabled_rules.iter().any(|r| r == RULE_NAME) {
        return Vec::new();
    }

    let mut findings = Vec::new();
    let mut chars_seen = std::collections::HashSet::new();
    for (line_idx, line) in content.lines().enumerate() {
        let line_num = (line_idx + 1) as u32;
        // BOM at the very start of the file is tolerated. Detect this
        // by checking if the line starts with FEFF AND we're on line 1.
        let mut start_idx = 0usize;
        let line_chars: Vec<char> = line.chars().collect();
        if line_idx == 0 && line_chars.first() == Some(&'\u{FEFF}') {
            start_idx = 1;
        }

        let mut local_seen = std::collections::HashSet::new();
        for (ci, c) in line_chars.iter().enumerate().skip(start_idx) {
            if let Some((_, name)) =
                INVISIBLE.iter().find(|(cp, _)| cp == c)
            {
                // Don't pile up duplicate findings for the same char on
                // the same line — once is enough to flag.
                if !local_seen.insert(*c) {
                    continue;
                }
                chars_seen.insert(*c);
                let snippet_start = ci.saturating_sub(8);
                let snippet_end = (ci + 8).min(line_chars.len());
                let snippet: String = line_chars[snippet_start..snippet_end]
                    .iter()
                    .collect();
                findings.push(Finding {
                    category: Category::PromptInjection,
                    severity: Severity::High,
                    rule: RULE_NAME.to_string(),
                    description: format!(
                        "invisible unicode character: {} (U+{:04X})",
                        name, *c as u32
                    ),
                    location: Location {
                        path: PathBuf::from(path),
                        line: Some(line_num),
                    },
                    snippet: super::sanitize_snippet(&snippet),
                });
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_str(s: &str) -> Vec<Finding> {
        scan(s, Path::new("test.md"), &GuardConfig::default())
    }

    #[test]
    fn plain_ascii_yields_no_findings() {
        assert!(scan_str("Hello, world.\nNothing weird.").is_empty());
    }

    #[test]
    fn zero_width_space_in_body_is_flagged() {
        let s = format!("Hello{}world.", '\u{200B}');
        let f = scan_str(&s);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, RULE_NAME);
        assert_eq!(f[0].severity, Severity::High);
    }

    #[test]
    fn rtl_override_is_flagged() {
        let s = format!("normal text{}fake", '\u{202E}');
        let f = scan_str(&s);
        assert_eq!(f.len(), 1);
        assert!(f[0].description.contains("right-to-left override"));
    }

    #[test]
    fn bom_at_start_of_line_one_is_tolerated() {
        let s = format!("{}# Heading", '\u{FEFF}');
        let f = scan_str(&s);
        assert!(f.is_empty(), "BOM at start of file must not flag");
    }

    #[test]
    fn bom_mid_file_is_flagged() {
        let s = format!("line1\n{}line2", '\u{FEFF}');
        let f = scan_str(&s);
        assert_eq!(f.len(), 1, "BOM mid-file is suspicious");
    }

    #[test]
    fn duplicate_chars_on_same_line_collapse() {
        let s = format!("a{}b{}c{}d", '\u{200B}', '\u{200B}', '\u{200B}');
        let f = scan_str(&s);
        assert_eq!(f.len(), 1, "same invisible char repeated should collapse");
    }

    #[test]
    fn different_chars_on_same_line_each_flag() {
        let s = format!("a{}b{}c", '\u{200B}', '\u{202E}');
        let f = scan_str(&s);
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn disabled_rule_skips_unicode_scan() {
        let cfg = GuardConfig {
            disabled_rules: vec![RULE_NAME.to_string()],
            ..Default::default()
        };
        let s = format!("a{}b", '\u{200B}');
        let f = scan(&s, Path::new("x.md"), &cfg);
        assert!(f.is_empty());
    }

    #[test]
    fn finding_carries_line_number() {
        let s = format!("safe\n{}\ntailing safe", '\u{200B}');
        let f = scan_str(&s);
        assert_eq!(f[0].location.line, Some(2));
    }

    #[test]
    fn category_is_prompt_injection() {
        let s = format!("a{}b", '\u{200B}');
        let f = scan_str(&s);
        assert_eq!(f[0].category, Category::PromptInjection);
    }
}
