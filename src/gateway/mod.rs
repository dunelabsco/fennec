pub mod auth;
pub mod routes;

use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct GatewayServer {
    addr: String,
}

impl GatewayServer {
    pub fn new(addr: String) -> Self {
        Self { addr }
    }

    /// Run the gateway with no shutdown signal — keeps serving until the
    /// surrounding task is aborted. Equivalent to the previous behavior;
    /// callers that want graceful shutdown should use [`Self::run_with_shutdown`].
    pub async fn run(
        &self,
        agent: Arc<Mutex<crate::agent::Agent>>,
        auth_token: Option<String>,
    ) -> anyhow::Result<()> {
        // Pending future never resolves → equivalent to "no graceful shutdown".
        self.run_with_shutdown(agent, auth_token, std::future::pending::<()>())
            .await
    }

    /// Run the gateway, gracefully shutting down when `shutdown` resolves.
    ///
    /// On shutdown:
    ///   - axum stops accepting new connections.
    ///   - In-flight requests are given a chance to complete.
    ///   - Once all in-flight requests finish (or hit their per-request
    ///     timeout), [`axum::serve`] returns.
    ///
    /// The previous version called `axum::serve(listener, app).await`
    /// with no shutdown wiring. The supervisor in `main.rs` could only
    /// shut the gateway down by `tokio::JoinHandle::abort()`, which
    /// dropped any in-flight `/chat` request mid-turn — including the
    /// `Arc<Mutex<Agent>>` guard a handler was holding. With graceful
    /// shutdown, in-flight handlers see their cancellation through
    /// natural future-drop semantics during request-timeout, and the
    /// agent mutex is released cleanly.
    pub async fn run_with_shutdown<F>(
        &self,
        agent: Arc<Mutex<crate::agent::Agent>>,
        auth_token: Option<String>,
        shutdown: F,
    ) -> anyhow::Result<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let state = routes::AppState { agent, auth_token };
        let app = routes::build_router(state);
        let listener = tokio::net::TcpListener::bind(&self.addr).await?;
        tracing::info!("Gateway listening on {}", self.addr);
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown.await;
                tracing::info!("Gateway received shutdown signal; draining...");
            })
            .await?;
        tracing::info!("Gateway stopped");
        Ok(())
    }
}
