//! Solution detail, listing, state change and propagation (Flow D) contracts.

use super::{SourceLink, VerificationSummary};
use crate::applicability::Applicability;
use crate::model::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct EvidenceView {
    pub id: Uuid,
    pub kind: EvidenceKind,
    pub result: EvidenceResult,
    /// Whether this evidence came from a trusted source (signed webhook or
    /// audited admin action). Untrusted evidence never verifies.
    pub trusted: bool,
    pub tested_revision: Option<String>,
    pub check_name: Option<String>,
    pub environment: Option<String>,
    pub detail: Option<String>,
    pub source: Option<SourceLink>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct SolutionVersionRef {
    pub id: Uuid,
    pub version: i32,
    pub state: SolutionState,
    pub applicability: Applicability,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct SolutionDetail {
    pub id: Uuid,
    pub repository: String,
    pub repository_id: Uuid,
    pub component: Option<String>,
    pub title: String,
    pub symptom: String,
    pub root_cause: Option<String>,
    pub remedy: String,
    pub error_signatures: Vec<String>,
    pub symbols: Vec<String>,
    pub paths: Vec<String>,
    pub applicability: Applicability,
    pub verification: VerificationSummary,
    pub version: i32,
    pub confidence: f32,
    pub extractor: String,
    pub corroboration_count: i32,
    pub fix_revision: Option<String>,
    pub fix_branch: Option<String>,
    pub superseded_by: Option<Uuid>,
    pub sources: Vec<SourceLink>,
    pub evidence: Vec<EvidenceView>,
    pub other_versions: Vec<SolutionVersionRef>,
    pub session_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Actions the caller is authorized to take.
    pub can_withdraw: bool,
    pub can_supersede: bool,
    pub notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct SolutionSummary {
    pub id: Uuid,
    pub repository: String,
    pub title: String,
    pub symptom: String,
    pub state: SolutionState,
    pub needs_revalidation: bool,
    pub merge_state: MergeState,
    pub version: i32,
    pub corroboration_count: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SolutionList {
    pub items: Vec<SolutionSummary>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SolutionStateChange {
    pub action: SolutionAction,
    /// Required for `supersede`: the replacing solution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SolutionAction {
    Withdraw,
    Supersede,
    /// Audited manual verification by an administrator.
    Verify,
    /// Clear the needs_revalidation flag after an administrator review.
    ClearRevalidation,
}

// ---------------------------------------------------------------------------
// Propagation (Flow D)

/// A place that may benefit from a fix. Informational only: Antibody never
/// applies changes, in this repository or any other.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct PropagationCandidate {
    pub repository_id: Uuid,
    pub repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    pub reason: String,
    /// A related solution in that location (same problem, different fix state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_solution_id: Option<Uuid>,
    /// CI failures there with the same normalized error signature.
    pub matching_ci_failures: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct PropagationList {
    pub solution_id: Uuid,
    pub candidates: Vec<PropagationCandidate>,
    pub notice: String,
}
