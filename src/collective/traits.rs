use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::memory::experience::Experience;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectiveSearchResult {
    pub id: String,
    pub goal: String,
    pub solution: Option<String>,
    pub gotchas: Vec<String>,
    pub trust_score: f64,
    pub relevance_score: f64,
    pub outcome_reports: OutcomeReports,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeReports {
    pub success: u32,
    pub failure: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeReport {
    pub success: bool,
    pub execution_time_ms: Option<u64>,
    pub error_message: Option<String>,
    pub context_notes: Option<String>,
}

#[async_trait]
pub trait CollectiveLayer: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<CollectiveSearchResult>>;
    async fn get_experience(&self, id: &str) -> anyhow::Result<Option<Experience>>;
    async fn publish(&self, experience: &Experience) -> anyhow::Result<String>;
    async fn report_outcome(&self, experience_id: &str, report: &OutcomeReport) -> anyhow::Result<()>;
    async fn health_check(&self) -> bool;
}
