use anyhow::Result;
use async_trait::async_trait;

/// A message received from a channel.
#[derive(Debug, Clone)]
pub struct ChannelMessage {
    pub id: String,
    pub sender: String,
    pub content: String,
    pub channel: String,
    pub timestamp: u64,
}

/// A message to send through a channel.
#[derive(Debug, Clone)]
pub struct SendMessage {
    pub content: String,
    pub recipient: String,
}

impl SendMessage {
    pub fn new(content: impl Into<String>, recipient: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
        }
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
    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> Result<()>;
}
