use std::path::Path;

use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPE: &str = "org:create_api_key user:profile user:inference";
const TOKEN_ENDPOINT: &str = "https://console.anthropic.com/v1/oauth/token";
const AUTH_ENDPOINT: &str = "https://claude.ai/oauth/authorize";
const TOKEN_FILE: &str = ".anthropic_oauth.json";

/// Persisted OAuth credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64, // unix timestamp in seconds
}

/// Generate a cryptographically random PKCE verifier (43-128 chars, base64url).
fn generate_verifier() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill(&mut buf);
    base64url_encode(&buf)
}

/// Compute the S256 code challenge from the verifier.
fn compute_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    base64url_encode(&hash)
}

/// Base64url-encode without padding (RFC 7636).
fn base64url_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    let len = input.len();
    let mut i = 0;
    while i < len {
        let b0 = input[i] as u32;
        let b1 = if i + 1 < len { input[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < len { input[i + 2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        s.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        s.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if i + 1 < len {
            s.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        }
        if i + 2 < len {
            s.push(CHARS[(triple & 0x3F) as usize] as char);
        }
        i += 3;
    }
    // Convert to url-safe alphabet.
    s.replace('+', "-").replace('/', "_")
}

/// Open a URL in the user's default browser.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let cmd = std::process::Command::new("cmd")
        .args(["/C", "start", url])
        .spawn();

    if let Err(e) = cmd {
        tracing::warn!("Failed to open browser: {e}");
    }
}

/// Run the full OAuth PKCE login flow interactively.
///
/// Opens the browser, waits for the user to paste the authorization code,
/// exchanges it for tokens, and saves them to `fennec_home/.anthropic_oauth.json`.
pub fn run_oauth_login(fennec_home: &Path) -> Result<OAuthCredentials> {
    // 1. Generate PKCE pair.
    let verifier = generate_verifier();
    let challenge = compute_challenge(&verifier);

    // 2. Build authorization URL.
    // Build authorization URL — matches Hermes's exact parameter set.
    // The "code=true" param and "state=verifier" are required by Anthropic.
    let auth_url = format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        AUTH_ENDPOINT,
        CLIENT_ID,
        urlencoded(REDIRECT_URI),
        urlencoded(SCOPE),
        challenge,
        urlencoded(&verifier),
    );

    // 3. Show URL and prompt user.
    println!();
    println!("  Open this link in your browser:");
    println!();
    println!("  {}", auth_url);
    println!();
    open_browser(&auth_url);
    println!("After authorizing, you'll see a code. Paste it below.");
    println!();

    print!("Authorization code: ");
    use std::io::Write;
    std::io::stdout().flush().ok();

    let mut raw_input = String::new();
    #[cfg(unix)]
    {
        use std::io::BufRead;
        if let Ok(tty) = std::fs::File::open("/dev/tty") {
            let mut reader = std::io::BufReader::new(tty);
            reader.read_line(&mut raw_input).context("reading authorization code")?;
        } else {
            std::io::stdin().read_line(&mut raw_input).context("reading authorization code")?;
        }
    }
    #[cfg(not(unix))]
    {
        std::io::stdin().read_line(&mut raw_input).context("reading authorization code")?;
    }
    let raw_input = raw_input.trim();
    if raw_input.is_empty() {
        anyhow::bail!("Empty authorization code");
    }

    // The callback page may return "code#state" — split on '#'.
    let parts: Vec<&str> = raw_input.splitn(2, '#').collect();
    let code = parts[0];
    let state = if parts.len() > 1 { parts[1] } else { "" };

    // 4. Exchange code for tokens — must include User-Agent matching Claude CLI.
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(TOKEN_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("User-Agent", "claude-cli/1.0 (external, cli)")
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": state,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": verifier,
        }))
        .send()
        .context("sending token exchange request")?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().context("parsing token response")?;

    if !status.is_success() {
        let error_msg = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("Token exchange failed ({}): {}", status, error_msg);
    }

    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .context("missing access_token in response")?
        .to_string();

    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let expires_in = body
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expires_at = now + expires_in;

    let creds = OAuthCredentials {
        access_token,
        refresh_token,
        expires_at,
    };

    // 5. Save to file.
    save_credentials(fennec_home, &creds)?;
    println!("Authentication successful! Credentials saved.");

    Ok(creds)
}

/// Load an OAuth access token from disk, refreshing if expired.
///
/// Returns `Ok(Some(token))` if valid credentials exist, `Ok(None)` if no
/// credentials are stored, or an error if refresh fails.
pub fn load_oauth_token(fennec_home: &Path) -> Result<Option<String>> {
    let path = fennec_home.join(TOKEN_FILE);
    if !path.exists() {
        return Ok(None);
    }

    let data = std::fs::read_to_string(&path).context("reading oauth token file")?;
    let creds: OAuthCredentials =
        serde_json::from_str(&data).context("parsing oauth credentials")?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // If token has more than 60 seconds remaining, use it as-is.
    if creds.expires_at > now + 60 {
        return Ok(Some(creds.access_token));
    }

    // Try to refresh.
    if creds.refresh_token.is_empty() {
        // No refresh token — credentials are expired and unrecoverable.
        tracing::warn!("OAuth token expired and no refresh token available");
        return Ok(None);
    }

    match refresh_oauth_token(fennec_home, &creds.refresh_token) {
        Ok(new_creds) => Ok(Some(new_creds.access_token)),
        Err(e) => {
            tracing::warn!("Failed to refresh OAuth token: {e}");
            Ok(None)
        }
    }
}

/// Refresh an OAuth token using the refresh_token grant.
pub fn refresh_oauth_token(
    fennec_home: &Path,
    refresh_token: &str,
) -> Result<OAuthCredentials> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(TOKEN_ENDPOINT)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID,
        }))
        .send()
        .context("sending refresh token request")?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().context("parsing refresh response")?;

    if !status.is_success() {
        let error_msg = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("Token refresh failed ({}): {}", status, error_msg);
    }

    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .context("missing access_token in refresh response")?
        .to_string();

    let new_refresh = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or(refresh_token)
        .to_string();

    let expires_in = body
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expires_at = now + expires_in;

    let creds = OAuthCredentials {
        access_token,
        refresh_token: new_refresh,
        expires_at,
    };

    save_credentials(fennec_home, &creds)?;
    Ok(creds)
}

/// Persist credentials to the token file.
fn save_credentials(fennec_home: &Path, creds: &OAuthCredentials) -> Result<()> {
    std::fs::create_dir_all(fennec_home)
        .context("creating fennec home directory")?;
    let path = fennec_home.join(TOKEN_FILE);
    let json = serde_json::to_string_pretty(creds).context("serializing credentials")?;
    std::fs::write(&path, json).context("writing credentials file")?;

    // Best-effort: restrict file permissions on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

/// Minimal percent-encoding for URL query parameters.
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64url_encode() {
        // Test with known values.
        let input = b"hello";
        let encoded = base64url_encode(input);
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('='));
    }

    #[test]
    fn test_pkce_challenge_is_deterministic() {
        let verifier = "test-verifier-value";
        let c1 = compute_challenge(verifier);
        let c2 = compute_challenge(verifier);
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_verifier_length() {
        let v = generate_verifier();
        // 32 bytes base64url-encoded should be 43 characters.
        assert!(v.len() >= 40);
    }

    #[test]
    fn test_urlencoded() {
        assert_eq!(urlencoded("hello world"), "hello%20world");
        assert_eq!(urlencoded("a+b"), "a%2Bb");
    }

    #[test]
    fn test_save_and_load_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let creds = OAuthCredentials {
            access_token: "test-access".to_string(),
            refresh_token: "test-refresh".to_string(),
            expires_at: u64::MAX,
        };
        save_credentials(dir.path(), &creds).unwrap();

        let loaded = load_oauth_token(dir.path()).unwrap();
        assert_eq!(loaded, Some("test-access".to_string()));
    }

    #[test]
    fn test_load_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_oauth_token(dir.path()).unwrap();
        assert_eq!(loaded, None);
    }
}
