//! Health check contracts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthStatus {
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<HealthCheck>,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthCheck {
    pub name: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
