//! Static USD pricing snapshot for `/usage` cost estimation.
//!
//! This is the **estimated** layer (in upstream taxonomy:
//! `cost_status: "estimated"`). It compiles a fixed pricing
//! table for the providers Fennec ships with, looks up a model
//! by name (with prefix-matching for dated snapshots), and
//! computes a USD cost from a [`UsageInfo`].
//!
//! Live API fetching against provider cost endpoints, snapshot
//! refresh from a docs URL, and a user-override file
//! (`~/.fennec/pricing.toml`) are deliberately separate from
//! this module — they can land later without changing this
//! interface. Callers who need "actual" cost are free to skip
//! [`estimate_cost`] and consult the provider directly.
//!
//! All prices are USD per million tokens, as published on the
//! providers' public pricing pages at the time of snapshot.

#[cfg(test)]
use crate::providers::traits::UsageInfo;

/// Pricing for one model. `None` for a column means the
/// provider doesn't expose that token type (e.g. no cache
/// reads).
#[derive(Debug, Clone, Copy)]
pub struct PricingEntry {
    pub model_prefix: &'static str,
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: Option<f64>,
    pub cache_write_per_million: Option<f64>,
}

/// Snapshot table. Order matters — longer prefixes first so
/// e.g. `claude-haiku-4-5-20251001` matches the haiku entry
/// before falling through to a generic `claude-` prefix.
const PRICING_TABLE: &[PricingEntry] = &[
    // -- Anthropic Claude -----------------------------------------
    PricingEntry {
        model_prefix: "claude-opus-4-7",
        input_per_million: 15.0,
        output_per_million: 75.0,
        cache_read_per_million: Some(1.50),
        cache_write_per_million: Some(18.75),
    },
    PricingEntry {
        model_prefix: "claude-opus-4-6",
        input_per_million: 15.0,
        output_per_million: 75.0,
        cache_read_per_million: Some(1.50),
        cache_write_per_million: Some(18.75),
    },
    PricingEntry {
        model_prefix: "claude-opus-4",
        input_per_million: 15.0,
        output_per_million: 75.0,
        cache_read_per_million: Some(1.50),
        cache_write_per_million: Some(18.75),
    },
    PricingEntry {
        model_prefix: "claude-sonnet-4-6",
        input_per_million: 3.0,
        output_per_million: 15.0,
        cache_read_per_million: Some(0.30),
        cache_write_per_million: Some(3.75),
    },
    PricingEntry {
        model_prefix: "claude-sonnet-4",
        input_per_million: 3.0,
        output_per_million: 15.0,
        cache_read_per_million: Some(0.30),
        cache_write_per_million: Some(3.75),
    },
    PricingEntry {
        model_prefix: "claude-haiku-4-5",
        input_per_million: 0.80,
        output_per_million: 4.0,
        cache_read_per_million: Some(0.08),
        cache_write_per_million: Some(1.0),
    },
    PricingEntry {
        model_prefix: "claude-3-5-sonnet",
        input_per_million: 3.0,
        output_per_million: 15.0,
        cache_read_per_million: Some(0.30),
        cache_write_per_million: Some(3.75),
    },
    PricingEntry {
        model_prefix: "claude-3-5-haiku",
        input_per_million: 0.80,
        output_per_million: 4.0,
        cache_read_per_million: Some(0.08),
        cache_write_per_million: Some(1.0),
    },
    PricingEntry {
        model_prefix: "claude-3-opus",
        input_per_million: 15.0,
        output_per_million: 75.0,
        cache_read_per_million: Some(1.50),
        cache_write_per_million: Some(18.75),
    },
    // -- OpenAI ---------------------------------------------------
    PricingEntry {
        model_prefix: "gpt-5-mini",
        input_per_million: 0.25,
        output_per_million: 2.0,
        cache_read_per_million: Some(0.025),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "gpt-5",
        input_per_million: 1.25,
        output_per_million: 10.0,
        cache_read_per_million: Some(0.125),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "gpt-4o-mini",
        input_per_million: 0.15,
        output_per_million: 0.60,
        cache_read_per_million: Some(0.075),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "gpt-4o",
        input_per_million: 2.50,
        output_per_million: 10.0,
        cache_read_per_million: Some(1.25),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "gpt-4.1-mini",
        input_per_million: 0.40,
        output_per_million: 1.60,
        cache_read_per_million: Some(0.10),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "gpt-4.1",
        input_per_million: 2.0,
        output_per_million: 8.0,
        cache_read_per_million: Some(0.50),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "o4-mini",
        input_per_million: 1.10,
        output_per_million: 4.40,
        cache_read_per_million: Some(0.275),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "o3-mini",
        input_per_million: 1.10,
        output_per_million: 4.40,
        cache_read_per_million: Some(0.55),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "o3",
        input_per_million: 2.0,
        output_per_million: 8.0,
        cache_read_per_million: Some(0.50),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "o1-mini",
        input_per_million: 1.10,
        output_per_million: 4.40,
        cache_read_per_million: Some(0.55),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "o1",
        input_per_million: 15.0,
        output_per_million: 60.0,
        cache_read_per_million: Some(7.50),
        cache_write_per_million: None,
    },
    // -- Moonshot / Kimi ------------------------------------------
    PricingEntry {
        model_prefix: "kimi-k2.5",
        input_per_million: 0.60,
        output_per_million: 2.50,
        cache_read_per_million: Some(0.15),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "kimi-k2",
        input_per_million: 0.60,
        output_per_million: 2.50,
        cache_read_per_million: Some(0.15),
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "moonshot-v1-128k",
        input_per_million: 1.68,
        output_per_million: 1.68,
        cache_read_per_million: None,
        cache_write_per_million: None,
    },
    PricingEntry {
        model_prefix: "moonshot-v1",
        input_per_million: 0.84,
        output_per_million: 0.84,
        cache_read_per_million: None,
        cache_write_per_million: None,
    },
];

/// Look up a pricing entry for the given model name. Returns
/// `None` if no prefix matches — callers render that as "—" or
/// omit the cost row entirely (matching upstream's `cost_status:
/// "unknown"`).
pub fn lookup(model: &str) -> Option<&'static PricingEntry> {
    let m = model.to_lowercase();
    PRICING_TABLE
        .iter()
        .find(|entry| m.starts_with(entry.model_prefix))
}

/// List of model-name prefixes the snapshot covers. Used by
/// `/model` (with no arg) to render a "known models" list so
/// the user has somewhere to look while picking a target. The
/// list isn't authoritative — anything else can still be
/// switched to via `/model <name>`; pricing for unrecognised
/// models lands as "(unknown)" in `/usage`.
pub fn known_models() -> Vec<&'static str> {
    PRICING_TABLE
        .iter()
        .map(|e| e.model_prefix)
        .collect()
}

/// Estimate session cost in USD for a model + accumulated usage
/// counters. Returns `None` when the model isn't in the snapshot
/// table.
pub fn estimate_cost(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> Option<f64> {
    let entry = lookup(model)?;
    let mut total = 0.0f64;
    total += (input_tokens as f64) * entry.input_per_million / 1_000_000.0;
    total += (output_tokens as f64) * entry.output_per_million / 1_000_000.0;
    if let Some(rate) = entry.cache_read_per_million {
        total += (cache_read_tokens as f64) * rate / 1_000_000.0;
    }
    if let Some(rate) = entry.cache_write_per_million {
        total += (cache_write_tokens as f64) * rate / 1_000_000.0;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_matches_dated_anthropic_snapshot() {
        let e = lookup("claude-sonnet-4-6").expect("entry");
        assert_eq!(e.model_prefix, "claude-sonnet-4-6");
    }

    #[test]
    fn lookup_matches_dated_openai_snapshot() {
        let e = lookup("gpt-4o-2024-11-20").expect("entry");
        assert_eq!(e.model_prefix, "gpt-4o");
    }

    #[test]
    fn lookup_returns_none_for_unknown_model() {
        assert!(lookup("definitely-not-a-real-model").is_none());
    }

    #[test]
    fn longer_prefix_wins_over_shorter() {
        // "gpt-4o-mini" must match the mini entry, not gpt-4o.
        let e = lookup("gpt-4o-mini").expect("entry");
        assert_eq!(e.model_prefix, "gpt-4o-mini");
    }

    #[test]
    fn estimate_cost_combines_all_token_classes() {
        // claude-sonnet-4-6: input 3, output 15, cache_read 0.30, cache_write 3.75
        // 1M each → 3 + 15 + 0.30 + 3.75 = 22.05
        let cost = estimate_cost(
            "claude-sonnet-4-6",
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
        )
        .unwrap();
        assert!((cost - 22.05).abs() < 0.001, "got {cost}");
    }

    #[test]
    fn estimate_cost_skips_missing_columns() {
        // OpenAI gpt-4o has no cache_write rate — the term
        // contributes 0 even with non-zero cache_write_tokens.
        let with_writes = estimate_cost("gpt-4o", 0, 0, 0, 1_000_000).unwrap();
        assert_eq!(with_writes, 0.0);
    }

    #[test]
    fn known_models_includes_each_provider_family() {
        let names = known_models();
        // At minimum: an Anthropic, an OpenAI, and a Kimi entry.
        assert!(
            names.iter().any(|n| n.starts_with("claude-")),
            "missing Anthropic family: {names:?}"
        );
        assert!(
            names.iter().any(|n| n.starts_with("gpt-")),
            "missing OpenAI family: {names:?}"
        );
        assert!(
            names.iter().any(|n| n.starts_with("kimi-")),
            "missing Kimi family: {names:?}"
        );
        // No empty entries.
        assert!(names.iter().all(|n| !n.is_empty()));
    }

    fn _coverage_check(usage: &UsageInfo) {
        // Compile-time assertion that the public types we reference
        // remain available — keeps this module honestly tied to
        // `UsageInfo`'s fields.
        let _ = usage.input_tokens;
        let _ = usage.output_tokens;
        let _ = usage.cache_read_tokens;
        let _ = usage.cache_write_tokens;
    }
}
