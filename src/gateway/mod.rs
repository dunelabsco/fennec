pub mod auth;
pub mod routes;

use std::sync::Arc;
use tokio::sync::Mutex;

pub struct GatewayServer {
    addr: String,
}

impl GatewayServer {
    pub fn new(addr: String) -> Self {
        Self { addr }
    }

    pub async fn run(
        &self,
        agent: Arc<Mutex<crate::agent::Agent>>,
        auth_token: Option<String>,
    ) -> anyhow::Result<()> {
        let state = routes::AppState { agent, auth_token };
        let app = routes::build_router(state);
        let listener = tokio::net::TcpListener::bind(&self.addr).await?;
        tracing::info!("Gateway listening on {}", self.addr);
        axum::serve(listener, app).await?;
        Ok(())
    }
}
