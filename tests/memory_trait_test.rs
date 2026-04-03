use fennec::memory::{MemoryCategory, MemoryEntry};

#[test]
fn test_category_serialize_core() {
    let json = serde_json::to_string(&MemoryCategory::Core).unwrap();
    assert_eq!(json, "\"core\"");
}

#[test]
fn test_category_serialize_daily() {
    let json = serde_json::to_string(&MemoryCategory::Daily).unwrap();
    assert_eq!(json, "\"daily\"");
}

#[test]
fn test_category_serialize_conversation() {
    let json = serde_json::to_string(&MemoryCategory::Conversation).unwrap();
    assert_eq!(json, "\"conversation\"");
}

#[test]
fn test_category_serialize_custom() {
    let json = serde_json::to_string(&MemoryCategory::Custom("project".to_string())).unwrap();
    assert_eq!(json, "\"project\"");
}

#[test]
fn test_category_deserialize_core() {
    let cat: MemoryCategory = serde_json::from_str("\"core\"").unwrap();
    assert_eq!(cat, MemoryCategory::Core);
}

#[test]
fn test_category_deserialize_daily() {
    let cat: MemoryCategory = serde_json::from_str("\"daily\"").unwrap();
    assert_eq!(cat, MemoryCategory::Daily);
}

#[test]
fn test_category_deserialize_conversation() {
    let cat: MemoryCategory = serde_json::from_str("\"conversation\"").unwrap();
    assert_eq!(cat, MemoryCategory::Conversation);
}

#[test]
fn test_category_deserialize_custom() {
    let cat: MemoryCategory = serde_json::from_str("\"workflow\"").unwrap();
    assert_eq!(cat, MemoryCategory::Custom("workflow".to_string()));
}

#[test]
fn test_category_roundtrip() {
    let categories = vec![
        MemoryCategory::Core,
        MemoryCategory::Daily,
        MemoryCategory::Conversation,
        MemoryCategory::Custom("special".to_string()),
    ];
    for cat in categories {
        let json = serde_json::to_string(&cat).unwrap();
        let back: MemoryCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(cat, back);
    }
}

#[test]
fn test_entry_defaults() {
    let entry = MemoryEntry::default();
    assert!(!entry.id.is_empty());
    assert_eq!(entry.key, "");
    assert_eq!(entry.content, "");
    assert_eq!(entry.category, MemoryCategory::Conversation);
    assert!(!entry.created_at.is_empty());
    assert!(!entry.updated_at.is_empty());
    assert!(entry.session_id.is_none());
    assert_eq!(entry.namespace, "default");
    assert!(entry.importance.is_none());
    assert!(entry.score.is_none());
    assert!(entry.superseded_by.is_none());
}

#[test]
fn test_entry_serialization_roundtrip() {
    let entry = MemoryEntry {
        key: "test_key".to_string(),
        content: "Some memory content".to_string(),
        category: MemoryCategory::Core,
        importance: Some(0.9),
        score: Some(0.85),
        ..MemoryEntry::default()
    };

    let json = serde_json::to_string(&entry).unwrap();
    let back: MemoryEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.key, "test_key");
    assert_eq!(back.content, "Some memory content");
    assert_eq!(back.category, MemoryCategory::Core);
    assert_eq!(back.importance, Some(0.9));
    assert_eq!(back.score, Some(0.85));
    assert_eq!(back.namespace, "default");
}
