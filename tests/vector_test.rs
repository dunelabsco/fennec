use fennec::memory::vector::{cosine_similarity, hybrid_merge, ScoredResult};

// --- Cosine similarity tests ---

#[test]
fn test_identical_vectors_similarity_is_one() {
    let a = vec![1.0_f32, 2.0, 3.0];
    let sim = cosine_similarity(&a, &a);
    assert!((sim - 1.0).abs() < 1e-9, "expected ~1.0, got {sim}");
}

#[test]
fn test_orthogonal_vectors_similarity_is_zero() {
    let a = vec![1.0_f32, 0.0];
    let b = vec![0.0_f32, 1.0];
    let sim = cosine_similarity(&a, &b);
    assert!(sim.abs() < 1e-9, "expected ~0.0, got {sim}");
}

#[test]
fn test_opposite_vectors_similarity_is_negative_one() {
    let a = vec![1.0_f32, 2.0, 3.0];
    let b = vec![-1.0_f32, -2.0, -3.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim + 1.0).abs() < 1e-9, "expected ~-1.0, got {sim}");
}

#[test]
fn test_zero_vector_similarity() {
    let a = vec![0.0_f32, 0.0, 0.0];
    let b = vec![1.0_f32, 2.0, 3.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - 0.0).abs() < 1e-9, "expected 0.0, got {sim}");
}

#[test]
fn test_both_zero_vectors() {
    let a = vec![0.0_f32, 0.0];
    let b = vec![0.0_f32, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - 0.0).abs() < 1e-9, "expected 0.0, got {sim}");
}

#[test]
fn test_cosine_similarity_scaled_vector() {
    // Scaling shouldn't change cosine similarity
    let a = vec![1.0_f32, 0.0, 0.0];
    let b = vec![100.0_f32, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - 1.0).abs() < 1e-9, "expected ~1.0, got {sim}");
}

// --- Hybrid merge tests ---

#[test]
fn test_merge_deduplication() {
    let vector = vec![
        ScoredResult { id: "a".to_string(), score: 0.8 },
        ScoredResult { id: "b".to_string(), score: 0.6 },
    ];
    let keyword = vec![
        ScoredResult { id: "a".to_string(), score: 10.0 },
        ScoredResult { id: "c".to_string(), score: 5.0 },
    ];

    let merged = hybrid_merge(&vector, &keyword, 0.7, 0.3, 10);

    // "a" should appear only once (deduplicated)
    let a_count = merged.iter().filter(|r| r.id == "a").count();
    assert_eq!(a_count, 1, "expected 'a' to appear exactly once");

    // We should have exactly 3 unique IDs
    assert_eq!(merged.len(), 3);
}

#[test]
fn test_merge_limit_respected() {
    let vector = vec![
        ScoredResult { id: "a".to_string(), score: 0.9 },
        ScoredResult { id: "b".to_string(), score: 0.8 },
        ScoredResult { id: "c".to_string(), score: 0.7 },
    ];
    let keyword = vec![
        ScoredResult { id: "d".to_string(), score: 10.0 },
        ScoredResult { id: "e".to_string(), score: 8.0 },
    ];

    let merged = hybrid_merge(&vector, &keyword, 0.7, 0.3, 2);
    assert_eq!(merged.len(), 2, "expected limit of 2 results");
}

#[test]
fn test_merge_empty_inputs() {
    let empty: Vec<ScoredResult> = vec![];

    // Both empty
    let merged = hybrid_merge(&empty, &empty, 0.7, 0.3, 10);
    assert!(merged.is_empty());

    // One empty
    let vector = vec![ScoredResult { id: "a".to_string(), score: 0.5 }];
    let merged = hybrid_merge(&vector, &empty, 0.7, 0.3, 10);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "a");

    let keyword = vec![ScoredResult { id: "b".to_string(), score: 5.0 }];
    let merged = hybrid_merge(&empty, &keyword, 0.7, 0.3, 10);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "b");
}

#[test]
fn test_merge_scores_weighted_correctly() {
    let vector = vec![ScoredResult { id: "a".to_string(), score: 1.0 }];
    let keyword = vec![ScoredResult { id: "a".to_string(), score: 10.0 }];

    let merged = hybrid_merge(&vector, &keyword, 0.7, 0.3, 10);
    assert_eq!(merged.len(), 1);
    // vector contribution: 1.0 * 0.7 = 0.7
    // keyword contribution: (10.0/10.0) * 0.3 = 0.3
    // total: 1.0
    let score = merged[0].score;
    assert!((score - 1.0).abs() < 1e-9, "expected 1.0, got {score}");
}

#[test]
fn test_merge_sorted_descending() {
    let vector = vec![
        ScoredResult { id: "low".to_string(), score: 0.1 },
        ScoredResult { id: "high".to_string(), score: 0.9 },
        ScoredResult { id: "mid".to_string(), score: 0.5 },
    ];
    let keyword: Vec<ScoredResult> = vec![];

    let merged = hybrid_merge(&vector, &keyword, 1.0, 0.0, 10);
    assert_eq!(merged[0].id, "high");
    assert_eq!(merged[1].id, "mid");
    assert_eq!(merged[2].id, "low");
}
