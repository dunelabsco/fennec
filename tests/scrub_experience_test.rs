use fennec::collective::scrub::{is_clean, scrub_experience, scrub_text};
use fennec::memory::experience::{Attempt, Experience, ExperienceContext};

fn make_experience(goal: &str, solution: Option<&str>) -> Experience {
    Experience {
        id: "test-exp".to_string(),
        goal: goal.to_string(),
        context: ExperienceContext {
            tools_used: vec!["cargo".to_string()],
            environment: "linux".to_string(),
            constraints: "none".to_string(),
        },
        attempts: vec![],
        solution: solution.map(|s| s.to_string()),
        gotchas: vec![],
        tags: vec!["rust".to_string()],
        confidence: 0.8,
        session_id: None,
        created_at: "2026-04-03T00:00:00Z".to_string(),
    }
}

// ---------------------------------------------------------------------------
// scrub_text tests
// ---------------------------------------------------------------------------

#[test]
fn scrub_openai_api_key() {
    let text = "Used key sk-abcdefghijklmnopqrstuvwxyz1234 to call API";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("sk-abcdefghijklmnopqrstuvwxyz1234"));
    assert!(scrubbed.contains("[REDACTED_API_KEY]"));
}

#[test]
fn scrub_github_token() {
    let text = "Clone with ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ12 token";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ12"));
    assert!(scrubbed.contains("[REDACTED_GITHUB_TOKEN]"));
}

#[test]
fn scrub_plurum_key() {
    let text = "Set plrm_live_abc123def456 in env";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("plrm_live_abc123def456"));
    assert!(scrubbed.contains("[REDACTED_PLURUM_KEY]"));
}

#[test]
fn scrub_slack_token() {
    let text = "Slack token xoxb-123456789-abcdefghij";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("xoxb-123456789-abcdefghij"));
    assert!(scrubbed.contains("[REDACTED_SLACK_TOKEN]"));
}

#[test]
fn scrub_bearer_token() {
    let text = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("eyJhbGciOiJIUzI1NiI"));
    assert!(scrubbed.contains("Bearer [REDACTED]"));
}

#[test]
fn scrub_unix_user_path() {
    let text = "File at /Users/johndoe/projects/fennec/src/main.rs";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("johndoe"));
    assert!(scrubbed.contains("/Users/[REDACTED]/"));
}

#[test]
fn scrub_linux_home_path() {
    let text = "Config in /home/alice/.config/fennec.toml";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("alice"));
    assert!(scrubbed.contains("/home/[REDACTED]/"));
}

#[test]
fn scrub_windows_user_path() {
    let text = "Path: C:\\Users\\bob\\Documents\\file.txt";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("bob"));
    assert!(scrubbed.contains("C:\\Users\\[REDACTED]\\"));
}

#[test]
fn scrub_ip_address() {
    let text = "Connected to server at 192.168.1.100 on port 8080";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("192.168.1.100"));
    assert!(scrubbed.contains("[REDACTED_IP]"));
}

#[test]
fn scrub_internal_hostname() {
    let text = "Resolved myserver.local to an address";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("myserver.local"));
    assert!(scrubbed.contains("[REDACTED_HOST]"));
}

#[test]
fn scrub_internal_hostname_with_internal_suffix() {
    let text = "API at backend.internal responded";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("backend.internal"));
    assert!(scrubbed.contains("[REDACTED_HOST]"));
}

#[test]
fn scrub_postgres_url() {
    let text = "Database: postgres://user:pass@host:5432/mydb";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("postgres://user:pass@host:5432/mydb"));
    assert!(scrubbed.contains("[REDACTED_DB_URL]"));
}

#[test]
fn scrub_mysql_url() {
    let text = "Connection: mysql://root:secret@localhost/app";
    let scrubbed = scrub_text(text);
    assert!(scrubbed.contains("[REDACTED_DB_URL]"));
}

#[test]
fn scrub_redis_url() {
    let text = "Cache at redis://default:pass@cache:6379";
    let scrubbed = scrub_text(text);
    assert!(scrubbed.contains("[REDACTED_DB_URL]"));
}

#[test]
fn scrub_generic_password_kv() {
    let text = "Set password=mysupersecret in config";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("mysupersecret"));
    assert!(scrubbed.contains("[REDACTED_SECRET]"));
}

#[test]
fn scrub_generic_api_key_kv() {
    let text = "api_key: some_long_value_here";
    let scrubbed = scrub_text(text);
    assert!(!scrubbed.contains("some_long_value_here"));
    assert!(scrubbed.contains("[REDACTED_SECRET]"));
}

#[test]
fn normal_text_unchanged() {
    let text = "Implemented a Rust function that processes 192 items using a HashMap";
    let scrubbed = scrub_text(text);
    // "192 items" should NOT be matched as an IP — it's not four octets.
    assert_eq!(scrubbed, text);
}

#[test]
fn normal_technical_text_preserved() {
    let text = "The function returns Vec<String> with capacity 256. Processed in 42ms.";
    let scrubbed = scrub_text(text);
    assert_eq!(scrubbed, text);
}

#[test]
fn version_numbers_not_matched_as_ip() {
    // "1.2.3" is only three parts, should not match the IP regex.
    let text = "Using version 1.2.3 of the library";
    let scrubbed = scrub_text(text);
    assert_eq!(scrubbed, text);
}

// ---------------------------------------------------------------------------
// scrub_experience tests
// ---------------------------------------------------------------------------

#[test]
fn scrub_experience_cleans_goal() {
    let exp = make_experience(
        "Fix bug using sk-abcdefghijklmnopqrstuvwxyz1234 key",
        None,
    );
    let scrubbed = scrub_experience(&exp);
    assert!(scrubbed.goal.contains("[REDACTED_API_KEY]"));
    assert!(!scrubbed.goal.contains("sk-abcdefghijklmnopqrstuvwxyz1234"));
}

#[test]
fn scrub_experience_cleans_solution() {
    let exp = make_experience(
        "Deploy app",
        Some("Connect to postgres://admin:hunter2@db.internal:5432/prod"),
    );
    let scrubbed = scrub_experience(&exp);
    let solution = scrubbed.solution.unwrap();
    assert!(solution.contains("[REDACTED_DB_URL]"));
}

#[test]
fn scrub_experience_cleans_attempts() {
    let mut exp = make_experience("Debug issue", None);
    exp.attempts.push(Attempt {
        action: "Checked /Users/alice/logs/error.log".to_string(),
        outcome: "Found token=supersecretvalue in output".to_string(),
        dead_end: false,
        insight: "Server at 10.0.0.5 was the culprit".to_string(),
    });

    let scrubbed = scrub_experience(&exp);
    let attempt = &scrubbed.attempts[0];
    assert!(attempt.action.contains("/Users/[REDACTED]/"));
    assert!(!attempt.action.contains("alice"));
    assert!(attempt.outcome.contains("[REDACTED_SECRET]"));
    assert!(!attempt.outcome.contains("supersecretvalue"));
    assert!(attempt.insight.contains("[REDACTED_IP]"));
    assert!(!attempt.insight.contains("10.0.0.5"));
}

#[test]
fn scrub_experience_cleans_gotchas() {
    let mut exp = make_experience("Setup env", None);
    exp.gotchas.push("Don't hardcode password=abc123 in config".to_string());

    let scrubbed = scrub_experience(&exp);
    assert!(scrubbed.gotchas[0].contains("[REDACTED_SECRET]"));
    assert!(!scrubbed.gotchas[0].contains("abc123"));
}

#[test]
fn scrub_experience_cleans_context() {
    let mut exp = make_experience("Deploy", None);
    exp.context.environment = "Production at myapp.internal".to_string();
    exp.context.constraints = "Must use secret=xyz789 for auth".to_string();

    let scrubbed = scrub_experience(&exp);
    assert!(scrubbed.context.environment.contains("[REDACTED_HOST]"));
    assert!(scrubbed.context.constraints.contains("[REDACTED_SECRET]"));
}

#[test]
fn scrub_experience_preserves_non_sensitive_fields() {
    let exp = make_experience("Implement HashMap cache", Some("Use dashmap crate"));
    let scrubbed = scrub_experience(&exp);

    assert_eq!(scrubbed.id, exp.id);
    assert_eq!(scrubbed.tags, exp.tags);
    assert_eq!(scrubbed.confidence, exp.confidence);
    assert_eq!(scrubbed.session_id, exp.session_id);
    assert_eq!(scrubbed.created_at, exp.created_at);
}

// ---------------------------------------------------------------------------
// is_clean tests
// ---------------------------------------------------------------------------

#[test]
fn clean_experience_passes_is_clean() {
    let exp = make_experience(
        "Implement a caching layer for database queries",
        Some("Used Redis with TTL of 300 seconds"),
    );
    assert!(is_clean(&exp));
}

#[test]
fn dirty_experience_fails_is_clean() {
    let exp = make_experience(
        "Fix auth using sk-abcdefghijklmnopqrstuvwxyz1234",
        None,
    );
    assert!(!is_clean(&exp));
}

#[test]
fn scrubbed_experience_passes_is_clean() {
    let mut exp = make_experience(
        "Debug server at 10.0.0.5 with ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ12",
        Some("Fixed via postgres://admin:pass@db.internal:5432/prod"),
    );
    exp.attempts.push(Attempt {
        action: "Checked /Users/alice/logs".to_string(),
        outcome: "Found password=hunter2".to_string(),
        dead_end: false,
        insight: "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9xxxxx was expired".to_string(),
    });
    exp.gotchas.push("plrm_live_testkey123 must be rotated".to_string());
    exp.context.environment = "myserver.local staging".to_string();

    // Before scrubbing — dirty.
    assert!(!is_clean(&exp));

    // After scrubbing — clean.
    let scrubbed = scrub_experience(&exp);
    assert!(
        is_clean(&scrubbed),
        "Scrubbed experience should be clean. Goal: {}, Solution: {:?}",
        scrubbed.goal,
        scrubbed.solution
    );
}
