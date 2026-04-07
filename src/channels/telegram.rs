use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;

use crate::bus::InboundMessage;

use super::traits::{Channel, SendMessage};

/// Maximum message length allowed by the Telegram Bot API.
const TELEGRAM_MAX_LEN: usize = 4096;

/// Telegram channel using the Bot API with long-polling and streaming edits.
pub struct TelegramChannel {
    bot_token: String,
    client: reqwest::Client,
    allowed_users: Vec<String>,
    /// Per-chat timestamp of the last edit, used for rate-limiting streaming deltas.
    last_edit: Arc<Mutex<HashMap<String, Instant>>>,
}

impl TelegramChannel {
    pub fn new(bot_token: String, allowed_users: Vec<String>) -> Self {
        Self {
            bot_token,
            client: reqwest::Client::new(),
            allowed_users,
            last_edit: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.bot_token, method)
    }

    /// Register bot commands with Telegram so they appear in the menu.
    async fn register_commands(&self) -> Result<()> {
        let commands = serde_json::json!([
            {"command": "new", "description": "Start a new conversation"},
            {"command": "status", "description": "Show agent status"},
            {"command": "help", "description": "Show available commands"},
        ]);
        self.client
            .post(self.api_url("setMyCommands"))
            .json(&serde_json::json!({"commands": commands}))
            .send()
            .await
            .context("Telegram setMyCommands request failed")?;
        Ok(())
    }

    /// Parse a Telegram `getUpdates` JSON response into a list of
    /// (update_id, sender_id, chat_id, text) tuples.
    pub fn parse_updates(body: &Value) -> Vec<(i64, String, String, String)> {
        let mut results = Vec::new();
        if let Some(arr) = body.get("result").and_then(|v| v.as_array()) {
            for update in arr {
                let update_id = update.get("update_id").and_then(|v| v.as_i64()).unwrap_or(0);
                if let Some(message) = update.get("message") {
                    let text = message
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if text.is_empty() {
                        continue;
                    }
                    let sender_id = message
                        .get("from")
                        .and_then(|f| f.get("id"))
                        .and_then(|v| v.as_i64())
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                    let chat_id = message
                        .get("chat")
                        .and_then(|c| c.get("id"))
                        .and_then(|v| v.as_i64())
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                    results.push((update_id, sender_id, chat_id, text));
                }
            }
        }
        results
    }
}

/// Split a long message into parts that fit within `max_len`, preserving code
/// blocks (triple-backtick state). Each part gets a `(i/N)` indicator when
/// there are multiple parts.
pub fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_code_block = false;
    let mut code_fence_lang = String::new();

    for line in text.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if in_code_block {
                // Closing a code block.
                in_code_block = false;
            } else {
                // Opening a code block; remember the language tag.
                in_code_block = true;
                code_fence_lang = trimmed.strip_prefix("```").unwrap_or("").to_string();
            }
        }

        // +1 for the newline character we add when joining.
        let addition = if current.is_empty() {
            line.len()
        } else {
            line.len() + 1
        };

        if !current.is_empty() && current.len() + addition > max_len {
            // Need to split here.
            if in_code_block {
                // Close the code block in the current part before splitting.
                // But we set in_code_block=true above for the *opening* line,
                // so the line that triggered this was already an interior line
                // of the block — close it.
                // However, we need to check: did we *just* open the block on
                // this line? If so, it is not yet in `current`, so don't close.
                // Actually, we haven't pushed `line` yet, so `current` is in
                // a code block that was opened earlier.
                current.push_str("\n```");
            }
            parts.push(current);
            current = String::new();
            if in_code_block {
                // Re-open the code block in the new part.
                current.push_str(&format!("```{}\n", code_fence_lang));
            }
        }

        // If a single line exceeds max_len, hard-split it.
        if line.len() > max_len {
            let mut remaining = line;
            while !remaining.is_empty() {
                let take = remaining.len().min(max_len.saturating_sub(current.len().min(max_len - 1) + 1));
                let take = if take == 0 {
                    // current is already near max_len, flush it first.
                    if !current.is_empty() {
                        if in_code_block {
                            current.push_str("\n```");
                        }
                        parts.push(current);
                        current = String::new();
                        if in_code_block {
                            current.push_str(&format!("```{}\n", code_fence_lang));
                        }
                    }
                    remaining.len().min(max_len)
                } else {
                    take
                };
                let (chunk, rest) = remaining.split_at(take);
                if !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(chunk);
                remaining = rest;

                if current.len() >= max_len && !remaining.is_empty() {
                    if in_code_block {
                        current.push_str("\n```");
                    }
                    parts.push(current);
                    current = String::new();
                    if in_code_block {
                        current.push_str(&format!("```{}\n", code_fence_lang));
                    }
                }
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }

    // Add part indicators if there are multiple parts.
    let total = parts.len();
    if total > 1 {
        parts = parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| format!("({}/{}) {}", i + 1, total, p))
            .collect();
    }

    parts
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        let parts = split_message(&message.content, TELEGRAM_MAX_LEN);
        for part in &parts {
            let body = serde_json::json!({
                "chat_id": message.recipient,
                "text": part,
            });
            let resp = self
                .client
                .post(self.api_url("sendMessage"))
                .json(&body)
                .send()
                .await
                .context("Telegram sendMessage request failed")?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("Telegram sendMessage returned {}: {}", status, text);
            }
        }
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()> {
        // Register bot commands with Telegram on startup.
        if let Err(e) = self.register_commands().await {
            tracing::warn!("Failed to register Telegram bot commands: {e}");
        }

        let mut offset: i64 = 0;

        loop {
            let url = format!(
                "{}?timeout=30&offset={}",
                self.api_url("getUpdates"),
                offset
            );
            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .context("Telegram getUpdates request failed")?;

            let body: Value = resp.json().await.context("Telegram getUpdates parse failed")?;
            let updates = Self::parse_updates(&body);

            for (update_id, sender_id, chat_id, text) in updates {
                // Track offset: next poll starts after the highest update_id.
                if update_id >= offset {
                    offset = update_id + 1;
                }

                if !self.allows_sender(&sender_id) {
                    tracing::debug!("Telegram: ignoring message from disallowed sender {}", sender_id);
                    continue;
                }

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                // Handle /new and /reset commands as session reset signals.
                let mut metadata = HashMap::new();
                let content = if text.starts_with("/new") || text.starts_with("/reset") {
                    metadata.insert("command".to_string(), "reset".to_string());
                    text.clone()
                } else {
                    text
                };

                let msg = InboundMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    sender: sender_id,
                    content,
                    channel: "telegram".to_string(),
                    chat_id,
                    timestamp: now,
                    reply_to: None,
                    metadata,
                };

                if tx.send(msg).await.is_err() {
                    tracing::info!("Telegram: inbound channel closed, stopping listener");
                    return Ok(());
                }
            }
        }
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn send_streaming_start(&self, chat_id: &str) -> Result<Option<String>> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": "...",
        });
        let resp = self
            .client
            .post(self.api_url("sendMessage"))
            .json(&body)
            .send()
            .await
            .context("Telegram streaming start sendMessage failed")?;
        let data: Value = resp.json().await.context("Telegram streaming start parse failed")?;
        let message_id = data
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string());
        Ok(message_id)
    }

    async fn send_streaming_delta(
        &self,
        chat_id: &str,
        message_id: &str,
        full_text: &str,
    ) -> Result<()> {
        // Rate-limit: skip if last edit for this chat was <300ms ago.
        {
            let map = self.last_edit.lock();
            if let Some(last) = map.get(chat_id) {
                if last.elapsed().as_millis() < 300 {
                    return Ok(());
                }
            }
        }

        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": full_text,
        });
        self.client
            .post(self.api_url("editMessageText"))
            .json(&body)
            .send()
            .await
            .context("Telegram editMessageText delta failed")?;

        {
            let mut map = self.last_edit.lock();
            map.insert(chat_id.to_string(), Instant::now());
        }

        Ok(())
    }

    async fn send_streaming_end(
        &self,
        chat_id: &str,
        message_id: &str,
        full_text: &str,
    ) -> Result<()> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": full_text,
        });
        self.client
            .post(self.api_url("editMessageText"))
            .json(&body)
            .send()
            .await
            .context("Telegram editMessageText end failed")?;

        // Clear the rate-limit entry for this chat.
        {
            let mut map = self.last_edit.lock();
            map.remove(chat_id);
        }

        Ok(())
    }

    async fn send_typing(&self, chat_id: &str) -> Result<()> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "action": "typing",
        });
        let _ = self
            .client
            .post(self.api_url("sendChatAction"))
            .json(&body)
            .send()
            .await;
        Ok(())
    }

    fn allows_sender(&self, sender_id: &str) -> bool {
        // Empty list or wildcard "*" means allow all.
        if self.allowed_users.is_empty() {
            return true;
        }
        if self.allowed_users.iter().any(|u| u == "*") {
            return true;
        }
        self.allowed_users.iter().any(|u| u == sender_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_message_short() {
        let parts = split_message("hello world", 4096);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], "hello world");
    }

    #[test]
    fn test_split_message_exact_limit() {
        let msg = "a".repeat(4096);
        let parts = split_message(&msg, 4096);
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn test_split_message_long() {
        // Create a message with many lines that exceeds limit.
        let line = "This is a test line that is fairly long.\n";
        let msg = line.repeat(200); // ~8200 chars
        let parts = split_message(&msg, 4096);
        assert!(parts.len() >= 2);
        for part in &parts {
            assert!(part.len() <= 4096 + 20); // allow indicator overhead
        }
        // Check indicators.
        assert!(parts[0].starts_with("(1/"));
    }

    #[test]
    fn test_split_message_code_block_preserved() {
        let mut msg = String::new();
        msg.push_str("Before code\n");
        msg.push_str("```rust\n");
        // Add enough lines to force a split inside the code block.
        for i in 0..100 {
            msg.push_str(&format!("let x{} = {};\n", i, i));
        }
        msg.push_str("```\n");
        msg.push_str("After code\n");

        let parts = split_message(&msg, 500);
        assert!(parts.len() >= 2);
        // Each interior part that continues a code block should re-open it.
        // The second part should contain ``` to re-open.
        assert!(parts[1].contains("```"));
    }

    #[test]
    fn test_split_message_single_huge_line() {
        let msg = "x".repeat(5000);
        let parts = split_message(&msg, 4096);
        assert!(parts.len() >= 2);
    }

    #[test]
    fn test_parse_updates_empty() {
        let body = serde_json::json!({"ok": true, "result": []});
        let updates = TelegramChannel::parse_updates(&body);
        assert!(updates.is_empty());
    }

    #[test]
    fn test_parse_updates_message() {
        let body = serde_json::json!({
            "ok": true,
            "result": [{
                "update_id": 123,
                "message": {
                    "text": "hello",
                    "from": {"id": 456},
                    "chat": {"id": 789}
                }
            }]
        });
        let updates = TelegramChannel::parse_updates(&body);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0], (123, "456".to_string(), "789".to_string(), "hello".to_string()));
    }

    #[test]
    fn test_allows_sender_empty_list() {
        let ch = TelegramChannel::new("token".to_string(), vec![]);
        assert!(ch.allows_sender("anyone"));
    }

    #[test]
    fn test_allows_sender_wildcard() {
        let ch = TelegramChannel::new("token".to_string(), vec!["*".to_string()]);
        assert!(ch.allows_sender("anyone"));
    }

    #[test]
    fn test_allows_sender_restricted() {
        let ch = TelegramChannel::new("token".to_string(), vec!["123".to_string()]);
        assert!(ch.allows_sender("123"));
        assert!(!ch.allows_sender("456"));
    }
}
