use std::sync::LazyLock;

use regex::Regex;

use crate::memory::experience::{Attempt, Experience, ExperienceContext};

/// Ordered list of `(pattern, replacement)` pairs applied to every text
/// field before an experience leaves the local process.
///
/// Order matters because earlier replacements may eliminate text that a
/// later (broader) pattern would also match — we want the specific
/// labels (e.g. `[REDACTED_AWS_KEY]`) to win over generic ones (e.g.
/// `[REDACTED_SECRET]`).
///
/// The set was expanded in the T3-A audit pass: AWS access keys, GCP
/// API keys, JWTs, PEM private-key blocks, and bare email addresses
/// can all show up in tool output or shell traces and leak via the
/// collective publish path if not redacted here.
static SCRUB_PATTERNS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    vec![
        // -- Provider API keys --
        (
            Regex::new(r"sk-[a-zA-Z0-9_-]{20,}").unwrap(),
            "[REDACTED_API_KEY]",
        ),
        (
            Regex::new(r"sk_(?:live|test)_[a-zA-Z0-9]{20,}").unwrap(),
            "[REDACTED_STRIPE_KEY]",
        ),
        // GitHub PATs (classic `ghp_`, server `ghs_`, user `ghu_`, OAuth
        // `gho_`, and fine-grained `github_pat_`).
        (
            Regex::new(r"gh[pousr]_[a-zA-Z0-9]{20,}").unwrap(),
            "[REDACTED_GITHUB_TOKEN]",
        ),
        (
            Regex::new(r"github_pat_[a-zA-Z0-9_]{20,}").unwrap(),
            "[REDACTED_GITHUB_TOKEN]",
        ),
        (
            Regex::new(r"plrm_live_\S+").unwrap(),
            "[REDACTED_PLURUM_KEY]",
        ),
        // Slack: bot/user/app/refresh/service (`xoxb`, `xoxp`, `xoxa`,
        // `xoxr`, `xoxs`) plus Slack app-level tokens (`xapp-`).
        (
            Regex::new(r"xox[bpars]-[a-zA-Z0-9-]+").unwrap(),
            "[REDACTED_SLACK_TOKEN]",
        ),
        (
            Regex::new(r"xapp-[a-zA-Z0-9-]+").unwrap(),
            "[REDACTED_SLACK_TOKEN]",
        ),

        // -- Cloud credentials --
        // AWS access key ID. Well-known format: `AKIA` + 16 upper-case alnum.
        (
            Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap(),
            "[REDACTED_AWS_KEY]",
        ),
        // Google API key. Well-known format: `AIza` + 35 alnum/underscore/dash.
        (
            Regex::new(r"\bAIza[0-9A-Za-z_-]{35}\b").unwrap(),
            "[REDACTED_GCP_KEY]",
        ),

        // -- JWT tokens --
        // Conservative: require the base64url-encoded `{"alg":...}` header
        // (which decodes to something starting with `{`), which always
        // base64-encodes with the prefix `eyJ`. Catches access_tokens /
        // id_tokens / OIDC assertions with minimal false positives.
        (
            Regex::new(r"\beyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+\b").unwrap(),
            "[REDACTED_JWT]",
        ),

        // -- PEM-armored private keys --
        // Covers RSA / EC / PGP / generic `PRIVATE KEY` variants. The `(?s)`
        // flag makes `.` match newlines so the whole block (BEGIN..END)
        // collapses to the placeholder rather than leaving a dangling half.
        (
            Regex::new(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----")
                .unwrap(),
            "[REDACTED_PRIVATE_KEY_BLOCK]",
        ),

        // -- Bearer tokens (generic) --
        (
            Regex::new(r"(?i)Bearer\s+\S{20,}").unwrap(),
            "Bearer [REDACTED]",
        ),

        // -- Email addresses --
        // RFC 5321 local-parts are more permissive than this, but a
        // tighter pattern avoids false positives on code like
        // `foo@bar.baz` used as a module path in some ecosystems.
        (
            Regex::new(r"\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}\b").unwrap(),
            "[REDACTED_EMAIL]",
        ),

        // -- User paths --
        (
            Regex::new(r"/Users/[a-zA-Z0-9._-]+/").unwrap(),
            "/Users/[REDACTED]/",
        ),
        (
            Regex::new(r"/home/[a-zA-Z0-9._-]+/").unwrap(),
            "/home/[REDACTED]/",
        ),
        (
            Regex::new(r"C:\\Users\\[a-zA-Z0-9._-]+\\").unwrap(),
            "C:\\Users\\[REDACTED]\\",
        ),

        // -- Network-y identifiers --
        // IP addresses (word boundaries to avoid matching version numbers).
        (
            Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap(),
            "[REDACTED_IP]",
        ),
        // Internal hostnames.
        (
            Regex::new(r"\b\w+\.(local|internal)\b").unwrap(),
            "[REDACTED_HOST]",
        ),
        // Database URLs (connection strings almost always include creds).
        (
            Regex::new(r"(?i)(postgres|mysql|mongodb|redis)://\S+").unwrap(),
            "[REDACTED_DB_URL]",
        ),

        // -- Generic key=value catch-all (lowest priority) --
        // Only applied AFTER the specific patterns above. The value
        // matcher excludes `[` as the leading char so already-redacted
        // placeholders (`[REDACTED_GITHUB_TOKEN]`, `[REDACTED_JWT]`, …)
        // are left intact instead of being clobbered into a generic
        // `[REDACTED_SECRET]`.
        (
            Regex::new(r"(?i)(password|passwd|secret|token|api_key)\s*[=:]\s*[^\[\s]\S*").unwrap(),
            "[REDACTED_SECRET]",
        ),
    ]
});

/// Apply all scrubbing patterns to a single string.
pub fn scrub_text(text: &str) -> String {
    let mut result = text.to_string();
    for (pattern, replacement) in SCRUB_PATTERNS.iter() {
        result = pattern.replace_all(&result, *replacement).into_owned();
    }
    result
}

/// Scrub all text fields in an experience, returning a cleaned clone.
pub fn scrub_experience(experience: &Experience) -> Experience {
    let context = ExperienceContext {
        tools_used: experience.context.tools_used.clone(),
        environment: scrub_text(&experience.context.environment),
        constraints: scrub_text(&experience.context.constraints),
    };

    let attempts = experience
        .attempts
        .iter()
        .map(|a| Attempt {
            action: scrub_text(&a.action),
            outcome: scrub_text(&a.outcome),
            dead_end: a.dead_end,
            insight: scrub_text(&a.insight),
        })
        .collect();

    Experience {
        id: experience.id.clone(),
        goal: scrub_text(&experience.goal),
        context,
        attempts,
        solution: experience.solution.as_ref().map(|s| scrub_text(s)),
        gotchas: experience.gotchas.iter().map(|g| scrub_text(g)).collect(),
        tags: experience.tags.clone(),
        confidence: experience.confidence,
        session_id: experience.session_id.clone(),
        created_at: experience.created_at.clone(),
    }
}

/// Check whether an experience is clean (no patterns match any text field).
///
/// This is a safety double-check — after scrubbing, `is_clean` should return
/// `true`. If it does not, something was missed.
pub fn is_clean(experience: &Experience) -> bool {
    let fields = collect_text_fields(experience);
    for field in &fields {
        for (pattern, _) in SCRUB_PATTERNS.iter() {
            if pattern.is_match(field) {
                return false;
            }
        }
    }
    true
}

/// Collect all scrubbable text fields from an experience into a flat list.
fn collect_text_fields(experience: &Experience) -> Vec<&str> {
    let mut fields: Vec<&str> = Vec::new();
    fields.push(&experience.goal);
    fields.push(&experience.context.environment);
    fields.push(&experience.context.constraints);
    for attempt in &experience.attempts {
        fields.push(&attempt.action);
        fields.push(&attempt.outcome);
        fields.push(&attempt.insight);
    }
    if let Some(ref sol) = experience.solution {
        fields.push(sol);
    }
    for gotcha in &experience.gotchas {
        fields.push(gotcha);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_openai_style_key() {
        let out = scrub_text("my key is sk-ant-abc123DEF456ghi789JKL");
        assert!(out.contains("[REDACTED_API_KEY]"));
        assert!(!out.contains("sk-ant-abc123DEF456ghi789JKL"));
    }

    #[test]
    fn scrubs_stripe_live_and_test_keys() {
        let out = scrub_text("stripe sk_live_abcdef1234567890ABCDEF and sk_test_1234567890abcdefABCDEF");
        assert!(out.contains("[REDACTED_STRIPE_KEY]"));
        assert!(!out.contains("sk_live_abcdef"));
        assert!(!out.contains("sk_test_"));
    }

    #[test]
    fn scrubs_github_token_variants() {
        for prefix in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
            let token = format!("{}abcdef1234567890abcd", prefix);
            let out = scrub_text(&format!("token={}", token));
            assert!(
                out.contains("[REDACTED_GITHUB_TOKEN]"),
                "{} not scrubbed: {}",
                prefix,
                out
            );
        }
    }

    #[test]
    fn scrubs_github_fine_grained_pat() {
        let out = scrub_text("github_pat_11AAAAAAA0abcdef1234567890_abcdef");
        assert!(out.contains("[REDACTED_GITHUB_TOKEN]"), "got: {}", out);
    }

    #[test]
    fn scrubs_slack_bot_and_app_tokens() {
        let cases = [
            "xoxb-1234567890-abcdefgh",
            "xoxp-1234567890-abcdefgh",
            "xapp-1-ABCDEF-1234-abcdef",
        ];
        for tok in cases {
            let out = scrub_text(&format!("slack={}", tok));
            assert!(
                out.contains("[REDACTED_SLACK_TOKEN]"),
                "not scrubbed: {} → {}",
                tok,
                out
            );
        }
    }

    /// Regression for T3-A: AWS access key IDs used to pass through
    /// scrub untouched. `AKIA` + 16 upper-alnum is an unambiguous
    /// signature.
    #[test]
    fn scrubs_aws_access_key_id() {
        let out = scrub_text("aws_access_key_id=AKIAIOSFODNN7EXAMPLE");
        assert!(out.contains("[REDACTED_AWS_KEY]"), "got: {}", out);
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    /// Regression for T3-A: GCP API keys. Real GCP keys are 39 chars
    /// total (4-char `AIza` prefix + 35-char body).
    #[test]
    fn scrubs_gcp_api_key() {
        let out = scrub_text("AIzaSyA-1234567890abcdefghijklmnopqrstu");
        assert!(out.contains("[REDACTED_GCP_KEY]"), "got: {}", out);
    }

    /// Regression for T3-A: JWT tokens. The `eyJ` prefix is the
    /// base64 encoding of `{"a`, which every JWT header begins with.
    #[test]
    fn scrubs_jwt_tokens() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let out = scrub_text(&format!("authorization: {}", jwt));
        assert!(out.contains("[REDACTED_JWT]"), "got: {}", out);
        assert!(!out.contains("SflKxwRJ"));
    }

    /// Regression for T3-A: PEM-armored private key blocks. Must
    /// collapse the ENTIRE block including newlines, not just the
    /// first line.
    #[test]
    fn scrubs_pem_private_key_block() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\n\
                   MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDR1234fakekey\n\
                   MoreLinesOfBase64Data\n\
                   -----END RSA PRIVATE KEY-----";
        let out = scrub_text(&format!("my key:\n{}\ntrailing", pem));
        assert!(out.contains("[REDACTED_PRIVATE_KEY_BLOCK]"), "got: {}", out);
        assert!(!out.contains("MIIEvQ"));
        assert!(!out.contains("-----END RSA PRIVATE KEY-----"));
        // Surrounding context must survive.
        assert!(out.contains("my key:"));
        assert!(out.contains("trailing"));
    }

    #[test]
    fn scrubs_generic_pem_private_key_type() {
        // Non-RSA variants: `EC PRIVATE KEY`, `PRIVATE KEY` (PKCS#8),
        // `OPENSSH PRIVATE KEY`.
        for kind in ["EC PRIVATE KEY", "PRIVATE KEY", "OPENSSH PRIVATE KEY"] {
            let pem = format!(
                "-----BEGIN {}-----\nbase64data\n-----END {}-----",
                kind, kind
            );
            let out = scrub_text(&pem);
            assert!(
                out.contains("[REDACTED_PRIVATE_KEY_BLOCK]"),
                "kind {} not scrubbed: {}",
                kind,
                out
            );
        }
    }

    /// Regression for T3-A: bare email addresses. Common in shell logs,
    /// git config, bounced-mail tracebacks.
    #[test]
    fn scrubs_email_address() {
        let out = scrub_text("contact alice.smith+work@example.co.uk tomorrow");
        assert!(out.contains("[REDACTED_EMAIL]"), "got: {}", out);
        assert!(!out.contains("alice.smith"));
    }

    #[test]
    fn scrubs_bearer_token_min_length() {
        let out = scrub_text("Authorization: Bearer abcdef1234567890abcdef");
        assert!(out.contains("Bearer [REDACTED]"));
        // Short bearers (e.g. random `Bearer xyz`) are intentionally not
        // matched — 20-char minimum avoids false positives.
        let out2 = scrub_text("Bearer ok");
        assert_eq!(out2, "Bearer ok");
    }

    #[test]
    fn scrubs_home_and_users_paths() {
        let out = scrub_text("/Users/alice/.fennec/");
        assert!(out.contains("/Users/[REDACTED]/"));
        assert!(!out.contains("alice"));
        let out = scrub_text("/home/bob/code");
        assert!(out.contains("/home/[REDACTED]/"));
    }

    #[test]
    fn scrubs_ip_and_internal_hostname() {
        let out = scrub_text("connect to 10.20.30.40 then admin.local");
        assert!(out.contains("[REDACTED_IP]"));
        assert!(out.contains("[REDACTED_HOST]"));
    }

    #[test]
    fn scrubs_database_urls() {
        let out = scrub_text("postgres://user:pass@db.example.com:5432/mydb");
        assert!(out.contains("[REDACTED_DB_URL]"));
    }

    #[test]
    fn scrubs_generic_secret_kv() {
        let out = scrub_text("password=hunter2_secret");
        assert!(out.contains("[REDACTED_SECRET]"));
    }

    /// Benign text must pass through unchanged — no false positives on
    /// typical agent output.
    #[test]
    fn preserves_benign_text() {
        let input = "The user asked about Rust error handling. I explained Result and ? operator.";
        assert_eq!(scrub_text(input), input);
    }

    /// is_clean must return false when ANY of the new patterns matches —
    /// confirms the audit post-scrub check catches what scrub_text fixes.
    #[test]
    fn is_clean_detects_new_patterns() {
        use crate::memory::experience::{Experience, ExperienceContext};
        let base = Experience {
            id: "x".to_string(),
            goal: "AKIAIOSFODNN7EXAMPLE in the notes".to_string(),
            context: ExperienceContext::default(),
            attempts: vec![],
            solution: None,
            gotchas: vec![],
            tags: vec![],
            confidence: 1.0,
            session_id: None,
            created_at: "ts".to_string(),
        };
        assert!(!is_clean(&base), "AWS key must not be considered clean");
        let cleaned = scrub_experience(&base);
        assert!(
            is_clean(&cleaned),
            "scrubbed experience must pass is_clean, got: {:?}",
            cleaned.goal
        );
    }
}
