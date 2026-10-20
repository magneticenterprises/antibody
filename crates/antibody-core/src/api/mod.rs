//! Request/response contracts shared by HTTP routes, MCP tools and the CLI.
//! These types are the single source for the OpenAPI document and the MCP
//! tool JSON schemas.

mod ci;
mod health;
mod outcomes;
mod repositories;
mod retrieval;
mod sessions;
mod solutions;

pub use ci::*;
pub use health::*;
pub use outcomes::*;
pub use repositories::*;
pub use retrieval::*;
pub use sessions::*;
pub use solutions::*;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ErrorDetail {
    pub code: crate::ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}
