use serde_json::json;

use fennec::agent::loop_::{LoopDetector, LoopStatus};

#[test]
fn test_normal_sequence_returns_ok() {
    let mut d = LoopDetector::new(20);
    d.record("read_file", &json!({"path": "/a.txt"}));
    d.record("shell", &json!({"cmd": "ls"}));
    d.record("write_file", &json!({"path": "/b.txt"}));
    assert_eq!(d.check(), LoopStatus::Ok);
}

#[test]
fn test_three_exact_repeats_returns_warning() {
    let mut d = LoopDetector::new(20);
    let args = json!({"path": "/a.txt"});
    d.record("read_file", &args);
    d.record("read_file", &args);
    d.record("read_file", &args);

    match d.check() {
        LoopStatus::Warning(msg) => {
            assert!(msg.contains("read_file"), "message should name the tool: {msg}");
            assert!(msg.contains("3"), "message should mention count: {msg}");
        }
        other => panic!("expected Warning, got {other:?}"),
    }
}

#[test]
fn test_five_exact_repeats_returns_break() {
    let mut d = LoopDetector::new(20);
    let args = json!({"path": "/a.txt"});
    for _ in 0..5 {
        d.record("read_file", &args);
    }

    match d.check() {
        LoopStatus::Break(msg) => {
            assert!(msg.contains("read_file"), "message should name the tool: {msg}");
            assert!(msg.contains("5"), "message should mention count: {msg}");
        }
        other => panic!("expected Break, got {other:?}"),
    }
}

#[test]
fn test_ping_pong_detected() {
    let mut d = LoopDetector::new(20);
    for i in 0..8 {
        if i % 2 == 0 {
            d.record("read_file", &json!({"i": i}));
        } else {
            d.record("write_file", &json!({"i": i}));
        }
    }

    match d.check() {
        LoopStatus::Break(msg) => {
            assert!(msg.contains("Ping-pong"), "message should say ping-pong: {msg}");
        }
        other => panic!("expected Break for ping-pong, got {other:?}"),
    }
}

#[test]
fn test_same_tool_different_args_flagged() {
    let mut d = LoopDetector::new(20);
    for i in 0..5 {
        d.record("shell", &json!({"cmd": format!("ls {i}")}));
    }

    match d.check() {
        LoopStatus::Warning(msg) => {
            assert!(msg.contains("No-progress"), "message should say no-progress: {msg}");
            assert!(msg.contains("shell"), "message should name the tool: {msg}");
        }
        other => panic!("expected Warning for no-progress, got {other:?}"),
    }
}
