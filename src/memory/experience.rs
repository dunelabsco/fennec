use serde::{Deserialize, Serialize};

/// Describes the context in which an experience occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceContext {
    pub tools_used: Vec<String>,
    pub environment: String,
    pub constraints: String,
}

/// A single attempt made while pursuing a goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub action: String,
    pub outcome: String,
    pub dead_end: bool,
    pub insight: String,
}

/// A recorded experience — a goal, the attempts made, and the eventual solution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    pub id: String,
    pub goal: String,
    pub context: ExperienceContext,
    pub attempts: Vec<Attempt>,
    pub solution: Option<String>,
    pub gotchas: Vec<String>,
    pub tags: Vec<String>,
    pub confidence: f32,
    pub session_id: Option<String>,
    pub created_at: String,
}
