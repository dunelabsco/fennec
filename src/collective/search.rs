use std::sync::Arc;

use crate::collective::cache::CollectiveCache;
use crate::collective::traits::{CollectiveLayer, CollectiveSearchResult, OutcomeReports};
use crate::memory::sqlite::SqliteMemory;
use crate::security::prompt_guard::{PromptGuard, ScanResult};

#[derive(Debug, Clone)]
pub enum SearchConfidence {
    High,    // top result >0.85
    Partial, // top result 0.5-0.85
    None,    // <0.5 or no results
}

#[derive(Debug, Clone)]
pub enum ExperienceSource {
    Local,
    Cache,
    Remote,
}

#[derive(Debug, Clone)]
pub struct RankedExperience {
    pub result: CollectiveSearchResult,
    pub source: ExperienceSource,
    pub final_score: f64,
}

impl RankedExperience {
    pub fn source_label(&self) -> &str {
        match self.source {
            ExperienceSource::Local => "local",
            ExperienceSource::Cache => "cache",
            ExperienceSource::Remote => "remote",
        }
    }
}

#[derive(Debug)]
pub struct SearchResult {
    pub experiences: Vec<RankedExperience>,
    pub confidence: SearchConfidence,
}

pub struct CollectiveSearch {
    memory: Arc<SqliteMemory>,
    cache: CollectiveCache,
    remote: Option<Arc<dyn CollectiveLayer>>,
    prompt_guard: Option<PromptGuard>,
}

impl CollectiveSearch {
    pub fn new(
        memory: Arc<SqliteMemory>,
        cache: CollectiveCache,
        remote: Option<Arc<dyn CollectiveLayer>>,
        prompt_guard: Option<PromptGuard>,
    ) -> Self {
        Self {
            memory,
            cache,
            remote,
            prompt_guard,
        }
    }

    pub async fn search(&self, query: &str, limit: usize) -> anyhow::Result<SearchResult> {
        let mut all_results: Vec<RankedExperience> = Vec::new();

        // 1. Local experiences (trust 1.0)
        let local = self.memory.search_experiences(query, limit).await?;
        for exp in local {
            all_results.push(RankedExperience {
                result: CollectiveSearchResult {
                    id: exp.id.clone(),
                    goal: exp.goal.clone(),
                    solution: exp.solution.clone(),
                    gotchas: exp.gotchas.clone(),
                    trust_score: 1.0,
                    relevance_score: 0.8, // local gets a base relevance
                    outcome_reports: OutcomeReports {
                        success: 0,
                        failure: 0,
                    },
                },
                source: ExperienceSource::Local,
                final_score: 0.8 * 1.0, // relevance * trust
            });
        }

        // 2. Collective cache (use cached trust scores)
        let cached = self.cache.search_cache(query, limit).await?;
        for result in cached {
            let score = result.relevance_score * result.trust_score;
            // Deduplicate against local results by goal
            if !all_results.iter().any(|r| r.result.goal == result.goal) {
                all_results.push(RankedExperience {
                    final_score: score,
                    result,
                    source: ExperienceSource::Cache,
                });
            }
        }

        // 3. Remote collective (only if local results are sparse)
        let high_quality_count = all_results.iter().filter(|r| r.final_score > 0.5).count();
        if high_quality_count < 2 {
            if let Some(ref remote) = self.remote {
                match remote.search(query, limit).await {
                    Ok(remote_results) => {
                        for result in remote_results {
                            // Scan for injection if prompt guard is set
                            if let Some(ref guard) = self.prompt_guard {
                                let text = format!(
                                    "{} {}",
                                    result.goal,
                                    result.solution.as_deref().unwrap_or("")
                                );
                                if let ScanResult::Blocked(_) = guard.scan(&text) {
                                    tracing::warn!(
                                        "Blocked potentially poisoned collective result: {}",
                                        result.id
                                    );
                                    continue;
                                }
                            }
                            // Cache the result locally
                            let _ = self.cache.cache_result(&result, "plurum").await;

                            // Use 0.3 as base trust for fresh remote
                            let trust = 0.3_f64.max(result.trust_score * 0.5);
                            let score = result.relevance_score * trust;

                            if !all_results.iter().any(|r| r.result.goal == result.goal) {
                                all_results.push(RankedExperience {
                                    final_score: score,
                                    result,
                                    source: ExperienceSource::Remote,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Remote collective search failed: {}", e);
                    }
                }
            }
        }

        // Sort by final_score descending
        all_results.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(limit);

        // Determine confidence
        let confidence = match all_results.first() {
            Some(r) if r.final_score > 0.85 => SearchConfidence::High,
            Some(r) if r.final_score > 0.5 => SearchConfidence::Partial,
            _ => SearchConfidence::None,
        };

        Ok(SearchResult {
            experiences: all_results,
            confidence,
        })
    }
}
