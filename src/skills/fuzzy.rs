//! Whitespace-tolerant find-and-replace for the `skill_manage patch`
//! action.
//!
//! LLM-generated diffs almost never get indentation right on the first
//! try. A patch like
//!
//! ```text
//! old_string: "    return value\n"
//! new_string: "    return value + 1\n"
//! ```
//!
//! often arrives as `"  return value\n"` or `"return value\n"` — the
//! LLM saw the line, copied its content, but lost the surrounding
//! indentation that lives in the original file. A naive exact match
//! against the file fails and the agent has to guess again.
//!
//! This module gives the patch action three matching strategies in
//! escalating tolerance:
//!
//! 1. **exact**: byte-for-byte substring match.
//! 2. **trim-trailing**: each line of the search and target has its
//!    trailing whitespace stripped before comparison. Catches the
//!    common case where the LLM dropped (or added) a trailing space.
//! 3. **trim-leading**: each line has both leading *and* trailing
//!    whitespace stripped. Catches indent drift. The replacement is
//!    re-indented from the matched location's leading whitespace so
//!    the output is well-formed.
//!
//! Strategies are tried in order; the first one that finds a unique
//! match wins. Ambiguity (the same search appears twice) is reported
//! up to the caller so they can decide whether to set `replace_all`.

use std::fmt;

/// Outcome of a `find` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindResult {
    /// No match at any tolerance level.
    NotFound,
    /// Exactly one match. `start..end` is the byte range in the
    /// original input that should be replaced.
    Unique {
        start: usize,
        end: usize,
        strategy: MatchStrategy,
    },
    /// More than one match. The caller can decide whether to retry
    /// with `replace_all=true` or surface an error.
    Ambiguous {
        count: usize,
        strategy: MatchStrategy,
    },
}

/// Which tolerance level produced a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchStrategy {
    /// Byte-for-byte substring.
    Exact,
    /// Trailing whitespace stripped from each line.
    TrimTrailing,
    /// Both leading and trailing whitespace stripped from each line.
    TrimLeading,
}

impl fmt::Display for MatchStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MatchStrategy::Exact => write!(f, "exact"),
            MatchStrategy::TrimTrailing => write!(f, "trim-trailing"),
            MatchStrategy::TrimLeading => write!(f, "trim-leading"),
        }
    }
}

/// Find the unique location of `needle` inside `haystack`, in
/// escalating tolerance. Returns `Unique` only when one of the
/// strategies produces exactly one hit.
///
/// All three strategies are line-aligned: the first line of `needle`
/// must coincide with the start of a line in `haystack`. This rules
/// out partial in-line matches (e.g., matching `return 42;` inside
/// `wreturn 42;`) which would corrupt the splice.
pub fn find(haystack: &str, needle: &str) -> FindResult {
    if needle.is_empty() {
        return FindResult::NotFound;
    }

    for strategy in [
        MatchStrategy::Exact,
        MatchStrategy::TrimTrailing,
        MatchStrategy::TrimLeading,
    ] {
        let hits = match_line_aligned(haystack, needle, strategy);
        match hits.len() {
            0 => continue,
            1 => {
                let (s, e) = hits[0];
                return FindResult::Unique {
                    start: s,
                    end: e,
                    strategy,
                };
            }
            n => {
                return FindResult::Ambiguous { count: n, strategy };
            }
        }
    }
    FindResult::NotFound
}

/// Find every byte-range in `haystack` that matches `needle` under
/// any tolerance. Used by `replace_all=true`.
///
/// Strategies are tried in order; the first one that produces any
/// hits is the one whose ranges are returned. This avoids mixing
/// indentation-sensitive matches with exact ones in the same pass,
/// which would produce overlapping replacements.
pub fn find_all(haystack: &str, needle: &str) -> (Vec<(usize, usize)>, MatchStrategy) {
    if needle.is_empty() {
        return (Vec::new(), MatchStrategy::Exact);
    }
    for strategy in [
        MatchStrategy::Exact,
        MatchStrategy::TrimTrailing,
        MatchStrategy::TrimLeading,
    ] {
        let hits = match_line_aligned(haystack, needle, strategy);
        if !hits.is_empty() {
            return (hits, strategy);
        }
    }
    (Vec::new(), MatchStrategy::Exact)
}

/// Replace one or more occurrences of `needle` with `replacement`,
/// using the same escalating tolerance.
///
/// Returns `(updated_haystack, strategy, count)`. When `replace_all`
/// is false and the match is ambiguous, returns `Err` describing the
/// ambiguity. When `replace_all` is false and the match is unique,
/// only the unique location is replaced; if multiple matches exist
/// they are NOT replaced — the caller must opt in via `replace_all`.
pub fn replace(
    haystack: &str,
    needle: &str,
    replacement: &str,
    replace_all: bool,
) -> Result<(String, MatchStrategy, usize), ReplaceError> {
    if needle.is_empty() {
        return Err(ReplaceError::EmptyNeedle);
    }

    if replace_all {
        let (ranges, strategy) = find_all(haystack, needle);
        if ranges.is_empty() {
            return Err(ReplaceError::NotFound);
        }
        let mut sorted = ranges.clone();
        sorted.sort_by_key(|r| r.0);
        for w in sorted.windows(2) {
            if w[0].1 > w[1].0 {
                return Err(ReplaceError::OverlappingMatches);
            }
        }
        let mut out = String::with_capacity(haystack.len() + replacement.len());
        let mut cursor = 0;
        for (start, end) in &sorted {
            out.push_str(&haystack[cursor..*start]);
            out.push_str(&render_replacement(haystack, *start, replacement, strategy));
            cursor = *end;
        }
        out.push_str(&haystack[cursor..]);
        return Ok((out, strategy, sorted.len()));
    }

    match find(haystack, needle) {
        FindResult::NotFound => Err(ReplaceError::NotFound),
        FindResult::Ambiguous { count, .. } => Err(ReplaceError::Ambiguous(count)),
        FindResult::Unique { start, end, strategy } => {
            let mut out = String::with_capacity(haystack.len() + replacement.len());
            out.push_str(&haystack[..start]);
            out.push_str(&render_replacement(haystack, start, replacement, strategy));
            out.push_str(&haystack[end..]);
            Ok((out, strategy, 1))
        }
    }
}

/// Reasons `replace` can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceError {
    /// Empty `needle` is never valid.
    EmptyNeedle,
    /// No match under any tolerance level.
    NotFound,
    /// More than one match — caller must set `replace_all=true` if
    /// they want to replace every instance.
    Ambiguous(usize),
    /// Defense-in-depth: should never happen because each match
    /// covers a contiguous distinct byte range.
    OverlappingMatches,
}

impl fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplaceError::EmptyNeedle => write!(f, "search string is empty"),
            ReplaceError::NotFound => write!(f, "search string not found in target"),
            ReplaceError::Ambiguous(n) => write!(
                f,
                "search string matches in {} locations; set replace_all=true to replace every instance",
                n
            ),
            ReplaceError::OverlappingMatches => {
                write!(f, "internal: matched ranges overlap; refusing to replace")
            }
        }
    }
}

impl std::error::Error for ReplaceError {}

/// Render the replacement at the matched location.
///
/// For `Exact` and `TrimTrailing` matches, the replacement is
/// inserted verbatim — the search succeeded with the supplied
/// indentation, so the caller is presumed to want their indentation
/// preserved.
///
/// For `TrimLeading` matches the search succeeded only because we
/// stripped leading whitespace, which means the haystack and needle
/// disagreed on indent. To keep the output well-formed we prefix
/// each non-blank replacement line with the haystack-line indent at
/// `start`. Lines that are already entirely blank are left blank
/// (so we don't emit trailing-whitespace-only lines).
fn render_replacement(
    haystack: &str,
    start: usize,
    replacement: &str,
    strategy: MatchStrategy,
) -> String {
    if !matches!(strategy, MatchStrategy::TrimLeading) {
        return replacement.to_string();
    }

    // The matched range begins at the line start; the indent we want
    // lives between `start` and the first non-indent char on that
    // line.
    let target_indent: String = haystack[start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();

    // Re-indent the replacement: strip each line's existing leading
    // whitespace and prefix with `target_indent`. Empty lines stay
    // empty (no indent on a blank line).
    let mut out = String::with_capacity(replacement.len() + target_indent.len() * 4);
    let mut first = true;
    for line in replacement.split_inclusive('\n') {
        let (body, newline) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        let trimmed = body.trim_start_matches([' ', '\t']);
        if !first {
            // Already pushed a newline at the end of the previous line.
        }
        if trimmed.is_empty() {
            // Preserve blank lines without injecting indent.
            out.push_str(newline);
        } else {
            out.push_str(&target_indent);
            out.push_str(trimmed);
            out.push_str(newline);
        }
        first = false;
    }
    out
}

/// Find every line-aligned region in `haystack` whose lines match
/// `needle`'s lines under the given strategy.
///
/// The match is line-aligned: needle's first line starts at a line
/// boundary in haystack and its last line ends at a line boundary.
/// This rules out partial in-line matches the patch tool can't
/// safely splice.
fn match_line_aligned(
    haystack: &str,
    needle: &str,
    strategy: MatchStrategy,
) -> Vec<(usize, usize)> {
    let strip_trailing = !matches!(strategy, MatchStrategy::Exact);
    let strip_leading = matches!(strategy, MatchStrategy::TrimLeading);
    let normalize = |line: &str| -> String {
        let mut s = line;
        // Always drop a trailing \r (CRLF support) for non-Exact;
        // Exact preserves bytes verbatim.
        if strip_trailing {
            s = s.trim_end_matches(['\r', ' ', '\t']);
        }
        if strip_leading {
            s = s.trim_start_matches([' ', '\t']);
        }
        s.to_string()
    };

    // Tokenize haystack into (normalized_line, byte_start, byte_end_inclusive_of_newline).
    let mut lines: Vec<(String, usize, usize)> = Vec::new();
    {
        let mut start = 0usize;
        for (i, b) in haystack.bytes().enumerate() {
            if b == b'\n' {
                let line = &haystack[start..i];
                lines.push((normalize(line), start, i + 1));
                start = i + 1;
            }
        }
        if start < haystack.len() {
            lines.push((normalize(&haystack[start..]), start, haystack.len()));
        }
    }

    // Tokenize needle the same way. We don't keep byte offsets for
    // needle — only its normalized lines drive comparison.
    let needle_lines: Vec<String> = {
        let mut v = Vec::new();
        let mut start = 0usize;
        for (i, b) in needle.bytes().enumerate() {
            if b == b'\n' {
                v.push(normalize(&needle[start..i]));
                start = i + 1;
            }
        }
        if start < needle.len() {
            v.push(normalize(&needle[start..]));
        }
        v
    };

    if needle_lines.is_empty() || needle_lines.len() > lines.len() {
        return Vec::new();
    }

    let mut hits = Vec::new();
    for i in 0..=(lines.len() - needle_lines.len()) {
        let window = &lines[i..i + needle_lines.len()];
        let all_match = window
            .iter()
            .zip(needle_lines.iter())
            .all(|((hl, _, _), nl)| hl == nl);
        if all_match {
            let start = window.first().unwrap().1;
            let end = window.last().unwrap().2;
            hits.push((start, end));
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_unique_match() {
        let h = "alpha\nbeta\ngamma\n";
        match find(h, "beta\n") {
            FindResult::Unique {
                start,
                end,
                strategy,
            } => {
                assert_eq!(strategy, MatchStrategy::Exact);
                assert_eq!(&h[start..end], "beta\n");
            }
            other => panic!("expected unique match, got {:?}", other),
        }
    }

    #[test]
    fn empty_needle_is_not_found() {
        assert_eq!(find("alpha\n", ""), FindResult::NotFound);
    }

    #[test]
    fn exact_ambiguous_match() {
        let h = "alpha\nalpha\n";
        match find(h, "alpha\n") {
            FindResult::Ambiguous { count, strategy } => {
                assert_eq!(count, 2);
                assert_eq!(strategy, MatchStrategy::Exact);
            }
            other => panic!("expected ambiguous, got {:?}", other),
        }
    }

    #[test]
    fn trim_trailing_finds_when_exact_misses() {
        let h = "alpha\nbeta   \ngamma\n";
        // Search for "beta\n" — exact won't match because the line has
        // trailing spaces. trim-trailing should find it.
        match find(h, "beta\n") {
            FindResult::Unique { strategy, .. } => {
                assert_eq!(strategy, MatchStrategy::TrimTrailing);
            }
            other => panic!("expected trim-trailing match, got {:?}", other),
        }
    }

    #[test]
    fn trim_leading_finds_when_indent_differs() {
        // Haystack has 4-space indent; needle uses 2-space. Exact
        // can't match (different byte sequence at line start),
        // trim-trailing can't either, trim-leading does.
        let h = "fn foo() {\n    return 42;\n}\n";
        match find(h, "  return 42;\n") {
            FindResult::Unique { start, end, strategy } => {
                assert_eq!(strategy, MatchStrategy::TrimLeading);
                assert!(h[start..end].contains("return 42"));
            }
            other => panic!("expected trim-leading match, got {:?}", other),
        }
    }

    /// Exact strategy is line-aligned: a needle that doesn't share the
    /// haystack line's leading whitespace doesn't match `Exact`.
    /// Falling through to `TrimLeading` is the only path.
    #[test]
    fn exact_does_not_match_inside_a_line() {
        let h = "fn foo() {\n    return 42;\n}\n";
        match find(h, "return 42;\n") {
            FindResult::Unique { strategy, .. } => {
                // Falls to TrimLeading — exact is line-aligned and
                // won't match a needle without the haystack indent.
                assert_eq!(strategy, MatchStrategy::TrimLeading);
            }
            other => panic!("expected fuzzy match, got {:?}", other),
        }
    }

    #[test]
    fn replace_unique_exact() {
        let (out, strategy, count) =
            replace("alpha\nbeta\ngamma\n", "beta\n", "BETA\n", false).unwrap();
        assert_eq!(out, "alpha\nBETA\ngamma\n");
        assert_eq!(strategy, MatchStrategy::Exact);
        assert_eq!(count, 1);
    }

    #[test]
    fn replace_ambiguous_without_replace_all_errors() {
        let err = replace("a\na\n", "a\n", "X\n", false).unwrap_err();
        assert_eq!(err, ReplaceError::Ambiguous(2));
    }

    #[test]
    fn replace_all_handles_multiple() {
        let (out, _, count) = replace("a\nb\na\nc\n", "a\n", "X\n", true).unwrap();
        assert_eq!(out, "X\nb\nX\nc\n");
        assert_eq!(count, 2);
    }

    #[test]
    fn replace_not_found_errors() {
        let err = replace("alpha\n", "beta\n", "X\n", false).unwrap_err();
        assert_eq!(err, ReplaceError::NotFound);
        let err_all = replace("alpha\n", "beta\n", "X\n", true).unwrap_err();
        assert_eq!(err_all, ReplaceError::NotFound);
    }

    #[test]
    fn replace_empty_needle_errors() {
        assert_eq!(
            replace("a", "", "x", false).unwrap_err(),
            ReplaceError::EmptyNeedle
        );
    }

    #[test]
    fn replace_with_trim_trailing_replaces_full_line() {
        // Original has trailing spaces; needle doesn't.
        let h = "a\nbeta   \nc\n";
        let (out, strategy, _) = replace(h, "beta\n", "BETA\n", false).unwrap();
        // The replacement consumes the whole line including trailing
        // whitespace and replaces with the new content.
        assert!(out.contains("BETA"));
        assert!(!out.contains("beta   "));
        assert_eq!(strategy, MatchStrategy::TrimTrailing);
    }

    #[test]
    fn replace_with_trim_leading_when_indent_differs() {
        let h = "fn foo() {\n    return 42;\n}\n";
        // Needle uses 2-space indent; haystack uses 4-space. Exact
        // misses, trim-leading hits, and the replacement is re-
        // indented to the haystack's 4-space indent.
        let (out, strategy, _) =
            replace(h, "  return 42;\n", "  return 43;\n", false).unwrap();
        assert_eq!(out, "fn foo() {\n    return 43;\n}\n");
        assert_eq!(strategy, MatchStrategy::TrimLeading);
    }

    #[test]
    fn replace_with_trim_leading_reindents_multiline_replacement() {
        let h = "fn foo() {\n    return 42;\n}\n";
        // Multi-line replacement, the LLM sent without indent. We
        // re-indent each non-blank line to the haystack's 4-space.
        let needle = "  return 42;\n";
        let repl = "let x = 1;\nreturn x + 41;\n";
        let (out, _, _) = replace(h, needle, repl, false).unwrap();
        assert_eq!(out, "fn foo() {\n    let x = 1;\n    return x + 41;\n}\n");
    }

    #[test]
    fn replace_with_trim_leading_preserves_blank_lines() {
        let h = "    foo\n";
        let (out, _, _) = replace(h, "foo\n", "bar\n\nbaz\n", false).unwrap();
        // Blank line stays blank (no injected indent).
        assert_eq!(out, "    bar\n\n    baz\n");
    }

    #[test]
    fn multi_line_block_match() {
        let h = "fn foo() {\n    let x = 1;\n    let y = 2;\n    return x + y;\n}\n";
        let needle = "    let x = 1;\n    let y = 2;\n";
        let replacement = "    let x = 100;\n    let y = 200;\n";
        let (out, strategy, _) = replace(h, needle, replacement, false).unwrap();
        assert!(out.contains("let x = 100"));
        assert!(out.contains("let y = 200"));
        assert_eq!(strategy, MatchStrategy::Exact);
    }

    #[test]
    fn last_line_no_trailing_newline_still_matches() {
        let h = "alpha\nbeta";
        match find(h, "beta") {
            FindResult::Unique { strategy, .. } => assert_eq!(strategy, MatchStrategy::Exact),
            other => panic!("expected match, got {:?}", other),
        }
    }
}
