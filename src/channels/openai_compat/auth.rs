//! Bearer-token authentication for the OpenAI-compat channel.
//!
//! When the channel is configured with a non-empty `api_key`, every
//! request to a `/v1/*` endpoint must include
//! `Authorization: Bearer <api_key>`. Empty `api_key` opens the
//! channel to anything that can reach the listen port — only safe
//! on `host = 127.0.0.1`. A startup warning is logged in that case.
//!
//! Comparison is constant-time so a slow attacker can't probe the
//! key one byte at a time.

use axum::http::HeaderMap;

/// Verify the `Authorization` header against the configured API key.
/// Returns `Ok(())` when the request is allowed, `Err(reason)` when
/// it should be rejected with 401.
pub fn check_bearer(headers: &HeaderMap, expected: &str) -> Result<(), String> {
    if expected.is_empty() {
        // Open mode — accept everything.
        return Ok(());
    }
    let header = headers
        .get("authorization")
        .or_else(|| headers.get("Authorization"))
        .ok_or_else(|| "missing Authorization header".to_string())?
        .to_str()
        .map_err(|_| "Authorization header is not valid UTF-8".to_string())?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or_else(|| "Authorization header missing 'Bearer ' prefix".to_string())?
        .trim();
    if !constant_time_eq(token.as_bytes(), expected.as_bytes()) {
        return Err("invalid bearer token".to_string());
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(name: &str, value: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        m
    }

    #[test]
    fn empty_expected_means_open() {
        assert!(check_bearer(&HeaderMap::new(), "").is_ok());
    }

    #[test]
    fn missing_header_with_expected_set_rejects() {
        assert!(check_bearer(&HeaderMap::new(), "k").is_err());
    }

    #[test]
    fn correct_bearer_passes() {
        let headers = h("authorization", "Bearer secret-key");
        assert!(check_bearer(&headers, "secret-key").is_ok());
    }

    #[test]
    fn wrong_bearer_rejects() {
        let headers = h("authorization", "Bearer wrong-key");
        assert!(check_bearer(&headers, "right-key").is_err());
    }

    #[test]
    fn missing_bearer_prefix_rejects() {
        let headers = h("authorization", "secret-key");
        let r = check_bearer(&headers, "secret-key");
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Bearer"));
    }

    #[test]
    fn lowercase_bearer_prefix_accepted() {
        let headers = h("authorization", "bearer my-key");
        assert!(check_bearer(&headers, "my-key").is_ok());
    }

    #[test]
    fn capitalized_authorization_header_accepted() {
        let headers = h("Authorization", "Bearer my-key");
        assert!(check_bearer(&headers, "my-key").is_ok());
    }

    #[test]
    fn whitespace_around_token_tolerated() {
        // axum normalizes header values; in practice the trim() on
        // our side is defense-in-depth for clients that include
        // trailing newlines.
        let headers = h("authorization", "Bearer my-key  ");
        assert!(check_bearer(&headers, "my-key").is_ok());
    }

    #[test]
    fn length_mismatch_does_not_short_circuit_match() {
        // If lengths differ, constant_time_eq returns false without
        // checking bytes — correct, because matching by length-prefix
        // can't succeed anyway.
        let headers = h("authorization", "Bearer sk-aaaaaaaaaaa");
        assert!(check_bearer(&headers, "sk-different-length").is_err());
    }
}
