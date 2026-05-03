use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::bus::InboundMessage;

/// Backward-compatible alias: `ChannelMessage` is now [`InboundMessage`].
pub type ChannelMessage = InboundMessage;

/// A message to send through a channel.
///
/// `reply_to` and `metadata` are optional addenda — channels that
/// don't support replies/threads simply ignore them. Populated by
/// `ChannelManager::dispatch_loop` from the originating
/// `OutboundMessage` so reply targets and per-channel hints
/// (e.g. Matrix `thread_id`) reach the channel implementation.
#[derive(Debug, Clone, Default)]
pub struct SendMessage {
    pub content: String,
    pub recipient: String,
    pub reply_to: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl SendMessage {
    pub fn new(content: impl Into<String>, recipient: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
            reply_to: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_reply_to(mut self, reply_to: Option<String>) -> Self {
        self.reply_to = reply_to;
        self
    }

    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Async trait for communication channels (CLI, Slack, Discord, etc.).
#[async_trait]
pub trait Channel: Send + Sync {
    /// Human-readable name of this channel.
    fn name(&self) -> &str;

    /// Send a message through this channel.
    async fn send(&self, message: &SendMessage) -> Result<()>;

    /// Listen for incoming messages, forwarding them through `tx`.
    async fn listen(&self, tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()>;

    /// Whether this channel supports streaming responses.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Begin a streaming response, returning an optional message ID handle.
    async fn send_streaming_start(&self, _chat_id: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// Update a streaming response in-place with the full accumulated text.
    async fn send_streaming_delta(
        &self,
        _chat_id: &str,
        _message_id: &str,
        _full_text: &str,
    ) -> Result<()> {
        Ok(())
    }

    /// Finalize a streaming response.
    async fn send_streaming_end(
        &self,
        _chat_id: &str,
        _message_id: &str,
        _full_text: &str,
    ) -> Result<()> {
        Ok(())
    }

    /// Check whether `sender_id` is permitted to interact with this channel.
    fn allows_sender(&self, _sender_id: &str) -> bool {
        true
    }

    /// Send a typing indicator (e.g. "typing..." bubble in Telegram/Discord).
    async fn send_typing(&self, _chat_id: &str) -> Result<()> {
        Ok(())
    }
}
