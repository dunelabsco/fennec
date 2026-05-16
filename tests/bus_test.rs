use std::collections::HashMap;

use fennec::bus::{InboundMessage, MessageBus, OutboundMessage};

fn make_inbound(content: &str, sender: &str) -> InboundMessage {
    InboundMessage {
        id: uuid::Uuid::new_v4().to_string(),
        sender: sender.to_string(),
        content: content.to_string(),
        channel: "test".to_string(),
        chat_id: "chat_1".to_string(),
        timestamp: 0,
        reply_to: None,
        metadata: HashMap::new(),
    }
}

fn make_outbound(content: &str) -> OutboundMessage {
    OutboundMessage {
        content: content.to_string(),
        channel: "test".to_string(),
        chat_id: "chat_1".to_string(),
        reply_to: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
    }
}

#[tokio::test]
async fn test_publish_and_receive_inbound() {
    let (bus, mut receiver) = MessageBus::new(16);

    let msg = make_inbound("hello", "alice");
    bus.publish_inbound(msg.clone()).await.unwrap();

    let received = receiver.inbound_rx.recv().await.unwrap();
    assert_eq!(received.content, "hello");
    assert_eq!(received.sender, "alice");
    assert_eq!(received.channel, "test");
    assert_eq!(received.chat_id, "chat_1");
}

#[tokio::test]
async fn test_publish_and_receive_outbound() {
    let (bus, mut receiver) = MessageBus::new(16);

    let msg = make_outbound("reply");
    bus.publish_outbound(msg).await.unwrap();

    let received = receiver.outbound_rx.recv().await.unwrap();
    assert_eq!(received.content, "reply");
    assert_eq!(received.channel, "test");
    assert_eq!(received.chat_id, "chat_1");
}

#[tokio::test]
async fn test_multiple_producers() {
    let (bus, mut receiver) = MessageBus::new(16);

    let bus1 = bus.clone();
    let bus2 = bus.clone();

    let t1 = tokio::spawn(async move {
        bus1.publish_inbound(make_inbound("from_task_1", "alice"))
            .await
            .unwrap();
    });

    let t2 = tokio::spawn(async move {
        bus2.publish_inbound(make_inbound("from_task_2", "bob"))
            .await
            .unwrap();
    });

    t1.await.unwrap();
    t2.await.unwrap();

    // Drop original bus so the channel closes after receiving both messages.
    drop(bus);

    let mut messages = Vec::new();
    while let Some(msg) = receiver.inbound_rx.recv().await {
        messages.push(msg.content);
    }

    assert_eq!(messages.len(), 2);
    assert!(messages.contains(&"from_task_1".to_string()));
    assert!(messages.contains(&"from_task_2".to_string()));
}

#[tokio::test]
async fn test_sender_accessors() {
    let (bus, mut receiver) = MessageBus::new(16);

    let inbound_tx = bus.inbound_sender();
    let outbound_tx = bus.outbound_sender();

    inbound_tx
        .send(make_inbound("via_sender", "carol"))
        .await
        .unwrap();
    outbound_tx.send(make_outbound("via_sender")).await.unwrap();

    let inbound = receiver.inbound_rx.recv().await.unwrap();
    assert_eq!(inbound.content, "via_sender");
    assert_eq!(inbound.sender, "carol");

    let outbound = receiver.outbound_rx.recv().await.unwrap();
    assert_eq!(outbound.content, "via_sender");
}
