//! Background job inspection and retry.

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use antibody_core::api::ErrorBody;
use antibody_db::audit::{self, Actor};
use antibody_db::jobs::{self, JobView};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct JobQuery {
    pub state: Option<String>,
    pub limit: Option<i64>,
}

#[utoipa::path(get, path = "/v1/admin/jobs", operation_id = "list_jobs", tag = "admin", params(JobQuery), responses((status = 200, body = Vec<JobView>)), security(("cookie" = []), ("bearer" = [])))]
pub async fn list_jobs(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<JobQuery>,
) -> ApiResult<Json<Vec<JobView>>> {
    auth.require_admin()?;
    if let Some(s) = &q.state {
        antibody_core::model::JobState::from_str(s)?;
    }
    Ok(Json(
        jobs::list(
            &state.pool,
            auth.0.org_id(),
            q.state.as_deref(),
            q.limit.unwrap_or(100).clamp(1, 500),
        )
        .await?,
    ))
}

#[utoipa::path(post, path = "/v1/admin/jobs/{id}/retry", operation_id = "retry_job", tag = "admin", params(("id" = Uuid, Path)), responses((status = 204), (status = 404, body = ErrorBody)), security(("cookie" = []), ("bearer" = [])))]
pub async fn retry_job(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    auth.require_admin()?;
    if !jobs::retry_dead(&state.pool, auth.0.org_id(), id).await? {
        return Err(ApiError::not_found("dead job"));
    }
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "job.retried",
        "job",
        Some(id.to_string()),
        serde_json::json!({}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
