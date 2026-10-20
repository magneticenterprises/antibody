//! Flow B: retrieval, solutions and outcomes (HTTP side; MCP shares the
//! same engine and permission checks).

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use antibody_core::api::*;
use antibody_core::model::{RetrievalChannel, Scope, SolutionState};
use antibody_core::state::SolutionEvent;
use antibody_db::audit::{self, Actor};
use antibody_db::evidence::{self, NewEvidence};
use antibody_db::{orgs, solutions};
use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use std::str::FromStr;
use uuid::Uuid;

pub async fn external_approved(state: &AppState, org_id: Uuid) -> ApiResult<bool> {
    Ok(orgs::get(&state.pool, org_id)
        .await?
        .map(|o| o.external_providers_approved)
        .unwrap_or(false))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SearchQuery {
    /// Preview only: run the search without recording a retrieval (it does
    /// not count as usage and cannot receive outcomes). For the dashboard.
    pub preview: Option<bool>,
}

#[utoipa::path(
    post, path = "/v1/context/search", operation_id = "search_context", tag = "retrieval",
    params(SearchQuery),
    request_body = ContextSearchRequest,
    responses((status = 200, description = "Up to three concise suggestions", body = ContextSearchResponse), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn search(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<SearchQuery>,
    ApiJson(req): ApiJson<ContextSearchRequest>,
) -> ApiResult<Json<ContextSearchResponse>> {
    auth.require(Scope::ContextRead)?;
    let scope = auth.scope(&state).await?;
    let approved = external_approved(&state, auth.0.org_id()).await?;
    let record = !q.preview.unwrap_or(false);
    Ok(Json(
        state
            .search
            .search_with(&scope, &req, RetrievalChannel::Http, approved, record)
            .await?,
    ))
}

#[utoipa::path(
    get, path = "/v1/solutions/{id}", operation_id = "get_solution", tag = "retrieval",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = SolutionDetail), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn get_solution(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SolutionDetail>> {
    auth.require(Scope::ContextRead)?;
    let scope = auth.scope(&state).await?;
    Ok(Json(
        state
            .search
            .get_solution(
                &scope,
                id,
                auth.0.is_admin(),
                auth.0.token_repository_ids.is_none(),
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SolutionQuery {
    /// Full-text query.
    pub q: Option<String>,
    pub state: Option<String>,
    pub repository_id: Option<Uuid>,
    pub needs_revalidation: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[utoipa::path(
    get, path = "/v1/solutions", operation_id = "list_solutions", tag = "retrieval",
    params(SolutionQuery),
    responses((status = 200, body = SolutionList)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn list_solutions(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<SolutionQuery>,
) -> ApiResult<Json<SolutionList>> {
    auth.require(Scope::ContextRead)?;
    let scope = auth.scope(&state).await?;
    let st = q
        .state
        .as_deref()
        .map(SolutionState::from_str)
        .transpose()?;
    let (rows, total) = solutions::list_in_scope(
        &state.pool,
        scope.organization_id,
        &scope.readable_ids(),
        &solutions::ListFilter {
            query: q.q.as_deref().filter(|s| !s.trim().is_empty()),
            state: st,
            repository_id: q.repository_id,
            needs_revalidation: q.needs_revalidation,
            limit: q.limit.unwrap_or(50).clamp(1, 200),
            offset: q.offset.unwrap_or(0).max(0),
        },
    )
    .await?;
    Ok(Json(SolutionList {
        total,
        items: rows
            .into_iter()
            .map(|r| SolutionSummary {
                id: r.id,
                state: r.state_enum(),
                merge_state: r.merge_enum(),
                repository: r.repository,
                title: r.title,
                symptom: antibody_core::text::one_line(&r.symptom, 240),
                needs_revalidation: r.needs_revalidation,
                version: r.version,
                corroboration_count: r.corroboration_count,
                updated_at: r.updated_at,
            })
            .collect(),
    }))
}

#[utoipa::path(
    post, path = "/v1/solutions/{id}/state", operation_id = "change_solution_state", tag = "retrieval",
    params(("id" = Uuid, Path)),
    request_body = SolutionStateChange,
    responses((status = 200, body = SolutionDetail), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn change_state(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    ApiJson(body): ApiJson<SolutionStateChange>,
) -> ApiResult<Json<SolutionDetail>> {
    auth.require_admin()?;
    let scope = auth.scope(&state).await?;
    let readable = scope.readable_ids();
    solutions::get_in_scope(&state.pool, scope.organization_id, &readable, id)
        .await?
        .ok_or_else(|| ApiError::not_found("solution"))?;
    let reason = body
        .reason
        .as_deref()
        .map(|r| antibody_core::text::one_line(r, 500));
    let mut tx = state.pool.begin().await?;
    match body.action {
        SolutionAction::Withdraw => {
            solutions::apply_event(&mut tx, id, &SolutionEvent::Withdrawn).await?;
        }
        SolutionAction::Supersede => {
            let by = body
                .superseded_by
                .ok_or_else(|| ApiError::invalid("superseded_by is required"))?;
            if solutions::get_in_scope(&state.pool, scope.organization_id, &readable, by)
                .await?
                .is_none()
            {
                return Err(ApiError::not_found("superseding solution"));
            }
            solutions::set_superseded_by(&mut tx, scope.organization_id, id, by).await?;
            solutions::apply_event(&mut tx, id, &SolutionEvent::Superseded).await?;
        }
        SolutionAction::Verify => {
            evidence::insert(
                &mut tx,
                NewEvidence {
                    org_id: scope.organization_id,
                    solution_id: id,
                    kind: antibody_core::model::EvidenceKind::ManualVerification,
                    result: antibody_core::model::EvidenceResult::Pass,
                    trusted: true,
                    tested_revision: None,
                    check_name: None,
                    environment: None,
                    source_artifact_id: None,
                    detail: Some(format!(
                        "verified by {}{}",
                        auth.0.principal.display_name,
                        reason
                            .as_ref()
                            .map(|r| format!(": {r}"))
                            .unwrap_or_default()
                    )),
                    dedupe_key: format!("manual:{}", uuid::Uuid::now_v7()),
                },
            )
            .await?;
            solutions::apply_event(&mut tx, id, &SolutionEvent::ManuallyVerified).await?;
        }
        SolutionAction::ClearRevalidation => {
            solutions::clear_revalidation(&mut tx, id).await?;
        }
    }
    audit::record(
        &mut *tx,
        Some(scope.organization_id),
        &Actor::from_auth(&auth.0),
        &format!(
            "solution.{}",
            serde_json::to_value(body.action)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        ),
        "solution",
        Some(id.to_string()),
        serde_json::json!({ "superseded_by": body.superseded_by, "reason": reason }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        state
            .search
            .get_solution(&scope, id, true, auth.0.token_repository_ids.is_none())
            .await?,
    ))
}

#[utoipa::path(
    post, path = "/v1/outcomes", operation_id = "report_outcome", tag = "retrieval",
    request_body = OutcomeRequest,
    responses((status = 200, body = OutcomeRecorded), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn report_outcome(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(req): ApiJson<OutcomeRequest>,
) -> ApiResult<Json<OutcomeRecorded>> {
    auth.require(Scope::OutcomesWrite)?;
    let scope = auth.scope(&state).await?;
    Ok(Json(state.search.report_outcome(&scope, &req).await?))
}

#[utoipa::path(
    get, path = "/v1/solutions/{id}/propagation", operation_id = "get_solution_propagation", tag = "retrieval",
    params(("id" = Uuid, Path)),
    responses((status = 200, description = "Locations that may benefit from this fix (never applied automatically)", body = PropagationList), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn propagation(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PropagationList>> {
    auth.require(Scope::ContextRead)?;
    let scope = auth.scope(&state).await?;
    Ok(Json(state.search.propagation(&scope, id).await?))
}

/// Hard-delete a solution (admin, audited). Intended for leak response;
/// withdraw is the normal way to retire knowledge.
#[utoipa::path(
    delete, path = "/v1/solutions/{id}", operation_id = "delete_solution", tag = "retrieval",
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn delete_solution(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<axum::http::StatusCode> {
    auth.require_admin()?;
    if !solutions::delete(&state.pool, auth.0.org_id(), id).await? {
        return Err(ApiError::not_found("solution"));
    }
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "solution.deleted",
        "solution",
        Some(id.to_string()),
        serde_json::json!({}),
    )
    .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
