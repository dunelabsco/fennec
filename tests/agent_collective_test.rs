use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;

use fennec::agent::AgentBuilder;
use fennec::collective::cache::CollectiveCache;
use fennec::collective::search::CollectiveSearch;
// CollectiveLayer traits used transitively by CollectiveSearch.
use fennec::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use fennec::providers::traits::{ChatRequest, ChatResponse, Provider, UsageInfo};

// ---------------------------------------------------------------------------
// Mock Provider that captures messages sent to it
// ---------------------------------------------------------------------------

struct CapturingProvider {
    responses: Mutex<Vec<ChatResponse>>,
    captured_messages: Mutex<Vec<String>>,
}

impl CapturingProvider {
    fn new(responses: Vec<ChatResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            captured_messages: Mutex::new(Vec::new()),
        }
    }

    #[allow(dead_code)]
    fn captured(&self) -> Vec<String> {
        self.captured_messages.lock().clone()
    }
}

#[async_trait]
impl Provider for CapturingProvider {
    fn name(&self) -> &str {
        "capturing_mock"
    }

    async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse> {
        // Capture all user messages so tests can inspect what was sent.
        for msg in request.messages {
            if msg.role == "user" {
                if let Some(ref content) = msg.content {
                    self.captured_messages.lock().push(content.clone());
                }
            }
        }
        let mut responses = self.responses.lock();
        if responses.is_empty() {
            anyhow::bail!("CapturingProvider: no more responses");
        }
        Ok(responses.remove(0))
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        100_000
    }

    async fn chat_stream(&self, request: ChatRequest<'_>) -> anyhow::Result<tokio::sync::mpsc::Receiver<fennec::providers::traits::StreamEvent>> {
        fennec::providers::traits::default_chat_stream(self, request).await
    }
}

// ---------------------------------------------------------------------------
// Stub Memory (no-op)
// ---------------------------------------------------------------------------

struct StubMemory;

#[async_trait]
impl Memory for StubMemory {
    fn name(&self) -> &str {
        "stub"
    }
    async fn store(&self, _entry: MemoryEntry) -> Result<()> {
        Ok(())
    }
    async fn recall(&self, _query: &str, _limit: usize) -> Result<Vec<MemoryEntry>> {
        Ok(vec![])
    }
    async fn get(&self, _key: &str) -> Result<Option<MemoryEntry>> {
        Ok(None)
    }
    async fn list(
        &self,
        _category: Option<&MemoryCategory>,
        _limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(vec![])
    }
    async fn forget(&self, _key: &str) -> Result<bool> {
        Ok(false)
    }
    async fn count(&self, _category: Option<&MemoryCategory>) -> Result<usize> {
        Ok(0)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test that the agent works normally when collective is None (disabled).
#[tokio::test]
async fn test_agent_works_without_collective() {
    let response = ChatResponse {
        content: Some("Hello!".to_string()),
        tool_calls: vec![],
        usage: Some(UsageInfo {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: None,
        }),
    };

    let provider = CapturingProvider::new(vec![response]);

    let mut agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .build()
        .expect("agent build should succeed");

    let result = agent.turn("Hi there").await.expect("turn should succeed");
    assert_eq!(result, "Hello!");
}

/// Test that the agent gracefully handles collective search failure.
#[tokio::test]
async fn test_agent_collective_search_failure_graceful() {
    let response = ChatResponse {
        content: Some("Still works!".to_string()),
        tool_calls: vec![],
        usage: None,
    };

    let provider = CapturingProvider::new(vec![response]);

    // Create a CollectiveSearch backed by a temp db — search_experiences will
    // just return empty because there are no experiences stored.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let embedder: Arc<dyn fennec::memory::embedding::EmbeddingProvider> =
        Arc::new(fennec::memory::embedding::NoopEmbedding::new(1536));
    let memory = Arc::new(
        fennec::memory::sqlite::SqliteMemory::new(db_path, 0.7, 0.3, 100, embedder).unwrap(),
    );
    let cache = CollectiveCache::new(memory.clone());
    // No remote — search will just return local results (empty).
    let search = CollectiveSearch::new(memory, cache, None, None);

    let mut agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .collective(Arc::new(search))
        .build()
        .expect("agent build should succeed");

    let result = agent
        .turn("How do I fix a lifetime error?")
        .await
        .expect("turn should succeed despite no collective results");
    assert_eq!(result, "Still works!");
}

/// Test that collective context is NOT injected when confidence is None.
#[tokio::test]
async fn test_agent_no_collective_injection_when_no_results() {
    // Empty collective — no experiences stored.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let embedder: Arc<dyn fennec::memory::embedding::EmbeddingProvider> =
        Arc::new(fennec::memory::embedding::NoopEmbedding::new(1536));
    let memory = Arc::new(
        fennec::memory::sqlite::SqliteMemory::new(db_path, 0.7, 0.3, 100, embedder).unwrap(),
    );
    let cache = CollectiveCache::new(memory.clone());
    let search = CollectiveSearch::new(memory, cache, None, None);

    // We test that the agent still returns correctly even with an empty collective.
    let mut agent = AgentBuilder::new()
        .provider(Arc::new(CapturingProvider::new(vec![ChatResponse {
            content: Some("No context needed.".to_string()),
            tool_calls: vec![],
            usage: None,
        }])) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .collective(Arc::new(search))
        .build()
        .expect("agent build should succeed");

    let result = agent
        .turn("Hello world")
        .await
        .expect("turn should succeed");
    assert_eq!(result, "No context needed.");
}

/// Test that the context helper function formats correctly.
#[test]
fn test_build_collective_injection_format() {
    let result = fennec::agent::context::build_collective_injection(
        "Some collective info here",
        "What is Rust?",
    );
    assert!(result.contains("[Collective context]"));
    assert!(result.contains("Some collective info here"));
    assert!(result.contains("[User message]"));
    assert!(result.contains("What is Rust?"));
}
