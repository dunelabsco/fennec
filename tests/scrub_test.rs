use fennec::agent::scrub::scrub_credentials;

#[test]
fn test_api_keys_are_scrubbed() {
    let text = r#"api_key = "abcdefghij1234567890""#;
    let result = scrub_credentials(text);
    assert!(result.contains("[REDACTED]"), "should contain [REDACTED]: {result}");
    assert!(!result.contains("abcdefghij1234567890"), "should not contain key: {result}");
}

#[test]
fn test_passwords_are_scrubbed() {
    let text = "password: mysupersecretpassword123";
    let result = scrub_credentials(text);
    assert!(result.contains("[REDACTED]"), "should contain [REDACTED]: {result}");
    assert!(!result.contains("mysupersecretpassword123"), "should not contain password: {result}");
}

#[test]
fn test_normal_text_unchanged() {
    let text = "This is a perfectly normal message with no credentials at all.";
    assert_eq!(scrub_credentials(text), text);
}

#[test]
fn test_sk_format_detected() {
    // Standalone sk- key (not preceded by a kv keyword).
    let text = "Found sk-abcdefghijklmnopqrstuvwx in the config";
    let result = scrub_credentials(text);
    assert!(result.contains("sk-[REDACTED]"), "should redact sk- key: {result}");
    assert!(!result.contains("abcdefghijklmnopqrstuvwx"), "should not contain key: {result}");
}

#[test]
fn test_ghp_format_detected() {
    // Standalone ghp_ key (not preceded by a kv keyword).
    let text = "Found ghp_abcdefghijklmnopqrstuvwxyz in the config";
    let result = scrub_credentials(text);
    assert!(result.contains("ghp_[REDACTED]"), "should redact ghp_ key: {result}");
    assert!(!result.contains("abcdefghijklmnopqrstuvwxyz"), "should not contain key: {result}");
}

#[test]
fn test_ghp_with_kv_prefix() {
    // ghp_ key preceded by a kv keyword — both patterns will fire.
    let text = "token=ghp_abcdefghijklmnopqrstuvwxyz";
    let result = scrub_credentials(text);
    assert!(result.contains("[REDACTED]"), "should contain [REDACTED]: {result}");
    assert!(!result.contains("abcdefghijklmnopqrstuvwxyz"), "should not contain key value: {result}");
}

#[test]
fn test_bearer_token_scrubbed() {
    let text = r#"Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"#;
    let result = scrub_credentials(text);
    assert!(result.contains("[REDACTED]"), "should contain [REDACTED]: {result}");
    assert!(!result.contains("eyJhbGciOiJ"), "should not contain token: {result}");
}

#[test]
fn test_plrm_live_token_scrubbed() {
    let text = "key is plrm_live_abc123def456ghi789";
    let result = scrub_credentials(text);
    assert!(result.contains("plrm_live_[REDACTED]"), "should redact plrm_live_ token: {result}");
    assert!(!result.contains("abc123def456ghi789"), "should not contain token: {result}");
}
