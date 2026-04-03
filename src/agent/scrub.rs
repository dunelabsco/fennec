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
    generic_kv: Regex::new(
        r#"(?i)(token|api[_\-]?key|password|passwd|secret|bearer|authorization|credential)\s*[=:]\s*["']?\S{8,}["']?"#,
    )
    .expect("generic_kv pattern"),
    bearer: Regex::new(
        r"(?i)(Authorization:\s*Bearer|Bearer)\s+\S{8,}",
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

    // Bearer token pattern (e.g. `Authorization: Bearer <token>`).
    result = PATTERNS
        .bearer
        .replace_all(&result, "$1 [REDACTED]")
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
}
