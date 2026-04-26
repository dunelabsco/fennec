use std::sync::LazyLock;

use regex::Regex;

/// Compiled credential-matching patterns.
struct CredentialPatterns {
    /// Generic key=value pattern for tokens, passwords, secrets, etc.
    generic_kv: Regex,
    /// Bearer token pattern (e.g. `Authorization: Bearer <token>`).
    bearer: Regex,
    /// OpenAI-style `sk-` keys.
    sk_key: Regex,
    /// GitHub personal access tokens.
    ghp_key: Regex,
    /// Plrm live tokens.
    plrm_key: Regex,
}

static PATTERNS: LazyLock<CredentialPatterns> = LazyLock::new(|| CredentialPatterns {
    // `\b...\b` word boundaries on the keyword group so we don't redact
    // tool output that happens to contain these words as substrings of
    // larger identifiers — e.g. `tokenizer = AutoTokenizer.from_pretrained`
    // (no boundary after "token"), `secret_word = "..."` (no boundary
    // after "secret"), `passwordless_login`. The original pattern fired
    // on all of those and silently gutted legitimate tool output.
    generic_kv: Regex::new(
        r#"(?i)\b(token|api[_\-]?key|password|passwd|secret|bearer|authorization|credential)\b\s*[=:]\s*["']?\S{8,}["']?"#,
    )
    .expect("generic_kv pattern"),
    // Bearer pattern: capture the value but use a base64url-ish charset
    // and ≥20 chars to drop natural-language phrases like
    // `Bearer authentication is...`, `Bearer of bad news...`.
    // A second filter in `scrub_credentials` requires the captured value
    // to contain at least one digit — real bearer tokens are universally
    // base64-encoded and have digits; English words don't.
    bearer: Regex::new(
        r"(?i)(Authorization:\s*Bearer|Bearer)\s+([A-Za-z0-9_\-\.=]{20,})",
    )
    .expect("bearer pattern"),
    sk_key: Regex::new(r"sk-[a-zA-Z0-9]{20,}").expect("sk_key pattern"),
    ghp_key: Regex::new(r"ghp_[a-zA-Z0-9]{20,}").expect("ghp_key pattern"),
    plrm_key: Regex::new(r"plrm_live_\S+").expect("plrm_key pattern"),
});

/// Scrub credential-like values from the given text, replacing them with
/// `[REDACTED]` markers.
pub fn scrub_credentials(text: &str) -> String {
    let mut result = text.to_string();

    // Apply well-known key formats first so they match before the generic
    // pattern can consume them.
    result = PATTERNS
        .sk_key
        .replace_all(&result, "sk-[REDACTED]")
        .into_owned();
    result = PATTERNS
        .ghp_key
        .replace_all(&result, "ghp_[REDACTED]")
        .into_owned();
    result = PATTERNS
        .plrm_key
        .replace_all(&result, "plrm_live_[REDACTED]")
        .into_owned();

    // Bearer token pattern (e.g. `Authorization: Bearer <token>`). The
    // regex has already enforced base64url charset + 20+ chars; here we
    // additionally require at least one digit in the captured value
    // before redacting, so 20+ char base64-shaped tokens (which always
    // have digits) get caught while a 20+ char all-letter run (e.g. an
    // unusually long English word after the bare word "Bearer") is left
    // alone.
    result = PATTERNS
        .bearer
        .replace_all(&result, |caps: &regex::Captures| {
            let prefix = caps.get(1).map_or("Bearer", |m| m.as_str());
            let value = caps.get(2).map_or("", |m| m.as_str());
            if value.chars().any(|c| c.is_ascii_digit()) {
                format!("{} [REDACTED]", prefix)
            } else {
                // No digit → looks like natural language, not a token.
                caps.get(0).unwrap().as_str().to_string()
            }
        })
        .into_owned();

    // Generic key=value patterns. Capture the key name and redact the value.
    result = PATTERNS
        .generic_kv
        .replace_all(&result, "$1=[REDACTED]")
        .into_owned();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_text_unchanged() {
        let text = "Hello, this is a normal message with no secrets.";
        assert_eq!(scrub_credentials(text), text);
    }

    #[test]
    fn test_generic_password() {
        let text = r#"password = "supersecretvalue123""#;
        let scrubbed = scrub_credentials(text);
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("supersecretvalue123"));
    }

    /// `tokenizer = AutoTokenizer.from_pretrained(...)` is legitimate
    /// Python in tool output; "tokenizer" contains "token" but is not a
    /// credential keyword. Old pattern redacted the model name. Word
    /// boundaries fix this.
    #[test]
    fn does_not_redact_tokenizer_in_code() {
        let text = r#"tokenizer = AutoTokenizer.from_pretrained("bert-base-uncased")"#;
        let scrubbed = scrub_credentials(text);
        assert_eq!(scrubbed, text, "should be unchanged");
    }

    /// `secret_word = ...` is a fine variable name in source-code output.
    /// The keyword "secret" appears as a prefix of a longer identifier.
    #[test]
    fn does_not_redact_secret_word_identifier() {
        let text = r#"let secret_word = "hello world""#;
        let scrubbed = scrub_credentials(text);
        assert_eq!(scrubbed, text);
    }

    /// `passwordless_login` appears in tool output describing auth flows;
    /// "password" prefix should not trigger redaction of the rest.
    #[test]
    fn does_not_redact_passwordless_login() {
        let text = r#"const passwordless_login = "magic-link-flow""#;
        let scrubbed = scrub_credentials(text);
        assert_eq!(scrubbed, text);
    }

    /// `Bearer authentication is HTTP's standard scheme...` is a
    /// natural-language phrase that happens to start with "Bearer". The
    /// follow-up word has no digits, so we skip redaction.
    #[test]
    fn does_not_redact_bearer_in_natural_language() {
        let text =
            "Bearer authentication is HTTP authorization that uses opaque tokens";
        let scrubbed = scrub_credentials(text);
        assert_eq!(scrubbed, text);
    }

    /// Real bearer tokens always have digits, so they're still caught.
    #[test]
    fn redacts_real_bearer_token() {
        let text =
            "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.foo.bar";
        let scrubbed = scrub_credentials(text);
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("eyJhbGciOiJIUzI1NiI"));
    }

    /// Bare keyword + value still redacts as before.
    #[test]
    fn still_redacts_bare_password_assignment() {
        let text = "password=hunter2hunter2";
        let scrubbed = scrub_credentials(text);
        assert!(scrubbed.contains("[REDACTED]"));
    }
}
