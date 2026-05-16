use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;

use fennec::bus::{InboundMessage, MessageBus, OutboundMessage};
use fennec::channels::traits::{Channel, SendMessage};
use fennec::channels::ChannelManager;

// ---------------------------------------------------------------------------
// Mock Channel
// ---------------------------------------------------------------------------

/// A mock channel that records sent messages and can simulate listen behavior.
struct MockChannel {
    channel_name: String,
    sent: Arc<Mutex<Vec<SendMessage>>>,
    /// When listen is called, immediately send one message through the sender
    /// and then return Ok(()).
    auto_message: Option<String>,
}

impl MockChannel {
    fn new(name: &str) -> Self {
        Self {
            channel_name: name.to_string(),
            sent: Arc::new(Mutex::new(Vec::new())),
            auto_message: None,
        }
    }

    fn with_auto_message(mut self, msg: &str) -> Self {
        self.auto_message = Some(msg.to_string());
        self
    }

    fn sent_messages(&self) -> Vec<SendMessage> {
        self.sent.lock().clone()
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn name(&self) -> &str {
        &self.channel_name
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        self.sent.lock().push(message.clone());
        Ok(())
    }

    async fn listen(
        &self,
        tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        if let Some(ref content) = self.auto_message {
            let msg = InboundMessage {
                id: "mock_1".to_string(),
                sender: "mock_user".to_string(),
                content: content.clone(),
                channel: self.channel_name.clone(),
                chat_id: "mock_chat".to_string(),
                timestamp: 0,
                reply_to: None,
                metadata: HashMap::new(),
            };
            let _ = tx.send(msg).await;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_channel_registration_and_lookup() {
    let (bus, _receiver) = MessageBus::new(16);

    let ch1: Arc<dyn Channel> = Arc::new(MockChannel::new("slack"));
    let ch2: Arc<dyn Channel> = Arc::new(MockChannel::new("discord"));

    let manager = ChannelManager::new(vec![ch1, ch2], bus);

    assert!(manager.get_channel("slack").is_some());
    assert!(manager.get_channel("discord").is_some());
    assert!(manager.get_channel("nonexistent").is_none());

    assert_eq!(manager.get_channel("slack").unwrap().name(), "slack");
    assert_eq!(manager.get_channel("discord").unwrap().name(), "discord");
}

#[tokio::test]
async fn test_outbound_dispatch_to_correct_channel() {
    let (bus, receiver) = MessageBus::new(16);

    let slack = Arc::new(MockChannel::new("slack"));
    let discord = Arc::new(MockChannel::new("discord"));

    let slack_ref = Arc::clone(&slack);
    let discord_ref = Arc::clone(&discord);

    let channels: Vec<Arc<dyn Channel>> = vec![
        slack_ref as Arc<dyn Channel>,
        discord_ref as Arc<dyn Channel>,
    ];
    let manager = ChannelManager::new(channels, bus.clone());

    // Send an outbound message destined for "slack".
    let out_msg = OutboundMessage {
        content: "hello slack".to_string(),
        channel: "slack".to_string(),
        chat_id: "ch_123".to_string(),
        reply_to: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
    };
    bus.publish_outbound(out_msg).await.unwrap();

    // Send another destined for "discord".
    let out_msg2 = OutboundMessage {
        content: "hello discord".to_string(),
        channel: "discord".to_string(),
        chat_id: "ch_456".to_string(),
        reply_to: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
    };
    bus.publish_outbound(out_msg2).await.unwrap();

    // Spawn dispatch using the method that doesn't hold a &self reference,
    // so the receiver can close when all external senders are dropped.
    let dispatch_handle = manager.spawn_outbound_dispatch(receiver.outbound_rx);

    // Drop all bus senders: the external clone and the one inside manager.
    drop(bus);
    drop(manager);

    // The dispatch task should finish once all senders are dropped.
    tokio::time::timeout(std::time::Duration::from_secs(2), dispatch_handle)
        .await
        .expect("dispatch should finish after senders dropped")
        .unwrap();

    // Verify slack received its message.
    let slack_sent = slack.sent_messages();
    assert_eq!(slack_sent.len(), 1);
    assert_eq!(slack_sent[0].content, "hello slack");
    assert_eq!(slack_sent[0].recipient, "ch_123");

    // Verify discord received its message.
    let discord_sent = discord.sent_messages();
    assert_eq!(discord_sent.len(), 1);
    assert_eq!(discord_sent[0].content, "hello discord");
    assert_eq!(discord_sent[0].recipient, "ch_456");
}

#[tokio::test]
async fn test_start_all_delivers_inbound() {
    let (bus, mut receiver) = MessageBus::new(16);

    let ch: Arc<dyn Channel> =
        Arc::new(MockChannel::new("test_ch").with_auto_message("hi from mock"));

    let manager = ChannelManager::new(vec![ch], bus);
    let handles = manager.start_all();

    // Wait for the listener to send its auto-message.
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.inbound_rx.recv(),
    )
    .await
    .expect("timed out waiting for inbound message")
    .expect("inbound_rx closed");

    assert_eq!(msg.content, "hi from mock");
    assert_eq!(msg.channel, "test_ch");

    // Wait for listener tasks to finish.
    for h in handles {
        h.await.unwrap();
    }
}

#[tokio::test]
async fn test_outbound_unknown_channel_ignored() {
    let (bus, receiver) = MessageBus::new(16);

    let ch: Arc<dyn Channel> = Arc::new(MockChannel::new("known"));
    let manager = ChannelManager::new(vec![ch], bus.clone());

    // Send to a channel that doesn't exist.
    let out_msg = OutboundMessage {
        content: "nobody home".to_string(),
        channel: "unknown".to_string(),
        chat_id: "ch_999".to_string(),
        reply_to: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
    };
    bus.publish_outbound(out_msg).await.unwrap();

    // Spawn dispatch using the method that owns the receiver independently.
    let dispatch_handle = manager.spawn_outbound_dispatch(receiver.outbound_rx);

    // Drop all senders.
    drop(bus);
    drop(manager);

    // Should complete without panic.
    tokio::time::timeout(std::time::Duration::from_secs(2), dispatch_handle)
        .await
        .expect("dispatch should finish")
        .unwrap();
}
