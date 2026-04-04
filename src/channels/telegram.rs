use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;

use crate::bus::InboundMessage;

use super::traits::{Channel, SendMessage};

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

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        let body = serde_json::json!({
            "chat_id": message.recipient,
            "text": message.content,
            "parse_mode": "Markdown",
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
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()> {
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

                let msg = InboundMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    sender: sender_id,
                    content: text,
                    channel: "telegram".to_string(),
                    chat_id,
                    timestamp: now,
                    reply_to: None,
                    metadata: HashMap::new(),
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
            "parse_mode": "Markdown",
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
            "parse_mode": "Markdown",
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
            "parse_mode": "Markdown",
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
