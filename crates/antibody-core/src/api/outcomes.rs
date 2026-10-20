//! Outcome reporting contracts.

use crate::model::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct CheckEvidence {
    /// CI check or test command name.
    pub name: String,
    /// Revision the check ran on.
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub result: EvidenceResult,
}

/// `POST /v1/outcomes` and MCP `report_outcome`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct OutcomeRequest {
    /// The retrieval that surfaced the solution (from find_relevant_context).
    /// Required so outcomes are always attributable to a real suggestion.
    pub retrieval_id: Uuid,
    pub solution_id: Uuid,
    pub outcome: OutcomeKind,
    /// Optional check evidence. Recorded as a claim until a signed CI event
    /// confirms the same check on the same revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct OutcomeRecorded {
    pub id: Uuid,
    pub verification: OutcomeVerification,
    /// True when an identical outcome was already recorded.
    pub duplicate: bool,
}
