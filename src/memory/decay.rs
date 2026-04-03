use crate::memory::traits::{MemoryCategory, MemoryEntry};

/// Default half-life for memory decay, in days.
pub const DEFAULT_HALF_LIFE_DAYS: f64 = 7.0;

/// Apply exponential time decay to memory entries in-place.
///
/// Formula: `score * 2^(-age_days / half_life)` implemented as
/// `score * exp(-age_days / half_life * LN_2)`.
///
/// - **Core** entries are exempt from decay.
/// - Entries with no score (`None`) are skipped.
/// - Entries with unparseable timestamps are skipped.
pub fn apply_time_decay(entries: &mut [MemoryEntry], half_life_days: f64) {
    let now = chrono::Utc::now();

    for entry in entries.iter_mut() {
        // Core entries never decay
        if entry.category == MemoryCategory::Core {
            continue;
        }

        // Skip entries without a score
        let score = match entry.score {
            Some(s) => s,
            None => continue,
        };

        // Parse the updated_at timestamp; skip on failure
        let updated = match chrono::DateTime::parse_from_rfc3339(&entry.updated_at) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => continue,
        };

        let age_days = (now - updated).num_seconds() as f64 / 86_400.0;

        // Don't let negative age (future timestamps) cause score inflation
        let age_days = age_days.max(0.0);

        let decay_factor = (-age_days / half_life_days * std::f64::consts::LN_2).exp();
        entry.score = Some(score * decay_factor);
    }
}
