/// A scored search result pairing an entry ID with its relevance score.
#[derive(Debug, Clone)]
pub struct ScoredResult {
    pub id: String,
    pub score: f64,
}

/// Compute cosine similarity between two vectors.
///
/// Returns 0.0 if either vector has zero magnitude.
/// Panics if the vectors differ in length.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len(), "vectors must have equal length");

    let mut dot: f64 = 0.0;
    let mut norm_a: f64 = 0.0;
    let mut norm_b: f64 = 0.0;

    for (ai, bi) in a.iter().zip(b.iter()) {
        let af = *ai as f64;
        let bf = *bi as f64;
        dot += af * bf;
        norm_a += af * af;
        norm_b += bf * bf;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return 0.0;
    }

    dot / denom
}

/// Merge vector and keyword search results using weighted linear combination.
///
/// - Keyword scores are normalized by dividing by the maximum keyword score.
/// - Vector scores are assumed to already be in [0, 1] (e.g., cosine similarity).
/// - Duplicate IDs are combined (scores summed from both sources).
/// - Results are sorted by descending score and truncated to `limit`.
pub fn hybrid_merge(
    vector_results: &[ScoredResult],
    keyword_results: &[ScoredResult],
    vector_weight: f64,
    keyword_weight: f64,
    limit: usize,
) -> Vec<ScoredResult> {
    use std::collections::HashMap;

    let mut scores: HashMap<String, f64> = HashMap::new();

    // Add vector scores (weighted)
    for r in vector_results {
        *scores.entry(r.id.clone()).or_insert(0.0) += r.score * vector_weight;
    }

    // Normalize keyword scores by the max keyword score
    let max_keyword = keyword_results
        .iter()
        .map(|r| r.score)
        .fold(f64::NEG_INFINITY, f64::max);

    if max_keyword > 0.0 {
        for r in keyword_results {
            let normalized = r.score / max_keyword;
            *scores.entry(r.id.clone()).or_insert(0.0) += normalized * keyword_weight;
        }
    }

    // Sort by score descending
    let mut results: Vec<ScoredResult> = scores
        .into_iter()
        .map(|(id, score)| ScoredResult { id, score })
        .collect();

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    results.truncate(limit);
    results
}
