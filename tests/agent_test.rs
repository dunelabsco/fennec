use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;

use fennec::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use fennec::providers::traits::{ChatRequest, ChatResponse, Provider, ToolCall, UsageInfo};
use fennec::tools::traits::{Tool, ToolResult};

use fennec::agent::AgentBuilder;

// ---------------------------------------------------------------------------
// Mock Provider
// ---------------------------------------------------------------------------

/// A mock provider that returns pre-configured responses in order.
struct MockProvider {
    responses: Mutex<Vec<ChatResponse>>,
}

impl MockProvider {
    fn new(responses: Vec<ChatResponse>) -> Self {
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
        if responses.is_empty() {
            anyhow::bail!("MockProvider: no more responses");
        }
        Ok(responses.remove(0))
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        100_000
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
        }),
    };

    let provider = MockProvider::new(vec![response]);

    let mut agent = AgentBuilder::new()
        .provider(Box::new(provider))
        .memory(Arc::new(StubMemory))
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
    };

    // Second response: the model returns a final text response.
    let final_response = ChatResponse {
        content: Some("The echo said: echo: ping".to_string()),
        tool_calls: vec![],
        usage: None,
    };

    let provider = MockProvider::new(vec![tool_call_response, final_response]);

    let mut agent = AgentBuilder::new()
        .provider(Box::new(provider))
        .memory(Arc::new(StubMemory))
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
        });
    }

    let provider = MockProvider::new(responses);

    let mut agent = AgentBuilder::new()
        .provider(Box::new(provider))
        .memory(Arc::new(StubMemory))
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
