//! Session ingestion contracts (Flow A).

use crate::applicability::DependencyVersion;
use crate::model::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One normalized transcript message. Content is untrusted.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct TranscriptMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Optional structured hints supplied by the agent integration.
/// Hints are untrusted and never count as verification.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
pub struct SessionHints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symptom: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_cause: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub error_signatures: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests_run: Vec<String>,
    /// Dependency versions in the environment where the fix was made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DependencyVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// `POST /v1/sessions` body. Requires an `Idempotency-Key` header.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionIngestRequest {
    pub agent_type: AgentType,
    /// The agent's own session identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// Repository full name, e.g. `acme/api`.
    pub repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// True when the working tree had uncommitted changes at session end, in
    /// which case `revision` does not contain the fix and is not used as the
    /// fix revision for verification.
    #[serde(default)]
    pub worktree_dirty: bool,
    /// Repository-relative path of the component worked on (monorepos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_path: Option<String>,
    #[serde(default)]
    pub messages: Vec<TranscriptMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hints: Option<SessionHints>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionAccepted {
    /// Ingestion ID; identical retries return the same ID.
    pub id: Uuid,
    pub status_url: String,
    /// True when this response replays an earlier identical request.
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionStatusResponse {
    pub id: Uuid,
    pub repository: String,
    pub agent_type: AgentType,
    pub external_id: Option<String>,
    pub revision: Option<String>,
    pub branch: Option<String>,
    /// Ingestion is durable once accepted; extraction is tracked separately.
    pub ingestion_status: String,
    pub extraction_status: ExtractionStatus,
    pub extraction_error: Option<String>,
    pub extractor: Option<String>,
    pub capture_mode: CaptureMode,
    pub raw_payload_available: bool,
    pub message_count: i32,
    pub payload_bytes: i32,
    pub redaction_count: i32,
    pub solution_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub retention_expires_at: Option<DateTime<Utc>>,
}
