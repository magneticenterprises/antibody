//! Retrieval contracts (Flow B / C).

use crate::applicability::{ApplicabilityAssessment, DependencyVersion};
use crate::model::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Search input for `POST /v1/context/search` and MCP `find_relevant_context`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct ContextSearchRequest {
    /// What the agent is about to work on (the user's task description).
    pub task: String,
    /// Repository full name, e.g. `acme/api`. Strongly recommended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Error output or message the agent is seeing, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Repository-relative component path (monorepos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Dependency versions in the caller's environment, for applicability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DependencyVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Maximum suggestions (1-3, default 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u8>,
    /// Include suggestions whose known constraints are violated.
    #[serde(default)]
    pub include_incompatible: bool,
    /// The agent's session id; used to only inject context once per session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct SourceLink {
    pub kind: SourceKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct VerificationSummary {
    pub state: SolutionState,
    pub merge_state: MergeState,
    pub deployment_state: DeploymentState,
    pub needs_revalidation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revalidation_reason: Option<String>,
    pub trusted_evidence_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verified_at: Option<DateTime<Utc>>,
    /// One-line human summary, e.g. "verified by CI check `test` on abc123".
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct Suggestion {
    pub solution_id: Uuid,
    pub title: String,
    /// The issue this solved (symptom), concise.
    pub issue: String,
    /// What to reuse, concise.
    pub reuse_action: String,
    pub repository: String,
    pub cross_repository: bool,
    pub source_links: Vec<SourceLink>,
    pub applicability: ApplicabilityAssessment,
    pub verification: VerificationSummary,
    pub match_reasons: Vec<String>,
    pub score: f32,
    /// Dashboard link for humans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct ContextSearchResponse {
    /// Pass to report_outcome to attribute feedback.
    pub retrieval_id: Uuid,
    pub suggestions: Vec<Suggestion>,
    /// True when some search stages were skipped (timeout, no embeddings).
    pub degraded: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degraded_reasons: Vec<String>,
    /// Candidates withheld because a known constraint is violated.
    pub suppressed_incompatible: u32,
    pub latency_ms: u32,
    pub notice: String,
}
