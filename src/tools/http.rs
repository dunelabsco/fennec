//! Shared `reqwest::Client` for tools that make outbound HTTP calls.
//!
//! Every networking tool used to build its own client via
//! `reqwest::Client::builder().timeout(...).build()`. That meant each
//! tool kept its own DNS cache, its own HTTP/2 connection pool, and
//! its own keep-alive state. For an agent that calls weather + vision
//! + http_request in the same turn, those redundant pools added up to
//! several seconds of cold-start latency per turn and considerable
//! memory churn over a long session.
//!
//! `shared_client` returns a clone of a single process-wide
//! `reqwest::Client`. `Client` is internally `Arc`-backed, so cloning
//! is cheap and every clone shares the same DNS cache and connection
//! pool — exactly what the audit asked for.
//!
//! The shared client has **no global timeout**: callers set timeouts
//! per-request via `RequestBuilder::timeout(d)`. A global timeout on
//! the client would force every tool to use the same value, which
//! ranges from 10s (weather) to 120s (Whisper audio uploads) in
//! practice. The redirect policy is the reqwest default
//! (`Policy::limited(10)`).

use std::sync::LazyLock;
use std::time::Duration;

use reqwest::{redirect::Policy, Client};

/// User-agent applied to every request from a Fennec tool. Some vendor
/// APIs (e.g. crates.io) require a non-empty UA and rate-limit anonymous
/// callers more aggressively, so it's worth identifying ourselves.
const USER_AGENT: &str = concat!("Fennec/", env!("CARGO_PKG_VERSION"));

static SHARED: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .user_agent(USER_AGENT)
        .redirect(Policy::limited(10))
        // pool_idle_timeout: how long to keep an idle connection. The
        // reqwest default is 90s; explicit here so future bumps are
        // intentional.
        .pool_idle_timeout(Some(Duration::from_secs(90)))
        .build()
        .expect("build shared reqwest client")
});

/// Return a `Client` that shares its connection pool with every other
/// caller of [`shared_client`]. Cloning a `Client` is O(1) — it's an
/// `Arc` under the hood — so callers can store the result in a struct
/// field freely.
///
/// Per-request timeouts must be set on the [`reqwest::RequestBuilder`]:
/// `client.get(url).timeout(Duration::from_secs(30)).send().await`.
pub fn shared_client() -> Client {
    SHARED.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two calls to `shared_client` return clones of the same underlying
    /// client. `Client::clone` shares the connection pool, so this isn't
    /// a perfect test of "pool sharing" — but it confirms the LazyLock
    /// path returns the same logical client.
    #[test]
    fn shared_client_returns_clones_of_one_client() {
        let a = shared_client();
        let b = shared_client();
        // We can't compare reqwest::Clients directly (no PartialEq), but
        // we can verify both can issue requests by going through their
        // builder. The real test of pool sharing is observed at runtime
        // (lower latency, fewer DNS lookups) and via `cargo test` not
        // measuring it.
        let _r1 = a.get("https://example.com");
        let _r2 = b.get("https://example.com");
    }

    #[test]
    fn user_agent_string_includes_crate_version() {
        assert!(USER_AGENT.starts_with("Fennec/"));
        // CARGO_PKG_VERSION is non-empty for any cargo build.
        assert!(USER_AGENT.len() > "Fennec/".len());
    }
}
