//! Antibody HTTP API and MCP server.

pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod error;
pub mod extract;
pub mod mcp;
pub mod net;
pub mod openapi;
pub mod routes;
pub mod state;

use crate::error::{ApiError, REQUEST_ID};
use crate::routes::*;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
pub use config::Config;
pub use state::AppState;
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Assign (or accept) a request id, expose it to error bodies and echo it.
async fn request_context(mut req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get(&X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if let Ok(v) = HeaderValue::from_str(&id) {
        req.headers_mut().insert(X_REQUEST_ID.clone(), v);
    }
    let mut resp = REQUEST_ID.scope(id.clone(), next.run(req)).await;
    if let Ok(v) = HeaderValue::from_str(&id) {
        resp.headers_mut().insert(X_REQUEST_ID.clone(), v);
    }
    resp
}

async fn not_found() -> ApiError {
    ApiError::not_found("route")
}

pub fn router(state: AppState) -> Router {
    let cfg = state.config.clone();

    let ingestion = Router::new()
        .route("/v1/sessions", post(sessions::ingest).get(sessions::list))
        .layer(DefaultBodyLimit::max(cfg.max_session_bytes));

    let webhooks = Router::new()
        .route("/v1/webhooks/github", post(webhooks::github))
        .route("/v1/webhooks/ci", post(webhooks::ci_failure))
        .layer(DefaultBodyLimit::max(cfg.max_webhook_bytes));

    let api = Router::new()
        .route("/v1/health/live", get(health::live))
        .route("/v1/health/ready", get(health::ready))
        .route(
            "/v1/sessions/{id}",
            get(sessions::status).delete(sessions::delete),
        )
        .route("/v1/sessions/{id}/transcript", get(sessions::transcript))
        .route("/v1/context/search", post(context::search))
        .route("/v1/solutions", get(context::list_solutions))
        .route(
            "/v1/solutions/{id}",
            get(context::get_solution).delete(context::delete_solution),
        )
        .route("/v1/solutions/{id}/state", post(context::change_state))
        .route("/v1/solutions/{id}/propagation", get(context::propagation))
        .route("/v1/outcomes", post(context::report_outcome))
        .route(
            "/v1/repositories",
            get(repositories::list).post(repositories::create),
        )
        .route(
            "/v1/repositories/{id}",
            axum::routing::patch(repositories::patch),
        )
        .route(
            "/v1/repositories/{id}/components",
            post(repositories::add_component),
        )
        .route(
            "/v1/hooks/user-prompt-submit",
            post(hooks::user_prompt_submit),
        )
        .route("/v1/auth/login", post(auth_routes::login))
        .route("/v1/auth/logout", post(auth_routes::logout))
        .route("/v1/auth/session", get(auth_routes::session))
        .route("/v1/overview", get(admin::overview))
        .route("/v1/activity/ci-events", get(admin::ci_events))
        .route("/v1/activity/retrievals", get(admin::retrievals))
        .route("/v1/admin/jobs", get(admin::list_jobs))
        .route("/v1/admin/jobs/{id}/retry", post(admin::retry_job))
        .route(
            "/v1/admin/principals",
            get(admin::list_principals).post(admin::create_principal),
        )
        .route(
            "/v1/admin/grants",
            get(admin::list_grants)
                .put(admin::put_grant)
                .delete(admin::delete_grant),
        )
        .route(
            "/v1/admin/tokens",
            get(admin::list_tokens).post(admin::create_token),
        )
        .route(
            "/v1/admin/tokens/{id}",
            axum::routing::delete(admin::revoke_token),
        )
        .route(
            "/v1/admin/settings",
            get(admin::get_settings).patch(admin::patch_settings),
        )
        .route("/v1/admin/webhook-secrets", post(admin::webhook_secrets))
        .route("/v1/admin/retention/sweep", post(admin::sweep))
        .route("/v1/admin/audit", get(admin::audit_log))
        .route("/v1/admin/export", get(admin::export))
        .route(
            "/v1/admin/workflow-improvements",
            get(admin::list_workflow).post(admin::create_workflow),
        )
        .route(
            "/v1/admin/workflow-improvements/{id}/transition",
            post(admin::transition_workflow),
        )
        .layer(DefaultBodyLimit::max(cfg.max_json_bytes));

    let mcp = Router::new()
        .nest_service("/mcp", mcp::service(state.clone()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            mcp::auth_middleware,
        ));

    Router::new()
        .merge(ingestion)
        .merge(webhooks)
        .merge(api)
        .merge(mcp)
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), net::rate_limit))
        .layer(middleware::from_fn_with_state(state.clone(), net::transport_guard))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(
            // Never log bodies, query strings or credentials.
            TraceLayer::new_for_http().make_span_with(|req: &Request| {
                let id = req.headers().get(&X_REQUEST_ID).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                tracing::info_span!("http", method = %req.method(), path = %req.uri().path(), request_id = %id)
            }),
        )
        .layer(SetSensitiveRequestHeadersLayer::new([
            axum::http::header::AUTHORIZATION,
            axum::http::header::COOKIE,
            HeaderName::from_static("x-hub-signature-256"),
            HeaderName::from_static("x-antibody-signature"),
            HeaderName::from_static("x-csrf-token"),
        ]))
        .layer(middleware::from_fn(request_context))
        .with_state(state)
}
