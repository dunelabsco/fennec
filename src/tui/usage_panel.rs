//! `/usage` panel rendering.
//!
//! Translates a [`crate::agent::TokenUsage`] snapshot into the
//! multi-line system message that lands in the chat scrollback
//! when the user types `/usage`. The row layout mirrors
//! upstream's Usage panel: Model / Input / Cache read /
//! Cache write / Output / Total / API calls / Cost / Context.
//!
//! Lives in its own module (rather than inline in `main.rs`)
//! so the formatting is unit-testable as part of the lib crate.

use crate::agent::TokenUsage;

/// Render a `/usage` panel from a token-usage snapshot.
///
/// Rows whose value isn't available yet (no API calls have
/// happened, model isn't in the pricing snapshot, no cache
/// activity, etc.) are omitted rather than displayed as zeros,
/// so the user can tell "no data" from "data is zero."
pub fn render(u: &TokenUsage) -> String {
    if u.api_calls == 0 {
        return "no API calls yet — usage will populate after the first turn".to_string();
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    let model_label = if u.model.is_empty() {
        "(unknown)".to_string()
    } else {
        u.model.clone()
    };
    rows.push(("Model".into(), model_label));
    rows.push(("Input tokens".into(), format_count(u.input_tokens)));
    if u.cache_read_tokens > 0 {
        rows.push((
            "Cache read tokens".into(),
            format_count(u.cache_read_tokens),
        ));
    }
    if u.cache_write_tokens > 0 {
        rows.push((
            "Cache write tokens".into(),
            format_count(u.cache_write_tokens),
        ));
    }
    rows.push(("Output tokens".into(), format_count(u.output_tokens)));
    rows.push(("Total tokens".into(), format_count(u.total_tokens())));
    rows.push(("API calls".into(), format_count(u.api_calls)));
    if let Some(cost) = u.cost_usd {
        rows.push((
            "Cost (estimated)".into(),
            format!("${cost:.4}"),
        ));
    }
    if let Some(pct) = u.context_percent() {
        rows.push((
            "Context".into(),
            format!(
                "{}/{} tokens ({}%)",
                format_count(u.last_prompt_tokens),
                format_count(u.context_max as u64),
                pct,
            ),
        ));
    }
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::from("Usage\n");
    for (label, value) in &rows {
        out.push_str(&format!(
            "  {:<width$}  {}\n",
            label,
            value,
            width = label_width,
        ));
    }
    out.trim_end().to_string()
}

/// Format a u64 with thousands separators ("1,234,567"). The
/// /usage panel reads more naturally that way for big numbers
/// like 4_500_000 cache_read tokens.
pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        let from_end = bytes.len() - i;
        if i > 0 && from_end % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_count_inserts_thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(1_234_567), "1,234,567");
        assert_eq!(format_count(1_000_000_000), "1,000,000,000");
    }

    fn sample_usage() -> TokenUsage {
        TokenUsage {
            model: "claude-sonnet-4-6".into(),
            input_tokens: 1_200,
            output_tokens: 300,
            cache_read_tokens: 500,
            cache_write_tokens: 50,
            api_calls: 3,
            last_prompt_tokens: 1_750,
            context_max: 200_000,
            cost_usd: Some(0.0123),
        }
    }

    #[test]
    fn render_panel_when_no_calls_says_so() {
        let u = TokenUsage::default();
        let body = render(&u);
        assert!(
            body.contains("no API calls yet"),
            "expected the empty-state message, got: {body}"
        );
    }

    #[test]
    fn render_panel_includes_all_expected_rows() {
        let u = sample_usage();
        let body = render(&u);
        assert!(body.starts_with("Usage\n"), "got: {body}");
        assert!(body.contains("Model"));
        assert!(body.contains("claude-sonnet-4-6"));
        assert!(body.contains("Input tokens"));
        assert!(body.contains("1,200"));
        assert!(body.contains("Cache read tokens"));
        assert!(body.contains("500"));
        assert!(body.contains("Cache write tokens"));
        assert!(body.contains("Output tokens"));
        assert!(body.contains("300"));
        assert!(body.contains("Total tokens"));
        assert!(body.contains("API calls"));
        assert!(body.contains("Cost (estimated)"));
        assert!(body.contains("$0.0123"));
        assert!(body.contains("Context"));
        assert!(body.contains("200,000"));
    }

    #[test]
    fn render_panel_omits_zero_cache_rows() {
        let mut u = sample_usage();
        u.cache_read_tokens = 0;
        u.cache_write_tokens = 0;
        let body = render(&u);
        assert!(!body.contains("Cache read"), "got: {body}");
        assert!(!body.contains("Cache write"), "got: {body}");
        assert!(body.contains("Input tokens"));
    }

    #[test]
    fn render_panel_omits_cost_row_when_pricing_unknown() {
        let mut u = sample_usage();
        u.cost_usd = None;
        let body = render(&u);
        assert!(!body.contains("Cost"), "expected no cost row, got: {body}");
    }

    #[test]
    fn render_panel_handles_unknown_model() {
        let mut u = sample_usage();
        u.model = String::new();
        let body = render(&u);
        assert!(body.contains("(unknown)"), "got: {body}");
    }
}
