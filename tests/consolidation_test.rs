use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;

use fennec::memory::consolidation::MemoryConsolidator;
use fennec::memory::embedding::NoopEmbedding;
use fennec::memory::sqlite::SqliteMemory;
use fennec::memory::traits::{Memory, MemoryCategory};
use fennec::providers::traits::{ChatMessage, ChatRequest, ChatResponse, Provider};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Mock Provider
// ---------------------------------------------------------------------------

struct MockProvider {
    responses: Mutex<Vec<String>>,
}

impl MockProvider {
    fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn chat(&self, _request: ChatRequest<'_>) -> Result<ChatResponse> {
        let mut responses = self.responses.lock();
        let text = if responses.is_empty() {
            "no more responses".to_string()
        } else {
            responses.remove(0)
        };
        Ok(ChatResponse {
            content: Some(text),
            tool_calls: vec![],
            usage: None,
        })
    }

    fn supports_tool_calling(&self) -> bool {
        false
    }

    fn context_window(&self) -> usize {
        8192
    }

    async fn chat_stream(&self, request: ChatRequest<'_>) -> anyhow::Result<tokio::sync::mpsc::Receiver<fennec::providers::traits::StreamEvent>> {
        fennec::providers::traits::default_chat_stream(self, request).await
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn make_db() -> (TempDir, SqliteMemory) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let embedder = Arc::new(NoopEmbedding::new(1536));
    let mem = SqliteMemory::new(db_path, 0.7, 0.3, 10_000, embedder).expect("new sqlite memory");
    (dir, mem)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn consolidate_with_valid_json() {
    let (_dir, mem) = make_db();

    let json_response = r#"{"daily_summary": "User discussed Rust project setup", "core_facts": [{"key": "preferred-language", "content": "User prefers Rust"}, {"key": "project-name", "content": "Fennec"}]}"#;
    let provider = MockProvider::new(vec![json_response.to_string()]);
    let consolidator = MemoryConsolidator::new(Box::new(provider));

    let conversation = vec![
        ChatMessage::user("I'm working on a Rust project called Fennec"),
        ChatMessage::assistant("That sounds great! Tell me more."),
    ];

    consolidator
        .consolidate(&mem, &conversation, "sess-1")
        .await
        .expect("consolidate");

    // Verify daily summary was stored.
    let daily = mem
        .list(Some(&MemoryCategory::Daily), 10)
        .await
        .expect("list daily");
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].content, "User discussed Rust project setup");
    assert!(daily[0].key.starts_with("daily-sess-1-"));

    // Verify core facts were stored.
    let lang = mem.get("preferred-language").await.expect("get").expect("exists");
    assert_eq!(lang.content, "User prefers Rust");
    assert_eq!(lang.category, MemoryCategory::Core);

    let proj = mem.get("project-name").await.expect("get").expect("exists");
    assert_eq!(proj.content, "Fennec");
}

#[tokio::test]
async fn consolidate_with_invalid_json_falls_back() {
    let (_dir, mem) = make_db();

    let bad_response = "Sorry, I couldn't parse that properly. Here's what I think happened.";
    let provider = MockProvider::new(vec![bad_response.to_string()]);
    let consolidator = MemoryConsolidator::new(Box::new(provider));

    let conversation = vec![ChatMessage::user("Tell me about the weather")];

    consolidator
        .consolidate(&mem, &conversation, "sess-2")
        .await
        .expect("consolidate should not fail on bad JSON");

    // Should still have a daily summary with the raw text.
    let daily = mem
        .list(Some(&MemoryCategory::Daily), 10)
        .await
        .expect("list daily");
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].content, bad_response);
    assert!(daily[0].key.starts_with("daily-sess-2-"));

    // No core facts should exist.
    let core = mem
        .list(Some(&MemoryCategory::Core), 10)
        .await
        .expect("list core");
    assert_eq!(core.len(), 0);
}

#[tokio::test]
async fn consolidate_with_empty_conversation() {
    let (_dir, mem) = make_db();

    let provider = MockProvider::new(vec!["should not be called".to_string()]);
    let consolidator = MemoryConsolidator::new(Box::new(provider));

    consolidator
        .consolidate(&mem, &[], "sess-3")
        .await
        .expect("consolidate empty");

    // Nothing should be stored.
    let count = mem.count(None).await.expect("count");
    assert_eq!(count, 0);
}
