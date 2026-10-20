//! Knowledge extraction from a (redacted) session transcript.
//!
//! The transcript is untrusted input. The heuristic extractor only copies
//! bounded text and normalized signatures; the LLM extractor wraps the
//! transcript as data and strictly validates the JSON it gets back. Neither
//! ever executes anything derived from the transcript.

mod heuristic;
mod llm;
#[cfg(test)]
mod tests;

pub use heuristic::heuristic;
pub use llm::{LlmExtractError, with_llm};

use antibody_core::api::SessionIngestRequest;
use antibody_core::applicability::{Applicability, DependencyConstraint};
use antibody_core::normalize;
use antibody_core::redact::redact;
use antibody_core::text::{clean, one_line};

pub const HEURISTIC_EXTRACTOR: &str = "heuristic-v1";

#[derive(Debug, Clone, Default)]
pub struct Candidate {
    pub title: String,
    pub symptom: String,
    pub root_cause: Option<String>,
    pub remedy: String,
    pub error_signatures: Vec<String>,
    pub symbols: Vec<String>,
    pub paths: Vec<String>,
    pub applicability: Applicability,
    pub confidence: f32,
    /// The agent (or model) claims tests passed. Never verification.
    pub claims_tests_passed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowObservation {
    pub title: String,
    pub proposed_change: String,
    pub scope: String,
}

#[derive(Debug, Clone, Default)]
pub struct Extraction {
    pub extractor: String,
    pub candidates: Vec<Candidate>,
    pub workflow: Vec<WorkflowObservation>,
}

/// Caret constraints for dependencies the session actually talked about.
fn constraints_for(req: &SessionIngestRequest, corpus: &str) -> Vec<DependencyConstraint> {
    let Some(h) = &req.hints else { return vec![] };
    let lower = corpus.to_lowercase();
    h.dependencies
        .iter()
        .filter(|d| d.name.len() >= 2 && lower.contains(&d.name.to_lowercase()))
        .take(8)
        .map(|d| DependencyConstraint {
            name: d.name.clone(),
            ecosystem: d.ecosystem.clone(),
            version_req: format!("^{}", d.version.trim_start_matches(['v', '^', '=', '~'])),
        })
        .collect()
}

fn applicability_for(req: &SessionIngestRequest, corpus: &str) -> Applicability {
    Applicability {
        dependencies: constraints_for(req, corpus),
        components: req
            .component_path
            .as_deref()
            .map(|p| {
                vec![format!(
                    "{}/**",
                    normalize::normalize_path(p).trim_end_matches('/')
                )]
            })
            .unwrap_or_default(),
        platforms: req
            .hints
            .as_ref()
            .and_then(|h| h.platform.clone())
            .into_iter()
            .collect(),
        conditions: None,
    }
}

/// Bound and re-redact every field (model output may echo secrets).
fn sanitize(mut c: Candidate) -> Candidate {
    let r = |s: &str, n: usize| redact(&clean(s, n)).text;
    c.title = redact(&one_line(&c.title, 160)).text;
    c.symptom = r(&c.symptom, 1200);
    c.root_cause = c
        .root_cause
        .as_deref()
        .map(|s| r(s, 1200))
        .filter(|s| !s.is_empty());
    c.remedy = r(&c.remedy, 2400);
    c.error_signatures.truncate(10);
    c.symbols.truncate(20);
    c.paths.truncate(20);
    c.confidence = c.confidence.clamp(0.0, 0.9);
    c
}
