//! Overview and activity feeds (scope-filtered; any dashboard user).

use crate::auth::Auth;
use crate::error::ApiResult;
use crate::state::AppState;
use antibody_core::model::Scope;
use antibody_db::ci;
use antibody_db::jobs::{self, QueueStats};
use antibody_db::retrievals::{self, RetrievalView};
use antibody_db::stats::{self, Overview};
use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OverviewResponse {
    pub stats: Overview,
    /// Queue health; administrators only.
    pub queue: Option<QueueStats>,
}

#[utoipa::path(get, path = "/v1/overview", operation_id = "get_overview", tag = "dashboard", responses((status = 200, body = OverviewResponse)), security(("cookie" = []), ("bearer" = [])))]
pub async fn overview(
    State(state): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<OverviewResponse>> {
    auth.require(Scope::ContextRead)?;
    let scope = auth.scope(&state).await?;
    let s = stats::overview(
        &state.pool,
        scope.organization_id,
        &scope.readable_ids(),
        can_view_unscoped_retrievals(&auth),
    )
    .await?;
    let queue = if auth.0.is_admin() {
        Some(jobs::stats(&state.pool, Some(scope.organization_id)).await?)
    } else {
        None
    };
    Ok(Json(OverviewResponse { stats: s, queue }))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CiEventView {
    pub id: Uuid,
    pub repository: String,
    pub source: String,
    pub revision: String,
    pub check_name: String,
    pub conclusion: String,
    pub signature: String,
    /// Redacted, untrusted excerpt. Render as plain text.
    pub excerpt: Option<String>,
    pub url: Option<String>,
    pub status: String,
    pub match_count: i32,
    pub suggested_solution_ids: Vec<Uuid>,
    pub retrieval_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct LimitQuery {
    pub limit: Option<i64>,
}

#[utoipa::path(get, path = "/v1/activity/ci-events", operation_id = "list_ci_events", tag = "dashboard", params(LimitQuery), responses((status = 200, body = Vec<CiEventView>)), security(("cookie" = []), ("bearer" = [])))]
pub async fn ci_events(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<LimitQuery>,
) -> ApiResult<Json<Vec<CiEventView>>> {
    auth.require(Scope::ContextRead)?;
    let scope = auth.scope(&state).await?;
    let rows = ci::list_in_scope(
        &state.pool,
        scope.organization_id,
        &scope.readable_ids(),
        q.limit.unwrap_or(50).clamp(1, 200),
    )
    .await?;
    // Only expose suggested solutions the caller can read.
    let readable = scope.readable_ids();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let visible = antibody_db::solutions::get_many_in_scope(
            &state.pool,
            scope.organization_id,
            &readable,
            &r.candidate_ids,
        )
        .await?;
        out.push(CiEventView {
            id: r.id,
            repository: r.repository,
            source: r.source,
            revision: r.revision,
            check_name: r.check_name,
            conclusion: r.conclusion,
            signature: r.signature,
            excerpt: r.excerpt,
            url: r.url,
            status: r.status,
            match_count: visible.len() as i32,
            suggested_solution_ids: visible.into_iter().map(|s| s.id).collect(),
            retrieval_id: r.retrieval_id,
            created_at: r.created_at,
        });
    }
    Ok(Json(out))
}

#[utoipa::path(get, path = "/v1/activity/retrievals", operation_id = "list_retrievals", tag = "dashboard", params(LimitQuery), responses((status = 200, body = Vec<RetrievalView>)), security(("cookie" = []), ("bearer" = [])))]
pub async fn retrievals(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<LimitQuery>,
) -> ApiResult<Json<Vec<RetrievalView>>> {
    auth.require(Scope::ContextRead)?;
    let scope = auth.scope(&state).await?;
    Ok(Json(
        retrievals::list_recent(
            &state.pool,
            scope.organization_id,
            &scope.readable_ids(),
            can_view_unscoped_retrievals(&auth),
            q.limit.unwrap_or(50).clamp(1, 200),
        )
        .await?,
    ))
}

// A repository-less search can span private repositories. It has no
// repository grant that can authorize its aggregate counts or activity.
fn can_view_unscoped_retrievals(auth: &Auth) -> bool {
    auth.0.is_admin() && auth.0.token_repository_ids.is_none()
}
