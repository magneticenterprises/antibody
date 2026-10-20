use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use antibody_core::api::*;
use antibody_core::model::{Permission, RepositoryProvider};
use antibody_db::audit::{self, Actor};
use antibody_db::repos::{self, NewRepository, RepositoryRow};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use std::str::FromStr;
use uuid::Uuid;

fn view(r: RepositoryRow, permission: Permission, comps: &[repos::ComponentRow]) -> RepositoryView {
    RepositoryView {
        id: r.id,
        provider: RepositoryProvider::from_str(&r.provider).unwrap_or(RepositoryProvider::Generic),
        full_name: r.full_name,
        url: r.url,
        default_branch: r.default_branch,
        status: r.status,
        permission,
        components: comps
            .iter()
            .filter(|c| c.repository_id == r.id)
            .map(|c| ComponentView {
                id: c.id,
                name: c.name.clone(),
                path_glob: c.path_glob.clone(),
                package_name: c.package_name.clone(),
                ecosystem: c.ecosystem.clone(),
            })
            .collect(),
        last_event_at: r.last_event_at,
        session_count: r.session_count,
        solution_count: r.solution_count,
        created_at: r.created_at,
    }
}

#[utoipa::path(
    get, path = "/v1/repositories", operation_id = "list_repositories", tag = "repositories",
    responses((status = 200, description = "Repositories the caller can access", body = RepositoryList)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn list(State(state): State<AppState>, auth: Auth) -> ApiResult<Json<RepositoryList>> {
    let scope = auth.scope(&state).await?;
    let ids = scope.readable_ids();
    let rows = if auth.0.is_admin() {
        repos::list_all(&state.pool, scope.organization_id).await?
    } else {
        repos::list_in_scope(&state.pool, scope.organization_id, &ids).await?
    };
    let all_ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let comps = repos::components_for(&state.pool, &all_ids).await?;
    Ok(Json(RepositoryList {
        items: rows
            .into_iter()
            .map(|r| {
                let perm = scope
                    .repos
                    .get(&r.id)
                    .map(|a| a.permission)
                    .unwrap_or(Permission::Read);
                view(r, perm, &comps)
            })
            .collect(),
    }))
}

#[utoipa::path(
    post, path = "/v1/repositories", operation_id = "create_repository", tag = "repositories",
    request_body = CreateRepository,
    responses((status = 201, description = "Created", body = RepositoryView), (status = 200, description = "Already existed; reactivated and updated", body = RepositoryView), (status = 403, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<CreateRepository>,
) -> ApiResult<(StatusCode, Json<RepositoryView>)> {
    auth.require_admin()?;
    if body
        .url
        .as_deref()
        .is_some_and(|u| !(u.starts_with("https://") || u.starts_with("http://")) || u.len() > 500)
    {
        return Err(ApiError::invalid("url must be an http(s) URL"));
    }
    let (id, created) = repos::upsert(
        &state.pool,
        NewRepository {
            org_id: auth.0.org_id(),
            provider: body.provider,
            full_name: &body.full_name,
            url: body.url.as_deref(),
            provider_repo_id: body.provider_repo_id.as_deref(),
            default_branch: body.default_branch.as_deref(),
        },
    )
    .await?;
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        if created {
            "repository.created"
        } else {
            "repository.updated"
        },
        "repository",
        Some(id.to_string()),
        serde_json::json!({ "full_name": body.full_name, "provider": body.provider }),
    )
    .await?;
    let row = repos::get(&state.pool, auth.0.org_id(), id)
        .await?
        .ok_or_else(ApiError::internal)?;
    let comps = repos::components_for(&state.pool, &[id]).await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(view(row, Permission::Write, &comps)),
    ))
}

#[utoipa::path(
    post, path = "/v1/repositories/{id}/components", operation_id = "add_component", tag = "repositories",
    params(("id" = Uuid, Path)),
    request_body = CreateComponent,
    responses((status = 201, body = ComponentView), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn add_component(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    ApiJson(body): ApiJson<CreateComponent>,
) -> ApiResult<(StatusCode, Json<ComponentView>)> {
    auth.require_admin()?;
    if body.name.trim().is_empty()
        || body.name.len() > 200
        || body.path_glob.trim().is_empty()
        || body.path_glob.len() > 500
    {
        return Err(ApiError::invalid(
            "name and path_glob are required (<= 200/500 chars)",
        ));
    }
    globset::Glob::new(&body.path_glob)
        .map_err(|_| ApiError::invalid("path_glob is not a valid glob"))?;
    let cid = repos::add_component(
        &state.pool,
        auth.0.org_id(),
        id,
        &body.name,
        &body.path_glob,
        body.package_name.as_deref(),
        body.ecosystem.as_deref(),
    )
    .await?;
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "component.upserted",
        "repository",
        Some(id.to_string()),
        serde_json::json!({ "component": body.name }),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(ComponentView {
            id: cid,
            name: body.name,
            path_glob: body.path_glob,
            package_name: body.package_name,
            ecosystem: body.ecosystem,
        }),
    ))
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RepositoryPatch {
    /// `active` or `disconnected`. Disconnected repositories are excluded
    /// from every access scope (and therefore from search).
    pub status: String,
}

#[utoipa::path(
    patch, path = "/v1/repositories/{id}", operation_id = "update_repository", tag = "repositories",
    params(("id" = Uuid, Path)),
    request_body = RepositoryPatch,
    responses((status = 204), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn patch(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    ApiJson(body): ApiJson<RepositoryPatch>,
) -> ApiResult<StatusCode> {
    auth.require_admin()?;
    if body.status != "active" && body.status != "disconnected" {
        return Err(ApiError::invalid("status must be active or disconnected"));
    }
    if !repos::set_status(&state.pool, auth.0.org_id(), id, &body.status).await? {
        return Err(ApiError::not_found("repository"));
    }
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "repository.status_changed",
        "repository",
        Some(id.to_string()),
        serde_json::json!({ "status": body.status }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
