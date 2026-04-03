use fennec::providers::traits::{ChatMessage, ChatResponse, ToolCall, UsageInfo};

#[test]
fn chat_message_user() {
    let msg = ChatMessage::user("hello world");
    assert_eq!(msg.role, "user");
    assert_eq!(msg.content.as_deref(), Some("hello world"));
    assert!(msg.tool_calls.is_none());
    assert!(msg.tool_call_id.is_none());
}

#[test]
fn chat_message_system() {
    let msg = ChatMessage::system("you are helpful");
    assert_eq!(msg.role, "system");
    assert_eq!(msg.content.as_deref(), Some("you are helpful"));
}

#[test]
fn chat_message_assistant() {
    let msg = ChatMessage::assistant("sure, I can help");
    assert_eq!(msg.role, "assistant");
    assert_eq!(msg.content.as_deref(), Some("sure, I can help"));
}

#[test]
fn chat_message_tool_result() {
    let msg = ChatMessage::tool_result("call_123", "file contents here");
    assert_eq!(msg.role, "tool");
    assert_eq!(msg.tool_call_id.as_deref(), Some("call_123"));
    assert_eq!(msg.content.as_deref(), Some("file contents here"));
}

#[test]
fn tool_call_creation() {
    let tc = ToolCall {
        id: "tc_001".to_string(),
        name: "read_file".to_string(),
        arguments: serde_json::json!({"path": "/tmp/test.txt"}),
    };
    assert_eq!(tc.id, "tc_001");
    assert_eq!(tc.name, "read_file");
    assert_eq!(tc.arguments["path"], "/tmp/test.txt");
}

#[test]
fn chat_response_with_tool_calls() {
    let response = ChatResponse {
        content: Some("Let me check that file.".to_string()),
        tool_calls: vec![
            ToolCall {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            },
            ToolCall {
                id: "tc_2".to_string(),
                name: "shell".to_string(),
                arguments: serde_json::json!({"command": "ls -la"}),
            },
        ],
        usage: Some(UsageInfo {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: Some(80),
        }),
    };

    assert_eq!(response.content.as_deref(), Some("Let me check that file."));
    assert_eq!(response.tool_calls.len(), 2);
    assert_eq!(response.tool_calls[0].name, "read_file");
    assert_eq!(response.tool_calls[1].name, "shell");

    let usage = response.usage.unwrap();
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.cache_read_tokens, Some(80));
}

#[test]
fn chat_response_empty() {
    let response = ChatResponse {
        content: None,
        tool_calls: vec![],
        usage: None,
    };
    assert!(response.content.is_none());
    assert!(response.tool_calls.is_empty());
    assert!(response.usage.is_none());
}

#[test]
fn chat_message_serialization_roundtrip() {
    let msg = ChatMessage::user("test message");
    let json = serde_json::to_string(&msg).expect("serialize");
    let deserialized: ChatMessage = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized.role, "user");
    assert_eq!(deserialized.content.as_deref(), Some("test message"));
}
