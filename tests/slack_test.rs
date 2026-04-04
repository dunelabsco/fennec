use fennec::channels::{Channel, SlackChannel};
use fennec::channels::slack::SlackChannel as SlackDirect;

#[test]
fn parse_event_envelope_valid_message() {
    let payload = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-abc-123",
        "payload": {
            "event": {
                "type": "message",
                "user": "U12345",
                "text": "Hello Fennec!",
                "channel": "C67890",
                "ts": "1700000000.000100"
            }
        }
    });

    let result = SlackDirect::parse_event_envelope(&payload);
    assert!(result.is_some());
    let (envelope_id, user, text, channel) = result.unwrap();
    assert_eq!(envelope_id, "env-abc-123");
    assert_eq!(user, "U12345");
    assert_eq!(text, "Hello Fennec!");
    assert_eq!(channel, "C67890");
}

#[test]
fn parse_event_envelope_skips_bot_messages() {
    let payload = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-bot-1",
        "payload": {
            "event": {
                "type": "message",
                "bot_id": "B999",
                "text": "I am a bot",
                "channel": "C111"
            }
        }
    });

    assert!(SlackDirect::parse_event_envelope(&payload).is_none());
}

#[test]
fn parse_event_envelope_skips_subtypes() {
    let payload = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-sub-1",
        "payload": {
            "event": {
                "type": "message",
                "subtype": "message_changed",
                "user": "U111",
                "text": "edited text",
                "channel": "C111"
            }
        }
    });

    assert!(SlackDirect::parse_event_envelope(&payload).is_none());
}

#[test]
fn parse_event_envelope_non_events_api_returns_none() {
    let payload = serde_json::json!({
        "type": "hello",
        "num_connections": 1
    });

    assert!(SlackDirect::parse_event_envelope(&payload).is_none());
}

#[test]
fn parse_event_envelope_non_message_event_returns_none() {
    let payload = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-123",
        "payload": {
            "event": {
                "type": "reaction_added",
                "user": "U111",
                "reaction": "thumbsup"
            }
        }
    });

    assert!(SlackDirect::parse_event_envelope(&payload).is_none());
}

#[test]
fn ack_envelope_format() {
    let ack = SlackDirect::ack_envelope("env-abc-123");
    let parsed: serde_json::Value = serde_json::from_str(&ack).unwrap();
    assert_eq!(parsed.get("envelope_id").unwrap().as_str().unwrap(), "env-abc-123");
}

#[test]
fn allows_sender_empty_list_permits_all() {
    let ch = SlackChannel::new("bot-tok".into(), "app-tok".into(), vec![]);
    assert!(ch.allows_sender("anyone"));
}

#[test]
fn allows_sender_wildcard() {
    let ch = SlackChannel::new("bot-tok".into(), "app-tok".into(), vec!["*".into()]);
    assert!(ch.allows_sender("U12345"));
}

#[test]
fn allows_sender_specific_ids() {
    let ch = SlackChannel::new(
        "bot-tok".into(),
        "app-tok".into(),
        vec!["U111".into(), "U222".into()],
    );
    assert!(ch.allows_sender("U111"));
    assert!(ch.allows_sender("U222"));
    assert!(!ch.allows_sender("U333"));
}

#[test]
fn supports_streaming() {
    let ch = SlackChannel::new("bot-tok".into(), "app-tok".into(), vec![]);
    assert!(ch.supports_streaming());
}
