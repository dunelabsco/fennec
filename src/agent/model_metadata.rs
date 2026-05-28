//! Per-model metadata: context-window size and token pricing.
//!
//! Two layers:
//!   1. A baked-in **static baseline** — context windows for the major model
//!      families (offline-safe), plus pricing from [`super::pricing`].
//!   2. A **models.dev overlay** — the community catalog at
//!      `https://models.dev/api.json`, fetched and cached to disk (24h TTL),
//!      which refines/extends both context and cost for any model it knows.
//!
//! Lookups consult the overlay first, then fall back to the static baseline,
//! so accuracy improves when online but nothing breaks offline. This is what
//! `/usage` reads for the context bar + cost, and what the context-compaction
//! threshold should key off per-model.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::schema::FennecConfig;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_FILE: &str = "models_dev_cache.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Resolved metadata for one model (any field may be unknown).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelMeta {
    pub context_window: Option<usize>,
    pub input_per_million: Option<f64>,
    pub output_per_million: Option<f64>,
    pub cache_read_per_million: Option<f64>,
    pub cache_write_per_million: Option<f64>,
}

/// Static context-window baseline by model-name prefix. Longest matching
/// prefix wins (so `gpt-4o` beats `gpt-4`). Approximate, version-agnostic
/// values; the models.dev overlay refines them when available.
const STATIC_CONTEXT: &[(&str, usize)] = &[
    ("claude-", 200_000),
    ("gpt-4o-mini", 128_000),
    ("gpt-4o", 128_000),
    ("gpt-4.1", 1_047_576),
    ("gpt-4-turbo", 128_000),
    ("gpt-4", 8_192),
    ("gpt-3.5", 16_385),
    ("gpt-5", 400_000),
    ("o1", 200_000),
    ("o3", 200_000),
    ("o4", 200_000),
    ("gemini-1.5-pro", 2_097_152),
    ("gemini-1.5-flash", 1_048_576),
    ("gemini-2", 1_048_576),
    ("gemini-3", 1_048_576),
    ("kimi", 262_144),
    ("moonshot", 131_072),
    ("llama", 128_000),
    ("mistral", 32_768),
    ("deepseek", 65_536),
    ("grok", 131_072),
];

/// On-disk cache shape.
#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    fetched_at: u64,
    models: HashMap<String, ModelMeta>,
}

/// The models.dev overlay, lazily loaded from the on-disk cache (if any).
static OVERLAY: LazyLock<RwLock<HashMap<String, ModelMeta>>> =
    LazyLock::new(|| RwLock::new(load_cache().unwrap_or_default()));

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path() -> std::path::PathBuf {
    FennecConfig::resolve_home(None).join(CACHE_FILE)
}

/// Read the cached models.dev overlay from disk (ignores TTL — a stale cache
/// is better than nothing; [`refresh`] updates it).
fn load_cache() -> Option<HashMap<String, ModelMeta>> {
    let data = std::fs::read_to_string(cache_path()).ok()?;
    let cache: CacheFile = serde_json::from_str(&data).ok()?;
    Some(cache.models)
}

fn cache_age() -> Option<Duration> {
    let data = std::fs::read_to_string(cache_path()).ok()?;
    let cache: CacheFile = serde_json::from_str(&data).ok()?;
    Some(Duration::from_secs(now_secs().saturating_sub(cache.fetched_at)))
}

/// Longest-prefix match against the static context table.
fn static_context(model: &str) -> Option<usize> {
    STATIC_CONTEXT
        .iter()
        .filter(|(prefix, _)| model.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, window)| *window)
}

fn overlay_get(model: &str) -> Option<ModelMeta> {
    OVERLAY
        .read()
        .ok()
        .and_then(|map| map.get(model).cloned())
}

/// Resolve the context-window size for a model: models.dev overlay first, then
/// the static baseline. `None` when neither knows it (caller should fall back
/// to the provider's default).
pub fn context_window(model: &str) -> Option<usize> {
    if let Some(meta) = overlay_get(model) {
        if meta.context_window.is_some() {
            return meta.context_window;
        }
    }
    static_context(model)
}

/// Estimate USD cost from token counts: models.dev overlay first (when it
/// carries pricing), then the static [`super::pricing`] snapshot.
pub fn estimate_cost(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> Option<f64> {
    if let Some(meta) = overlay_get(model) {
        if let (Some(input), Some(output)) = (meta.input_per_million, meta.output_per_million) {
            let mut total = (input_tokens as f64) * input / 1_000_000.0
                + (output_tokens as f64) * output / 1_000_000.0;
            if let Some(rate) = meta.cache_read_per_million {
                total += (cache_read_tokens as f64) * rate / 1_000_000.0;
            }
            if let Some(rate) = meta.cache_write_per_million {
                total += (cache_write_tokens as f64) * rate / 1_000_000.0;
            }
            return Some(total);
        }
    }
    super::pricing::estimate_cost(
        model,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    )
}

/// Parse the models.dev `api.json` catalog (keyed by provider → `models` →
/// entry) into a flat `model_id → ModelMeta` map.
fn parse_models_dev(json: &Value) -> HashMap<String, ModelMeta> {
    let mut out = HashMap::new();
    let Some(providers) = json.as_object() else {
        return out;
    };
    for provider in providers.values() {
        let Some(models) = provider.get("models").and_then(|m| m.as_object()) else {
            continue;
        };
        for (model_id, entry) in models {
            let limit = entry.get("limit");
            let cost = entry.get("cost");
            let meta = ModelMeta {
                context_window: limit
                    .and_then(|l| l.get("context"))
                    .and_then(|c| c.as_u64())
                    .map(|c| c as usize),
                input_per_million: cost.and_then(|c| c.get("input")).and_then(|v| v.as_f64()),
                output_per_million: cost.and_then(|c| c.get("output")).and_then(|v| v.as_f64()),
                cache_read_per_million: cost
                    .and_then(|c| c.get("cache_read"))
                    .and_then(|v| v.as_f64()),
                cache_write_per_million: cost
                    .and_then(|c| c.get("cache_write"))
                    .and_then(|v| v.as_f64()),
            };
            // Only keep entries that carry at least one useful field.
            if meta.context_window.is_some() || meta.input_per_million.is_some() {
                out.insert(model_id.clone(), meta);
            }
        }
    }
    out
}

/// Fetch the models.dev catalog, update the in-memory overlay, and write the
/// disk cache. Best-effort — callers typically ignore the result.
pub async fn refresh() -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building models.dev client")?;
    let json: Value = client
        .get(MODELS_DEV_URL)
        .send()
        .await
        .context("fetching models.dev catalog")?
        .json()
        .await
        .context("parsing models.dev catalog")?;

    let models = parse_models_dev(&json);
    if models.is_empty() {
        anyhow::bail!("models.dev catalog parsed to zero entries");
    }

    let cache = CacheFile {
        fetched_at: now_secs(),
        models: models.clone(),
    };
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(serialized) = serde_json::to_string(&cache) {
        let _ = std::fs::write(&path, serialized);
    }

    if let Ok(mut overlay) = OVERLAY.write() {
        *overlay = models;
    }
    Ok(())
}

/// Spawn a best-effort background refresh when the cache is missing or older
/// than the TTL. Safe to call at startup; never blocks or fails the caller.
pub fn refresh_in_background() {
    let needs_refresh = match cache_age() {
        Some(age) => age > CACHE_TTL,
        None => true,
    };
    if needs_refresh {
        tokio::spawn(async {
            if let Err(e) = refresh().await {
                tracing::debug!("models.dev refresh skipped: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_context_longest_prefix_wins() {
        // gpt-4o must beat gpt-4 (longer prefix), and gpt-4.1 its own window.
        assert_eq!(static_context("gpt-4o-2024-11-20"), Some(128_000));
        assert_eq!(static_context("gpt-4.1"), Some(1_047_576));
        assert_eq!(static_context("gpt-4-turbo"), Some(128_000));
        assert_eq!(static_context("gpt-4"), Some(8_192));
        assert_eq!(static_context("claude-sonnet-4-6"), Some(200_000));
        assert_eq!(static_context("gemini-2.5-flash"), Some(1_048_576));
        assert_eq!(static_context("gemini-1.5-pro"), Some(2_097_152));
        assert!(static_context("totally-unknown-model").is_none());
    }

    #[test]
    fn context_window_falls_back_to_static_when_overlay_empty() {
        // No overlay entry for this in tests → static baseline applies.
        assert_eq!(context_window("claude-opus-4-20250101"), Some(200_000));
    }

    #[test]
    fn estimate_cost_falls_back_to_pricing_snapshot() {
        // Without an overlay entry, this delegates to pricing.rs — same result
        // as calling pricing::estimate_cost directly.
        let via_meta = estimate_cost("claude-sonnet-4-6", 1_000_000, 1_000_000, 0, 0);
        let via_pricing = super::super::pricing::estimate_cost(
            "claude-sonnet-4-6",
            1_000_000,
            1_000_000,
            0,
            0,
        );
        assert_eq!(via_meta, via_pricing);
        assert!(via_meta.is_some());
    }

    #[test]
    fn parse_models_dev_extracts_context_and_cost() {
        let json = serde_json::json!({
            "anthropic": {
                "id": "anthropic",
                "models": {
                    "claude-opus-4-1-20250805": {
                        "id": "claude-opus-4-1-20250805",
                        "limit": { "context": 200000, "output": 32000 },
                        "cost": { "input": 15, "output": 75, "cache_read": 1.5, "cache_write": 18.75 }
                    }
                }
            },
            "openai": {
                "models": {
                    "gpt-4.1": { "limit": { "context": 1047576 }, "cost": { "input": 2, "output": 8 } },
                    "freebie": { "name": "no useful fields" }
                }
            }
        });
        let map = parse_models_dev(&json);
        let opus = map.get("claude-opus-4-1-20250805").unwrap();
        assert_eq!(opus.context_window, Some(200_000));
        assert_eq!(opus.input_per_million, Some(15.0));
        assert_eq!(opus.cache_write_per_million, Some(18.75));
        let g = map.get("gpt-4.1").unwrap();
        assert_eq!(g.context_window, Some(1_047_576));
        assert_eq!(g.output_per_million, Some(8.0));
        // Entry with no useful fields is dropped.
        assert!(!map.contains_key("freebie"));
    }

    #[test]
    fn estimate_cost_uses_overlay_when_present() {
        // Inject an overlay entry and confirm it's used over the static table.
        OVERLAY.write().unwrap().insert(
            "test-overlay-model".to_string(),
            ModelMeta {
                context_window: Some(500_000),
                input_per_million: Some(1.0),
                output_per_million: Some(2.0),
                cache_read_per_million: None,
                cache_write_per_million: None,
            },
        );
        assert_eq!(context_window("test-overlay-model"), Some(500_000));
        // 1M input @1 + 1M output @2 = 3.0
        let cost = estimate_cost("test-overlay-model", 1_000_000, 1_000_000, 0, 0).unwrap();
        assert!((cost - 3.0).abs() < 1e-9, "got {cost}");
        OVERLAY.write().unwrap().remove("test-overlay-model");
    }
}
