//! Workflow improvement proposals and their lifecycle.

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use antibody_core::api::ErrorBody;
use antibody_core::model::WorkflowState;
use antibody_core::state::{PromotionGate, workflow_transition};
use antibody_db::audit::{self, Actor};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct WorkflowImprovementView {
    pub id: Uuid,
    pub repository_id: Option<Uuid>,
    pub target_scope: String,
    pub title: String,
    pub proposed_change: String,
    pub version: i32,
    pub state: String,
    pub evaluation: Option<serde_json::Value>,
    pub approved_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[utoipa::path(get, path = "/v1/admin/workflow-improvements", operation_id = "list_workflow_improvements", tag = "admin", responses((status = 200, body = Vec<WorkflowImprovementView>)), security(("cookie" = []), ("bearer" = [])))]
pub async fn list_workflow(
    State(state): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<Vec<WorkflowImprovementView>>> {
    auth.require_admin()?;
    Ok(Json(
        sqlx::query_as(
            "SELECT id, repository_id, target_scope, title, proposed_change, version, state, evaluation, approved_by, created_at, updated_at
               FROM workflow_improvements WHERE organization_id = $1 ORDER BY updated_at DESC LIMIT 200",
        )
        .bind(auth.0.org_id())
        .fetch_all(&state.pool)
        .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkflowImprovement {
    pub title: String,
    pub proposed_change: String,
    /// e.g. `repository`, `organization`, `component:services/api`.
    pub target_scope: String,
    pub repository_id: Option<Uuid>,
}

/// Propose a workflow improvement manually (they are also proposed by the
/// LLM extractor). New proposals start as `candidate`.
#[utoipa::path(post, path = "/v1/admin/workflow-improvements", operation_id = "create_workflow_improvement", tag = "admin", request_body = CreateWorkflowImprovement, responses((status = 201, body = WorkflowImprovementView), (status = 409, body = ErrorBody)), security(("cookie" = []), ("bearer" = [])))]
pub async fn create_workflow(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<CreateWorkflowImprovement>,
) -> ApiResult<(StatusCode, Json<WorkflowImprovementView>)> {
    auth.require_admin()?;
    let title = antibody_core::text::one_line(&body.title, 160);
    let change = antibody_core::text::clean(&body.proposed_change, 4000);
    let scope = antibody_core::text::one_line(&body.target_scope, 120);
    if title.is_empty() || change.len() < 10 || scope.is_empty() {
        return Err(ApiError::invalid(
            "title, target_scope and a proposed_change of at least 10 characters are required",
        ));
    }
    if let Some(repo) = body.repository_id {
        antibody_db::repos::get(&state.pool, auth.0.org_id(), repo)
            .await?
            .ok_or_else(|| ApiError::not_found("repository"))?;
    }
    let fingerprint = antibody_core::normalize::short_hash(
        format!(
            "{}|{}",
            body.repository_id
                .map(|r| r.to_string())
                .unwrap_or_default(),
            antibody_core::normalize::query_terms(&change, 24).join(" ")
        )
        .as_bytes(),
    );
    let row: Option<WorkflowImprovementView> = sqlx::query_as(
        "INSERT INTO workflow_improvements (id, organization_id, repository_id, target_scope, title, proposed_change, fingerprint, owner_principal_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (organization_id, fingerprint) DO NOTHING
         RETURNING id, repository_id, target_scope, title, proposed_change, version, state, evaluation, approved_by, created_at, updated_at",
    )
    .bind(Uuid::now_v7())
    .bind(auth.0.org_id())
    .bind(body.repository_id)
    .bind(&scope)
    .bind(&title)
    .bind(&change)
    .bind(&fingerprint)
    .bind(auth.0.principal.id)
    .fetch_optional(&state.pool)
    .await?;
    let row = row.ok_or_else(|| {
        ApiError::new(
            antibody_core::ErrorCode::Conflict,
            "an equivalent workflow improvement already exists",
        )
    })?;
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "workflow.created",
        "workflow_improvement",
        Some(row.id.to_string()),
        serde_json::json!({ "title": title }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTransition {
    pub to: WorkflowState,
    /// Evaluation record (required to move to `evaluated`).
    pub evaluation: Option<serde_json::Value>,
    #[serde(default)]
    pub evaluation_reproducible: bool,
    #[serde(default)]
    pub has_negative_cases: bool,
    #[serde(default)]
    pub preserves_checks: bool,
}

#[utoipa::path(post, path = "/v1/admin/workflow-improvements/{id}/transition", operation_id = "transition_workflow_improvement", tag = "admin", params(("id" = Uuid, Path)), request_body = WorkflowTransition, responses((status = 200, body = WorkflowImprovementView), (status = 409, body = ErrorBody)), security(("cookie" = []), ("bearer" = [])))]
pub async fn transition_workflow(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    ApiJson(body): ApiJson<WorkflowTransition>,
) -> ApiResult<Json<WorkflowImprovementView>> {
    auth.require_admin()?;
    let current: Option<String> = sqlx::query_scalar(
        "SELECT state FROM workflow_improvements WHERE id = $1 AND organization_id = $2",
    )
    .bind(id)
    .bind(auth.0.org_id())
    .fetch_optional(&state.pool)
    .await?;
    let current = WorkflowState::from_str(
        &current.ok_or_else(|| ApiError::not_found("workflow improvement"))?,
    )?;
    if body.to == WorkflowState::Evaluated && body.evaluation.is_none() {
        return Err(ApiError::invalid("an evaluation record is required"));
    }
    // The approving administrator is the owner approval for promotion.
    let gate = PromotionGate {
        owner_approved: true,
        evaluation_reproducible: body.evaluation_reproducible,
        has_negative_cases: body.has_negative_cases,
        preserves_checks: body.preserves_checks,
    };
    let next = workflow_transition(current, body.to, &gate)?;
    let row: WorkflowImprovementView = sqlx::query_as(
        "UPDATE workflow_improvements SET state = $3, evaluation = COALESCE($4, evaluation),
                approved_by = CASE WHEN $3 = 'promoted' THEN $5 ELSE approved_by END, updated_at = now()
          WHERE id = $1 AND organization_id = $2
        RETURNING id, repository_id, target_scope, title, proposed_change, version, state, evaluation, approved_by, created_at, updated_at",
    )
    .bind(id)
    .bind(auth.0.org_id())
    .bind(next.as_str())
    .bind(&body.evaluation)
    .bind(auth.0.principal.id)
    .fetch_one(&state.pool)
    .await?;
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "workflow.transitioned",
        "workflow_improvement",
        Some(id.to_string()),
        serde_json::json!({ "from": current, "to": next }),
    )
    .await?;
    Ok(Json(row))
}
