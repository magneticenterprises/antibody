//! Browser login backed by server-side sessions (HttpOnly cookies).

use crate::auth::{self, Auth};
use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::net::client_ip;
use crate::state::AppState;
use antibody_core::ErrorCode;
use antibody_core::api::ErrorBody;
use antibody_db::access::AuthMethod;
use antibody_db::audit::{self, Actor};
use antibody_db::{orgs, tokens};
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use uuid::Uuid;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SessionInfo {
    pub principal_id: Uuid,
    pub display_name: String,
    pub username: Option<String>,
    pub role: antibody_core::model::Role,
    pub organization_id: Uuid,
    pub organization_slug: String,
    pub organization_name: String,
    /// Send as `X-CSRF-Token` on mutating requests (browser sessions only).
    pub csrf_token: Option<String>,
    pub scopes: Vec<antibody_core::model::Scope>,
}

fn session_cookie(state: &AppState, value: &str, max_age_secs: u64) -> HeaderValue {
    let mut c = format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}",
        state.config.cookie_name()
    );
    if state.config.cookie_secure {
        c.push_str("; Secure");
    }
    HeaderValue::from_str(&c).expect("cookie header is ASCII")
}

async fn info(state: &AppState, ctx: &antibody_db::access::AuthContext) -> ApiResult<SessionInfo> {
    let org = orgs::get(&state.pool, ctx.org_id())
        .await?
        .ok_or_else(ApiError::internal)?;
    let (username, csrf) = match &ctx.method {
        AuthMethod::BrowserSession {
            user_id,
            csrf_token,
        } => (
            tokens::username_for(&state.pool, *user_id).await?,
            Some(csrf_token.clone()),
        ),
        AuthMethod::Token { .. } => (None, None),
    };
    Ok(SessionInfo {
        principal_id: ctx.principal.id,
        display_name: ctx.principal.display_name.clone(),
        username,
        role: ctx.principal.role,
        organization_id: org.id,
        organization_slug: org.slug,
        organization_name: org.name,
        csrf_token: csrf,
        scopes: ctx.scopes.clone(),
    })
}

#[utoipa::path(
    post, path = "/v1/auth/login", operation_id = "login", tag = "auth",
    request_body = LoginRequest,
    responses((status = 200, body = SessionInfo, description = "Sets an HttpOnly session cookie"), (status = 401, body = ErrorBody), (status = 429, body = ErrorBody))
)]
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LoginRequest>,
) -> ApiResult<Response> {
    let ip = client_ip(&headers, Some(peer), state.config.trust_proxy)
        .map(|i| i.to_string())
        .unwrap_or_default();
    if state.limiters.login.check_key(&ip).is_err() {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            "too many login attempts; retry later",
        ));
    }
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        && origin != state.config.public_origin()
    {
        return Err(ApiError::new(
            ErrorCode::CsrfFailed,
            "cross-origin login rejected",
        ));
    }
    if body.username.len() > 128 || body.password.len() > 256 {
        return Err(ApiError::unauthenticated());
    }
    let creds = tokens::find_user_credentials(&state.pool, &body.username).await?;
    let Some(creds) = creds else {
        auth::dummy_verify(&body.password);
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "invalid username or password",
        ));
    };
    let password = body.password.clone();
    let hash = creds.password_hash.clone();
    let ok = tokio::task::spawn_blocking(move || auth::verify_password(&password, &hash))
        .await
        .unwrap_or(false);
    if !ok {
        audit::record(
            &state.pool,
            Some(creds.organization_id),
            &Actor {
                principal_id: Some(creds.principal_id),
                label: format!("user:{}", body.username),
            },
            "auth.login_failed",
            "user",
            Some(creds.user_id.to_string()),
            serde_json::json!({ "ip": ip }),
        )
        .await?;
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "invalid username or password",
        ));
    }
    let ttl =
        chrono::Duration::from_std(state.config.session_ttl).unwrap_or(chrono::Duration::hours(12));
    let session = tokens::create_web_session(&state.pool, &creds, ttl).await?;
    audit::record(
        &state.pool,
        Some(creds.organization_id),
        &Actor {
            principal_id: Some(creds.principal_id),
            label: format!("user:{}", body.username),
        },
        "auth.login",
        "user",
        Some(creds.user_id.to_string()),
        serde_json::json!({ "ip": ip }),
    )
    .await?;
    let ctx = tokens::authenticate_web_session(&state.pool, &session.session_id)
        .await?
        .ok_or_else(ApiError::internal)?;
    let info = info(&state, &ctx).await?;
    let mut resp = Json(info).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(
            &state,
            &session.session_id,
            state.config.session_ttl.as_secs(),
        ),
    );
    Ok(resp)
}

#[utoipa::path(
    post, path = "/v1/auth/logout", operation_id = "logout", tag = "auth",
    responses((status = 204)),
    security(("cookie" = []))
)]
pub async fn logout(
    State(state): State<AppState>,
    auth: Auth,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Some(sid) = auth::cookie_value(&headers, state.config.cookie_name()) {
        tokens::delete_web_session(&state.pool, sid).await?;
    }
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "auth.logout",
        "principal",
        Some(auth.0.principal.id.to_string()),
        serde_json::json!({}),
    )
    .await?;
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, session_cookie(&state, "", 0));
    Ok(resp)
}

#[utoipa::path(
    get, path = "/v1/auth/session", operation_id = "get_auth_session", tag = "auth",
    responses((status = 200, body = SessionInfo), (status = 401, body = ErrorBody)),
    security(("cookie" = []), ("bearer" = []))
)]
pub async fn session(State(state): State<AppState>, auth: Auth) -> ApiResult<Json<SessionInfo>> {
    Ok(Json(info(&state, &auth.0).await?))
}
