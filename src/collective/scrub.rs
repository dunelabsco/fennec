use std::sync::LazyLock;

use regex::Regex;

use crate::memory::experience::{Attempt, Experience, ExperienceContext};

static SCRUB_PATTERNS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    vec![
        // API keys
        (
            Regex::new(r"sk-[a-zA-Z0-9_-]{20,}").unwrap(),
            "[REDACTED_API_KEY]",
        ),
        (
            Regex::new(r"ghp_[a-zA-Z0-9]{20,}").unwrap(),
            "[REDACTED_GITHUB_TOKEN]",
        ),
        (
            Regex::new(r"plrm_live_\S+").unwrap(),
            "[REDACTED_PLURUM_KEY]",
        ),
        (
            Regex::new(r"xox[bpars]-[a-zA-Z0-9-]+").unwrap(),
            "[REDACTED_SLACK_TOKEN]",
        ),
        // Bearer tokens
        (
            Regex::new(r"(?i)Bearer\s+\S{20,}").unwrap(),
            "Bearer [REDACTED]",
        ),
        // User paths
        (
            Regex::new(r"/Users/[a-zA-Z0-9._-]+/").unwrap(),
            "/Users/[REDACTED]/",
        ),
        (
            Regex::new(r"/home/[a-zA-Z0-9._-]+/").unwrap(),
            "/home/[REDACTED]/",
        ),
        (
            Regex::new(r"C:\\Users\\[a-zA-Z0-9._-]+\\").unwrap(),
            "C:\\Users\\[REDACTED]\\",
        ),
        // IP addresses (require word boundaries to avoid matching version numbers etc.)
        (
            Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap(),
            "[REDACTED_IP]",
        ),
        // Internal hostnames
        (
            Regex::new(r"\b\w+\.(local|internal)\b").unwrap(),
            "[REDACTED_HOST]",
        ),
        // Database URLs
        (
            Regex::new(r"(?i)(postgres|mysql|mongodb|redis)://\S+").unwrap(),
            "[REDACTED_DB_URL]",
        ),
        // Generic password/secret in key=value
        (
            Regex::new(r"(?i)(password|passwd|secret|token|api_key)\s*[=:]\s*\S+").unwrap(),
            "[REDACTED_SECRET]",
        ),
    ]
});

/// Apply all scrubbing patterns to a single string.
pub fn scrub_text(text: &str) -> String {
    let mut result = text.to_string();
    for (pattern, replacement) in SCRUB_PATTERNS.iter() {
        result = pattern.replace_all(&result, *replacement).into_owned();
    }
    result
}

/// Scrub all text fields in an experience, returning a cleaned clone.
pub fn scrub_experience(experience: &Experience) -> Experience {
    let context = ExperienceContext {
        tools_used: experience.context.tools_used.clone(),
        environment: scrub_text(&experience.context.environment),
        constraints: scrub_text(&experience.context.constraints),
    };

    let attempts = experience
        .attempts
        .iter()
        .map(|a| Attempt {
            action: scrub_text(&a.action),
            outcome: scrub_text(&a.outcome),
            dead_end: a.dead_end,
            insight: scrub_text(&a.insight),
        })
        .collect();

    Experience {
        id: experience.id.clone(),
        goal: scrub_text(&experience.goal),
        context,
        attempts,
        solution: experience.solution.as_ref().map(|s| scrub_text(s)),
        gotchas: experience.gotchas.iter().map(|g| scrub_text(g)).collect(),
        tags: experience.tags.clone(),
        confidence: experience.confidence,
        session_id: experience.session_id.clone(),
        created_at: experience.created_at.clone(),
    }
}

/// Check whether an experience is clean (no patterns match any text field).
///
/// This is a safety double-check — after scrubbing, `is_clean` should return
/// `true`. If it does not, something was missed.
pub fn is_clean(experience: &Experience) -> bool {
    let fields = collect_text_fields(experience);
    for field in &fields {
        for (pattern, _) in SCRUB_PATTERNS.iter() {
            if pattern.is_match(field) {
                return false;
            }
        }
    }
    true
}

/// Collect all scrubbable text fields from an experience into a flat list.
fn collect_text_fields(experience: &Experience) -> Vec<&str> {
    let mut fields: Vec<&str> = Vec::new();
    fields.push(&experience.goal);
    fields.push(&experience.context.environment);
    fields.push(&experience.context.constraints);
    for attempt in &experience.attempts {
        fields.push(&attempt.action);
        fields.push(&attempt.outcome);
        fields.push(&attempt.insight);
    }
    if let Some(ref sol) = experience.solution {
        fields.push(sol);
    }
    for gotcha in &experience.gotchas {
        fields.push(gotcha);
    }
    fields
}
