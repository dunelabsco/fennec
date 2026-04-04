use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::traits::*;
use crate::memory::experience::{Attempt, Experience, ExperienceContext};

pub struct PlurumlClient {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

// ---------------------------------------------------------------------------
// Wire types for Plurum API serialization/deserialization
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SearchRequest {
    query: String,
    match_count: usize,
}

#[derive(Deserialize)]
struct PlurumlSearchResult {
    id: String,
    goal: String,
    solution: Option<String>,
    #[serde(default)]
    gotchas: Vec<String>,
    /// Plurum may send `trust_score` or the older `quality_score` name.
    trust_score: Option<f64>,
    quality_score: Option<f64>,
    relevance_score: Option<f64>,
    #[serde(default)]
    outcome_reports: Option<PlurumlOutcomeReports>,
}

#[derive(Deserialize)]
struct PlurumlOutcomeReports {
    success: u32,
    failure: u32,
}

/// Plurum experience response — may use `attempts` or the legacy
/// `dead_ends` + `breakthroughs` fields.
#[derive(Deserialize)]
struct PlurumlExperience {
    id: String,
    goal: String,
    #[serde(default)]
    context: Option<PlurumlContext>,
    #[serde(default)]
    attempts: Option<Vec<PlurumlAttempt>>,
    #[serde(default)]
    dead_ends: Option<Vec<String>>,
    #[serde(default)]
    breakthroughs: Option<Vec<String>>,
    solution: Option<String>,
    #[serde(default)]
    gotchas: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_confidence")]
    confidence: f32,
    session_id: Option<String>,
    #[serde(default = "default_timestamp")]
    created_at: String,
}

fn default_confidence() -> f32 {
    0.5
}

fn default_timestamp() -> String {
    String::new()
}

#[derive(Deserialize)]
struct PlurumlContext {
    #[serde(default)]
    tools_used: Vec<String>,
    #[serde(default)]
    environment: String,
    #[serde(default)]
    constraints: String,
}

#[derive(Deserialize)]
struct PlurumlAttempt {
    action: String,
    outcome: String,
    #[serde(default)]
    dead_end: bool,
    #[serde(default)]
    insight: String,
}

#[derive(Serialize)]
struct PublishRequest {
    id: String,
    goal: String,
    context: PublishContext,
    attempts: Vec<PublishAttempt>,
    solution: Option<String>,
    gotchas: Vec<String>,
    tags: Vec<String>,
    confidence: f32,
    session_id: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
struct PublishContext {
    tools_used: Vec<String>,
    environment: String,
    constraints: String,
}

#[derive(Serialize)]
struct PublishAttempt {
    action: String,
    outcome: String,
    dead_end: bool,
    insight: String,
}

#[derive(Deserialize)]
struct PublishResponse {
    id: String,
}

#[derive(Serialize)]
struct OutcomeReportRequest {
    success: bool,
    execution_time_ms: Option<u64>,
    error_message: Option<String>,
    context_notes: Option<String>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    #[serde(default)]
    message: String,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl PlurumlClient {
    pub fn new(api_key: String, base_url: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");

        Self {
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.plurum.ai".to_string()),
            client,
        }
    }

    /// Build a request with standard headers.
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
    }

    /// Parse an error response body into a contextual anyhow error.
    async fn parse_error(resp: reqwest::Response) -> anyhow::Error {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<ErrorResponse>(&body)
            .map(|e| e.message)
            .unwrap_or(body);
        anyhow::anyhow!("Plurum API error ({}): {}", status, message)
    }
}

#[async_trait]
impl CollectiveLayer for PlurumlClient {
    fn name(&self) -> &str {
        "plurum"
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<CollectiveSearchResult>> {
        let body = SearchRequest {
            query: query.to_string(),
            match_count: limit,
        };

        let resp = self
            .request(reqwest::Method::POST, "/api/v1/experiences/search")
            .json(&body)
            .send()
            .await
            .context("Plurum search request failed")?;

        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }

        let results: Vec<PlurumlSearchResult> = resp
            .json()
            .await
            .context("failed to parse Plurum search response")?;

        Ok(results
            .into_iter()
            .map(|r| {
                let trust_score = r.trust_score.or(r.quality_score).unwrap_or(0.0);
                let reports = r.outcome_reports.unwrap_or(PlurumlOutcomeReports {
                    success: 0,
                    failure: 0,
                });
                CollectiveSearchResult {
                    id: r.id,
                    goal: r.goal,
                    solution: r.solution,
                    gotchas: r.gotchas,
                    trust_score,
                    relevance_score: r.relevance_score.unwrap_or(0.0),
                    outcome_reports: OutcomeReports {
                        success: reports.success,
                        failure: reports.failure,
                    },
                }
            })
            .collect())
    }

    async fn get_experience(&self, id: &str) -> anyhow::Result<Option<Experience>> {
        let resp = self
            .request(reqwest::Method::GET, &format!("/api/v1/experiences/{}", id))
            .send()
            .await
            .context("Plurum get_experience request failed")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }

        let pe: PlurumlExperience = resp
            .json()
            .await
            .context("failed to parse Plurum experience response")?;

        let context = pe.context.map_or_else(
            || ExperienceContext {
                tools_used: vec![],
                environment: String::new(),
                constraints: String::new(),
            },
            |c| ExperienceContext {
                tools_used: c.tools_used,
                environment: c.environment,
                constraints: c.constraints,
            },
        );

        // Use `attempts` if present; otherwise synthesise from legacy fields.
        let attempts = if let Some(atts) = pe.attempts {
            atts.into_iter()
                .map(|a| Attempt {
                    action: a.action,
                    outcome: a.outcome,
                    dead_end: a.dead_end,
                    insight: a.insight,
                })
                .collect()
        } else {
            let mut synth = Vec::new();
            if let Some(dead_ends) = pe.dead_ends {
                for de in dead_ends {
                    synth.push(Attempt {
                        action: de.clone(),
                        outcome: "dead end".to_string(),
                        dead_end: true,
                        insight: de,
                    });
                }
            }
            if let Some(breakthroughs) = pe.breakthroughs {
                for bt in breakthroughs {
                    synth.push(Attempt {
                        action: bt.clone(),
                        outcome: "breakthrough".to_string(),
                        dead_end: false,
                        insight: bt,
                    });
                }
            }
            synth
        };

        Ok(Some(Experience {
            id: pe.id,
            goal: pe.goal,
            context,
            attempts,
            solution: pe.solution,
            gotchas: pe.gotchas,
            tags: pe.tags,
            confidence: pe.confidence,
            session_id: pe.session_id,
            created_at: pe.created_at,
        }))
    }

    async fn publish(&self, experience: &Experience) -> anyhow::Result<String> {
        let body = PublishRequest {
            id: experience.id.clone(),
            goal: experience.goal.clone(),
            context: PublishContext {
                tools_used: experience.context.tools_used.clone(),
                environment: experience.context.environment.clone(),
                constraints: experience.context.constraints.clone(),
            },
            attempts: experience
                .attempts
                .iter()
                .map(|a| PublishAttempt {
                    action: a.action.clone(),
                    outcome: a.outcome.clone(),
                    dead_end: a.dead_end,
                    insight: a.insight.clone(),
                })
                .collect(),
            solution: experience.solution.clone(),
            gotchas: experience.gotchas.clone(),
            tags: experience.tags.clone(),
            confidence: experience.confidence,
            session_id: experience.session_id.clone(),
            created_at: experience.created_at.clone(),
        };

        let resp = self
            .request(reqwest::Method::POST, "/api/v1/experiences")
            .json(&body)
            .send()
            .await
            .context("Plurum publish request failed")?;

        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }

        let created: PublishResponse = resp
            .json()
            .await
            .context("failed to parse Plurum publish response")?;

        Ok(created.id)
    }

    async fn report_outcome(
        &self,
        experience_id: &str,
        report: &OutcomeReport,
    ) -> anyhow::Result<()> {
        let body = OutcomeReportRequest {
            success: report.success,
            execution_time_ms: report.execution_time_ms,
            error_message: report.error_message.clone(),
            context_notes: report.context_notes.clone(),
        };

        let resp = self
            .request(
                reqwest::Method::POST,
                &format!("/api/v1/experiences/{}/outcome", experience_id),
            )
            .json(&body)
            .send()
            .await
            .context("Plurum report_outcome request failed")?;

        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }

        Ok(())
    }

    async fn health_check(&self) -> bool {
        let resp = self
            .request(reqwest::Method::GET, "/api/v1/health")
            .send()
            .await;

        matches!(resp, Ok(r) if r.status().is_success())
    }
}
