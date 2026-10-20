//! Principals, repository grants and API tokens.

use crate::auth::{self, Auth};
use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use antibody_core::api::ErrorBody;
use antibody_core::model::{Permission, PrincipalKind, Role, Scope};
use antibody_db::access::{self, GrantRow, PrincipalListRow};
use antibody_db::audit::{self, Actor};
use antibody_db::tokens::{self, NewToken, TokenRow};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[utoipa::path(get, path = "/v1/admin/principals", operation_id = "list_principals", tag = "admin", responses((status = 200, body = Vec<PrincipalListRow>)), security(("cookie" = []), ("bearer" = [])))]
pub async fn list_principals(
    State(state): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<Vec<PrincipalListRow>>> {
    auth.require_admin()?;
    Ok(Json(
        access::list_principals(&state.pool, auth.0.org_id()).await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePrincipal {
    pub kind: PrincipalKind,
    pub display_name: String,
    pub role: Role,
    /// Required for `user` principals that sign in to the dashboard.
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrincipalCreated {
    pub id: Uuid,
}

#[utoipa::path(post, path = "/v1/admin/principals", operation_id = "create_principal", tag = "admin", request_body = CreatePrincipal, responses((status = 201, body = PrincipalCreated)), security(("cookie" = []), ("bearer" = [])))]
pub async fn create_principal(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<CreatePrincipal>,
) -> ApiResult<(StatusCode, Json<PrincipalCreated>)> {
    auth.require_admin()?;
    let name = antibody_core::text::one_line(&body.display_name, 120);
    if name.is_empty() {
        return Err(ApiError::invalid("display_name is required"));
    }
    let hash = match (body.kind, &body.username, &body.password) {
        (PrincipalKind::User, Some(u), Some(p)) => {
            auth::validate_username(u)?;
            auth::validate_new_password(p)?;
            let p = p.clone();
            Some(
                tokio::task::spawn_blocking(move || auth::hash_password(&p))
                    .await
                    .map_err(|_| ApiError::internal())??,
            )
        }
        (PrincipalKind::User, _, _) => {
            return Err(ApiError::invalid(
                "user principals need username and password",
            ));
        }
        (PrincipalKind::Service, None, None) => None,
        (PrincipalKind::Service, _, _) => {
            return Err(ApiError::invalid(
                "service principals cannot have passwords",
            ));
        }
    };
    let mut tx = state.pool.begin().await?;
    let p =
        access::create_principal(&mut *tx, auth.0.org_id(), body.kind, body.role, &name).await?;
    if let (Some(h), Some(u)) = (hash, &body.username) {
        tokens::create_user(&mut *tx, auth.0.org_id(), p.id, u, &h)
            .await
            .map_err(|e| match e {
                antibody_db::DbError::Sqlx(sqlx::Error::Database(d)) if d.is_unique_violation() => {
                    ApiError::new(
                        antibody_core::ErrorCode::Conflict,
                        "username already exists",
                    )
                }
                other => other.into(),
            })?;
    }
    audit::record(
        &mut *tx,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "principal.created",
        "principal",
        Some(p.id.to_string()),
        serde_json::json!({ "kind": body.kind, "role": body.role, "name": name }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(PrincipalCreated { id: p.id })))
}

#[utoipa::path(get, path = "/v1/admin/grants", operation_id = "list_grants", tag = "admin", responses((status = 200, body = Vec<GrantRow>)), security(("cookie" = []), ("bearer" = [])))]
pub async fn list_grants(
    State(state): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<Vec<GrantRow>>> {
    auth.require_admin()?;
    Ok(Json(
        access::list_grants(&state.pool, auth.0.org_id()).await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantRequest {
    pub principal_id: Uuid,
    pub repository_id: Uuid,
    pub permission: Permission,
}

#[utoipa::path(put, path = "/v1/admin/grants", operation_id = "put_grant", tag = "admin", request_body = GrantRequest, responses((status = 204)), security(("cookie" = []), ("bearer" = [])))]
pub async fn put_grant(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<GrantRequest>,
) -> ApiResult<StatusCode> {
    auth.require_admin()?;
    access::grant_repository(
        &state.pool,
        auth.0.org_id(),
        body.principal_id,
        body.repository_id,
        body.permission,
    )
    .await?;
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "grant.upserted",
        "principal",
        Some(body.principal_id.to_string()),
        serde_json::json!({ "repository_id": body.repository_id, "permission": body.permission }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeGrantRequest {
    pub principal_id: Uuid,
    pub repository_id: Uuid,
}

#[utoipa::path(delete, path = "/v1/admin/grants", operation_id = "delete_grant", tag = "admin", request_body = RevokeGrantRequest, responses((status = 204), (status = 404, body = ErrorBody)), security(("cookie" = []), ("bearer" = [])))]
pub async fn delete_grant(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<RevokeGrantRequest>,
) -> ApiResult<StatusCode> {
    auth.require_admin()?;
    if !access::revoke_grant(
        &state.pool,
        auth.0.org_id(),
        body.principal_id,
        body.repository_id,
    )
    .await?
    {
        return Err(ApiError::not_found("grant"));
    }
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "grant.revoked",
        "principal",
        Some(body.principal_id.to_string()),
        serde_json::json!({ "repository_id": body.repository_id }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/v1/admin/tokens", operation_id = "list_tokens", tag = "admin", responses((status = 200, body = Vec<TokenRow>)), security(("cookie" = []), ("bearer" = [])))]
pub async fn list_tokens(
    State(state): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<Vec<TokenRow>>> {
    auth.require_admin()?;
    Ok(Json(
        tokens::list_tokens(&state.pool, auth.0.org_id()).await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateToken {
    pub principal_id: Uuid,
    pub name: String,
    pub scopes: Vec<Scope>,
    /// Optional repository allow-list (intersected with the principal's grants).
    pub repository_ids: Option<Vec<Uuid>>,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TokenCreated {
    pub id: Uuid,
    /// Shown once. Store it in a secret manager or the agent's environment.
    pub token: String,
    pub prefix: String,
}

#[utoipa::path(post, path = "/v1/admin/tokens", operation_id = "create_token", tag = "admin", request_body = CreateToken, responses((status = 201, body = TokenCreated)), security(("cookie" = []), ("bearer" = [])))]
pub async fn create_token(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<CreateToken>,
) -> ApiResult<(StatusCode, Json<TokenCreated>)> {
    auth.require_admin()?;
    let name = antibody_core::text::one_line(&body.name, 120);
    if name.is_empty() {
        return Err(ApiError::invalid("name is required"));
    }
    let expires_at = body
        .expires_in_days
        .map(|d| Utc::now() + chrono::Duration::days(d.clamp(1, 3650) as i64));
    let issued = tokens::issue_token(
        &state.pool,
        NewToken {
            org_id: auth.0.org_id(),
            principal_id: body.principal_id,
            name: &name,
            scopes: &body.scopes,
            repository_ids: body.repository_ids.as_deref(),
            expires_at,
            created_by: Some(auth.0.principal.id),
        },
    )
    .await?;
    audit::record(&state.pool, Some(auth.0.org_id()), &Actor::from_auth(&auth.0), "token.created", "token", Some(issued.id.to_string()), serde_json::json!({ "principal_id": body.principal_id, "scopes": body.scopes, "prefix": issued.prefix })).await?;
    Ok((
        StatusCode::CREATED,
        Json(TokenCreated {
            id: issued.id,
            token: issued.token,
            prefix: issued.prefix,
        }),
    ))
}

#[utoipa::path(delete, path = "/v1/admin/tokens/{id}", operation_id = "revoke_token", tag = "admin", params(("id" = Uuid, Path)), responses((status = 204), (status = 404, body = ErrorBody)), security(("cookie" = []), ("bearer" = [])))]
pub async fn revoke_token(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    auth.require_admin()?;
    if !tokens::revoke_token(&state.pool, auth.0.org_id(), id).await? {
        return Err(ApiError::not_found("active token"));
    }
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "token.revoked",
        "token",
        Some(id.to_string()),
        serde_json::json!({}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
