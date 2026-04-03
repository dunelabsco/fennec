use fennec::agent::compressor::ContextCompressor;
use fennec::providers::traits::{ChatMessage, ToolCall};

#[test]
fn test_should_compress_under_threshold() {
    let compressor = ContextCompressor::new(0.50, 3, 4);
    // context_window = 1000 tokens => threshold = 500 tokens => 2000 chars.
    // 5 messages x ~8 chars = 40 chars => ~10 tokens, well under 500.
    let messages: Vec<ChatMessage> = (0..5)
        .map(|i| ChatMessage::user(format!("msg {i:04}")))
        .collect();
    assert!(!compressor.should_compress(&messages, 1000));
}

#[test]
fn test_should_compress_over_threshold() {
    let compressor = ContextCompressor::new(0.50, 3, 4);
    // context_window = 100 tokens => threshold = 50 tokens => 200 chars.
    // 10 messages x 100 chars = 1000 chars => 250 tokens, over 50.
    let long_text = "x".repeat(100);
    let messages: Vec<ChatMessage> = (0..10)
        .map(|_| ChatMessage::user(&long_text))
        .collect();
    assert!(compressor.should_compress(&messages, 100));
}

#[test]
fn test_tool_output_pruning() {
    // protect_first=2, protect_last=1 — middle zone is indices 2..len-1.
    let compressor = ContextCompressor::new(0.50, 2, 1);

    let mut assistant = ChatMessage::assistant("calling tools");
    assistant.tool_calls = Some(vec![
        ToolCall {
            id: "tc_1".to_string(),
            name: "test".to_string(),
            arguments: serde_json::Value::Null,
        },
        ToolCall {
            id: "tc_2".to_string(),
            name: "test".to_string(),
            arguments: serde_json::Value::Null,
        },
    ]);

    let mut messages = vec![
        ChatMessage::user("start"),                          // 0: protected
        assistant,                                           // 1: protected
        ChatMessage::tool_result("tc_1", &"a".repeat(300)), // 2: middle — long
        ChatMessage::tool_result("tc_2", &"b".repeat(300)), // 3: middle — long
        ChatMessage::user("end"),                            // 4: protected (last 1)
    ];

    // Manually do Phase 1 pruning (same logic as compress Phase 1).
    let pf = compressor.protect_first_val();
    let pl = compressor.protect_last_val();
    let end = messages.len().saturating_sub(pl);
    for i in pf..end {
        if messages[i].role == "tool"
            && messages[i]
                .content
                .as_ref()
                .map_or(false, |c| c.len() > 200)
        {
            messages[i].content =
                Some("[Old tool output cleared to save context]".to_string());
        }
    }

    assert_eq!(
        messages[2].content.as_deref().unwrap(),
        "[Old tool output cleared to save context]"
    );
    assert_eq!(
        messages[3].content.as_deref().unwrap(),
        "[Old tool output cleared to save context]"
    );
}

#[test]
fn test_orphan_sanitization() {
    let compressor = ContextCompressor::new(0.50, 0, 0);

    let mut assistant = ChatMessage::assistant("calling tools");
    assistant.tool_calls = Some(vec![ToolCall {
        id: "tc_keep".to_string(),
        name: "test".to_string(),
        arguments: serde_json::Value::Null,
    }]);

    let mut messages = vec![
        assistant,
        ChatMessage::tool_result("tc_keep", "kept result"),
        // Orphaned: no assistant message has tool_call id "tc_orphan".
        ChatMessage::tool_result("tc_orphan", "orphaned result"),
    ];

    compressor.sanitize_orphans(&mut messages);

    let tool_results: Vec<_> = messages
        .iter()
        .filter(|m| m.role == "tool")
        .collect();
    assert_eq!(tool_results.len(), 1);
    assert_eq!(
        tool_results[0].tool_call_id.as_deref().unwrap(),
        "tc_keep"
    );
}
