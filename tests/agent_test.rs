use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;

use fennec::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use fennec::providers::traits::{
    ChatMessage, ChatRequest, ChatResponse, Provider, ToolCall, UsageInfo,
};
use fennec::tools::traits::{Tool, ToolResult};

use fennec::agent::AgentBuilder;

// ---------------------------------------------------------------------------
// Mock Provider
// ---------------------------------------------------------------------------

/// A mock provider that returns pre-configured responses in order.
struct MockProvider {
    responses: Mutex<Vec<ChatResponse>>,
    /// Reported context window. Small values let tests trigger auto-compaction.
    ctx_window: usize,
}

impl MockProvider {
    fn new(responses: Vec<ChatResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            ctx_window: 100_000,
        }
    }

    /// Override the reported context window (default 100_000).
    fn with_context_window(mut self, ctx_window: usize) -> Self {
        self.ctx_window = ctx_window;
        self
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn chat(&self, _request: ChatRequest<'_>) -> Result<ChatResponse> {
        let mut responses = self.responses.lock();
        if responses.is_empty() {
            anyhow::bail!("MockProvider: no more responses");
        }
        Ok(responses.remove(0))
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        self.ctx_window
    }

    async fn chat_stream(&self, request: ChatRequest<'_>) -> anyhow::Result<tokio::sync::mpsc::Receiver<fennec::providers::traits::StreamEvent>> {
        fennec::providers::traits::default_chat_stream(self, request).await
    }
}

// ---------------------------------------------------------------------------
// Echo Tool
// ---------------------------------------------------------------------------

/// A simple tool that echoes its input.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes back the input message."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to echo"
                }
            },
            "required": ["message"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("(no message)");
        Ok(ToolResult {
            success: true,
            output: format!("echo: {message}"),
            error: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Stub Memory
// ---------------------------------------------------------------------------

/// A no-op memory backend for testing.
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

#[tokio::test]
async fn test_simple_chat_response() {
    let response = ChatResponse {
        content: Some("Hello from the mock!".to_string()),
        tool_calls: vec![],
        usage: Some(UsageInfo {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }),
        reasoning: None,
    };

    let provider = MockProvider::new(vec![response]);

    let mut agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .build()
        .expect("agent build should succeed");

    let result = agent.turn("Hi there").await.expect("turn should succeed");
    assert_eq!(result, "Hello from the mock!");
}

#[tokio::test]
async fn test_tool_call_and_response() {
    // First response: the model requests a tool call.
    let tool_call_response = ChatResponse {
        content: None,
        tool_calls: vec![ToolCall {
            id: "tc_1".to_string(),
            name: "echo".to_string(),
            arguments: json!({"message": "ping"}),
        }],
        usage: None,
        reasoning: None,
    };

    // Second response: the model returns a final text response.
    let final_response = ChatResponse {
        content: Some("The echo said: echo: ping".to_string()),
        tool_calls: vec![],
        usage: None,
        reasoning: None,
    };

    let provider = MockProvider::new(vec![tool_call_response, final_response]);

    let mut agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .tool(Box::new(EchoTool))
        .build()
        .expect("agent build should succeed");

    let result = agent.turn("echo ping").await.expect("turn should succeed");
    assert_eq!(result, "The echo said: echo: ping");
}

#[tokio::test]
async fn test_max_iterations_exceeded() {
    // Create responses that always return tool calls (more than max iterations).
    let max_iters = 3;
    let mut responses = Vec::new();
    for i in 0..(max_iters + 1) {
        responses.push(ChatResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: format!("tc_{i}"),
                name: "echo".to_string(),
                arguments: json!({"message": "loop"}),
            }],
            usage: None,
            reasoning: None,
        });
    }

    let provider = MockProvider::new(responses);

    let mut agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .tool(Box::new(EchoTool))
        .max_tool_iterations(max_iters)
        .build()
        .expect("agent build should succeed");

    let result = agent.turn("loop forever").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("max tool iterations"),
        "error should mention max tool iterations, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Automatic context compaction
// ---------------------------------------------------------------------------

/// Build a multi-message history big enough to exceed a small context
/// threshold and long enough for the compressor's middle block to be
/// non-empty (> protect_first + protect_last).
fn big_history() -> Vec<ChatMessage> {
    (0..12)
        .map(|i| {
            let body = format!("conversation message number {i} with some filler text");
            if i % 2 == 0 {
                ChatMessage::user(body)
            } else {
                ChatMessage::assistant(body)
            }
        })
        .collect()
}

#[tokio::test]
async fn test_auto_compaction_triggers_over_threshold() {
    // Two responses: the first is the compaction summary, the second is the
    // turn's actual reply. If auto-compaction fires, it consumes the summary
    // response and the turn returns the second ("done"). If it does NOT fire,
    // the turn would consume the summary response first and return that.
    let responses = vec![
        ChatResponse {
            content: Some("[SUMMARY]".to_string()),
            tool_calls: vec![],
            usage: None,
            reasoning: None,
        },
        ChatResponse {
            content: Some("done".to_string()),
            tool_calls: vec![],
            usage: None,
            reasoning: None,
        },
    ];
    // Tiny context window so the loaded history is over the 50% threshold.
    let provider = MockProvider::new(responses).with_context_window(40);

    let mut agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .build()
        .expect("agent build should succeed");

    agent.replace_history(big_history());
    let len_before = agent.history_len();

    let result = agent.turn("go").await.expect("turn should succeed");
    assert_eq!(
        result, "done",
        "auto-compaction should have consumed the summary response, leaving the turn to return 'done'"
    );
    assert!(
        agent.history_len() < len_before,
        "history should be smaller after compaction (was {len_before}, now {})",
        agent.history_len()
    );
}

#[tokio::test]
async fn test_no_compaction_under_threshold() {
    // Same setup, but a large context window keeps history under threshold, so
    // compaction must NOT fire — the turn returns the first response.
    let responses = vec![
        ChatResponse {
            content: Some("[SUMMARY]".to_string()),
            tool_calls: vec![],
            usage: None,
            reasoning: None,
        },
        ChatResponse {
            content: Some("done".to_string()),
            tool_calls: vec![],
            usage: None,
            reasoning: None,
        },
    ];
    let provider = MockProvider::new(responses); // default 100_000-token window

    let mut agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .build()
        .expect("agent build should succeed");

    agent.replace_history(big_history());

    let result = agent.turn("go").await.expect("turn should succeed");
    assert_eq!(
        result, "[SUMMARY]",
        "no compaction under threshold — the turn returns the first response unchanged"
    );
}

#[tokio::test]
async fn test_compaction_disabled_skips_compaction() {
    // With compression disabled, even an over-threshold history must not
    // compact: the turn returns the first response.
    let responses = vec![
        ChatResponse {
            content: Some("[SUMMARY]".to_string()),
            tool_calls: vec![],
            usage: None,
            reasoning: None,
        },
        ChatResponse {
            content: Some("done".to_string()),
            tool_calls: vec![],
            usage: None,
            reasoning: None,
        },
    ];
    let provider = MockProvider::new(responses).with_context_window(40);

    let mut agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn Provider>)
        .memory(Arc::new(StubMemory) as Arc<dyn Memory>)
        .compression_enabled(false)
        .build()
        .expect("agent build should succeed");

    agent.replace_history(big_history());

    let result = agent.turn("go").await.expect("turn should succeed");
    assert_eq!(
        result, "[SUMMARY]",
        "compaction disabled — no compaction even over threshold"
    );
}
