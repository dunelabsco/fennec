use chrono::{Duration, Utc};
use fennec::memory::decay::{apply_time_decay, DEFAULT_HALF_LIFE_DAYS};
use fennec::memory::{MemoryCategory, MemoryEntry};

fn make_entry(category: MemoryCategory, score: Option<f64>, age: Duration) -> MemoryEntry {
    let timestamp = (Utc::now() - age).to_rfc3339();
    MemoryEntry {
        category,
        score,
        updated_at: timestamp,
        ..MemoryEntry::default()
    }
}

#[test]
fn test_default_half_life_constant() {
    assert!((DEFAULT_HALF_LIFE_DAYS - 7.0).abs() < f64::EPSILON);
}

#[test]
fn test_core_never_decays() {
    let mut entries = vec![make_entry(
        MemoryCategory::Core,
        Some(1.0),
        Duration::days(30),
    )];

    apply_time_decay(&mut entries, 7.0);

    assert!(
        (entries[0].score.unwrap() - 1.0).abs() < f64::EPSILON,
        "Core entry score should remain 1.0, got {}",
        entries[0].score.unwrap()
    );
}

#[test]
fn test_daily_decays_half_after_one_half_life() {
    let mut entries = vec![make_entry(
        MemoryCategory::Daily,
        Some(1.0),
        Duration::days(7),
    )];

    apply_time_decay(&mut entries, 7.0);

    let score = entries[0].score.unwrap();
    // After exactly one half-life, score should be ~0.5
    assert!(
        (score - 0.5).abs() < 0.05,
        "expected ~0.5 after one half-life, got {score}"
    );
}

#[test]
fn test_fresh_entry_barely_decays() {
    let mut entries = vec![make_entry(
        MemoryCategory::Conversation,
        Some(1.0),
        Duration::seconds(60), // 1 minute old
    )];

    apply_time_decay(&mut entries, 7.0);

    let score = entries[0].score.unwrap();
    assert!(
        score > 0.99,
        "fresh entry should barely decay, got {score}"
    );
}

#[test]
fn test_old_entry_decays_heavily() {
    let mut entries = vec![make_entry(
        MemoryCategory::Daily,
        Some(1.0),
        Duration::days(70), // 10 half-lives
    )];

    apply_time_decay(&mut entries, 7.0);

    let score = entries[0].score.unwrap();
    // After 10 half-lives: 2^(-10) ≈ 0.000977
    assert!(
        score < 0.01,
        "entry after 10 half-lives should be nearly zero, got {score}"
    );
    assert!(
        (score - 1.0 / 1024.0).abs() < 0.001,
        "expected ~0.000977, got {score}"
    );
}

#[test]
fn test_no_score_entries_skipped() {
    let mut entries = vec![make_entry(
        MemoryCategory::Daily,
        None,
        Duration::days(7),
    )];

    apply_time_decay(&mut entries, 7.0);

    assert!(
        entries[0].score.is_none(),
        "entries with no score should remain None"
    );
}

#[test]
fn test_unparseable_timestamp_skipped() {
    let mut entries = vec![MemoryEntry {
        category: MemoryCategory::Daily,
        score: Some(1.0),
        updated_at: "not-a-timestamp".to_string(),
        ..MemoryEntry::default()
    }];

    apply_time_decay(&mut entries, 7.0);

    assert!(
        (entries[0].score.unwrap() - 1.0).abs() < f64::EPSILON,
        "entries with bad timestamps should not be modified"
    );
}

#[test]
fn test_custom_half_life() {
    // Use a 1-day half-life: after 1 day, score should be ~0.5
    let mut entries = vec![make_entry(
        MemoryCategory::Conversation,
        Some(1.0),
        Duration::days(1),
    )];

    apply_time_decay(&mut entries, 1.0);

    let score = entries[0].score.unwrap();
    assert!(
        (score - 0.5).abs() < 0.05,
        "expected ~0.5 with 1-day half-life after 1 day, got {score}"
    );
}

#[test]
fn test_multiple_entries_mixed() {
    let mut entries = vec![
        make_entry(MemoryCategory::Core, Some(1.0), Duration::days(100)),
        make_entry(MemoryCategory::Daily, Some(1.0), Duration::days(7)),
        make_entry(MemoryCategory::Conversation, None, Duration::days(7)),
        make_entry(MemoryCategory::Custom("project".into()), Some(0.8), Duration::days(14)),
    ];

    apply_time_decay(&mut entries, 7.0);

    // Core: unchanged
    assert!((entries[0].score.unwrap() - 1.0).abs() < f64::EPSILON);
    // Daily after 1 half-life: ~0.5
    assert!((entries[1].score.unwrap() - 0.5).abs() < 0.05);
    // No score: unchanged
    assert!(entries[2].score.is_none());
    // Custom after 2 half-lives: ~0.8 * 0.25 = 0.2
    assert!((entries[3].score.unwrap() - 0.2).abs() < 0.05);
}
