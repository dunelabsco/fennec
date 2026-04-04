use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::traits::*;
use crate::memory::experience::Experience;

pub struct MockCollective {
    experiences: Arc<Mutex<Vec<Experience>>>,
}

impl MockCollective {
    pub fn new() -> Self {
        Self {
            experiences: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_experiences(experiences: Vec<Experience>) -> Self {
        Self {
            experiences: Arc::new(Mutex::new(experiences)),
        }
    }
}

#[async_trait]
impl CollectiveLayer for MockCollective {
    fn name(&self) -> &str {
        "mock"
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<CollectiveSearchResult>> {
        let exps = self.experiences.lock().unwrap();
        let query_lower = query.to_lowercase();
        let results: Vec<CollectiveSearchResult> = exps
            .iter()
            .filter(|e| e.goal.to_lowercase().contains(&query_lower))
            .take(limit)
            .map(|e| CollectiveSearchResult {
                id: e.id.clone(),
                goal: e.goal.clone(),
                solution: e.solution.clone(),
                gotchas: e.gotchas.clone(),
                trust_score: 0.5,
                relevance_score: 0.7,
                outcome_reports: OutcomeReports {
                    success: 0,
                    failure: 0,
                },
            })
            .collect();
        Ok(results)
    }

    async fn get_experience(&self, id: &str) -> anyhow::Result<Option<Experience>> {
        let exps = self.experiences.lock().unwrap();
        Ok(exps.iter().find(|e| e.id == id).cloned())
    }

    async fn publish(&self, experience: &Experience) -> anyhow::Result<String> {
        let mut exps = self.experiences.lock().unwrap();
        let id = experience.id.clone();
        exps.push(experience.clone());
        Ok(id)
    }

    async fn report_outcome(
        &self,
        _experience_id: &str,
        _report: &OutcomeReport,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> bool {
        true
    }
}
