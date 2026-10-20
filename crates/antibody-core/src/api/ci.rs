//! CI (Flow C) and webhook contracts.

use super::Suggestion;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Generic signed CI failure event (`POST /v1/webhooks/ci`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CiFailureEvent {
    pub repository: String,
    pub revision: String,
    pub check_name: String,
    /// Already-normalized signature, if the CI system computed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Permitted excerpt of the failing log (redacted and truncated server side).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CiLookupResponse {
    pub ci_event_id: Uuid,
    pub duplicate: bool,
    pub signature: String,
    pub suggestions: Vec<Suggestion>,
    /// CI lookup never fails the build; this is informational.
    pub blocking: bool,
    pub notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WebhookAck {
    pub accepted: bool,
    pub duplicate: bool,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
