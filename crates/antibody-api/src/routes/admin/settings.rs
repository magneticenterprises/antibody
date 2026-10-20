//! Organization settings, webhook secrets, retention, audit log and export.

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use antibody_db::audit::{self, Actor, AuditEventView};
use antibody_db::orgs::{self, Organization, SettingsPatch};
use antibody_db::retention::{self, SweepReport};
use axum::Json;
use axum::extract::{Query, State};
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SettingsView {
    pub organization: Organization,
    /// Configured model providers (no secrets).
    pub providers: serde_json::Value,
    pub github_webhook_url: String,
    pub ci_webhook_url: String,
    pub mcp_url: String,
}

fn settings_view(state: &AppState, org: Organization) -> SettingsView {
    let base = &state.config.public_url;
    SettingsView {
        github_webhook_url: format!("{base}/v1/webhooks/github?org={}", org.slug),
        ci_webhook_url: format!("{base}/v1/webhooks/ci"),
        mcp_url: format!("{base}/mcp"),
        organization: org,
        providers: state.providers.describe(),
    }
}

#[utoipa::path(get, path = "/v1/admin/settings", operation_id = "get_settings", tag = "admin", responses((status = 200, body = SettingsView)), security(("cookie" = []), ("bearer" = [])))]
pub async fn get_settings(
    State(state): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<SettingsView>> {
    auth.require_admin()?;
    let org = orgs::get(&state.pool, auth.0.org_id())
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok(Json(settings_view(&state, org)))
}

#[utoipa::path(patch, path = "/v1/admin/settings", operation_id = "update_settings", tag = "admin", request_body = SettingsPatch, responses((status = 200, body = SettingsView)), security(("cookie" = []), ("bearer" = [])))]
pub async fn patch_settings(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<SettingsPatch>,
) -> ApiResult<Json<SettingsView>> {
    auth.require_admin()?;
    let org =
        orgs::update_settings(&state.pool, auth.0.org_id(), auth.0.principal.id, &body).await?;
    audit::record(
        &state.pool,
        Some(org.id),
        &Actor::from_auth(&auth.0),
        "settings.updated",
        "organization",
        Some(org.id.to_string()),
        serde_json::json!({
            "raw_capture_enabled": body.raw_capture_enabled,
            "raw_retention_days": body.raw_retention_days,
            "external_providers_approved": body.external_providers_approved,
            "ci_cross_repository_search": body.ci_cross_repository_search,
        }),
    )
    .await?;
    Ok(Json(settings_view(&state, org)))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookSecretRequest {
    /// Rotate before revealing; old secrets stop working immediately.
    #[serde(default)]
    pub rotate: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookSecrets {
    pub version: i32,
    pub github_secret: String,
    pub ci_secret: String,
    pub github_webhook_url: String,
    pub ci_webhook_url: String,
}

/// Reveal (and optionally rotate) webhook secrets. Audited.
#[utoipa::path(post, path = "/v1/admin/webhook-secrets", operation_id = "reveal_webhook_secrets", tag = "admin", request_body = WebhookSecretRequest, responses((status = 200, body = WebhookSecrets)), security(("cookie" = []), ("bearer" = [])))]
pub async fn webhook_secrets(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<WebhookSecretRequest>,
) -> ApiResult<Json<WebhookSecrets>> {
    auth.require_admin()?;
    let org_id = auth.0.org_id();
    let version = if body.rotate {
        orgs::rotate_webhook_secret(&state.pool, org_id).await?
    } else {
        orgs::get(&state.pool, org_id)
            .await?
            .ok_or_else(ApiError::internal)?
            .webhook_secret_version
    };
    let org = orgs::get(&state.pool, org_id)
        .await?
        .ok_or_else(ApiError::internal)?;
    audit::record(
        &state.pool,
        Some(org_id),
        &Actor::from_auth(&auth.0),
        if body.rotate {
            "webhook_secret.rotated"
        } else {
            "webhook_secret.revealed"
        },
        "organization",
        Some(org_id.to_string()),
        serde_json::json!({ "version": version }),
    )
    .await?;
    let v = settings_view(&state, org);
    Ok(Json(WebhookSecrets {
        version,
        github_secret: state.github_webhook_secret(org_id, version),
        ci_secret: state.ci_webhook_secret(org_id, version),
        github_webhook_url: v.github_webhook_url,
        ci_webhook_url: v.ci_webhook_url,
    }))
}

#[utoipa::path(post, path = "/v1/admin/retention/sweep", operation_id = "run_retention_sweep", tag = "admin", responses((status = 200, body = SweepReport)), security(("cookie" = []), ("bearer" = [])))]
pub async fn sweep(State(state): State<AppState>, auth: Auth) -> ApiResult<Json<SweepReport>> {
    auth.require_admin()?;
    let report = retention::sweep(&state.pool).await?;
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "retention.swept",
        "organization",
        None,
        serde_json::to_value(&report).unwrap_or_default(),
    )
    .await?;
    Ok(Json(report))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct AuditQuery {
    /// Action prefix, e.g. `session.` or `token.`.
    pub action: Option<String>,
    pub before_id: Option<i64>,
    pub limit: Option<i64>,
}

#[utoipa::path(get, path = "/v1/admin/audit", operation_id = "list_audit_events", tag = "admin", params(AuditQuery), responses((status = 200, body = Vec<AuditEventView>)), security(("cookie" = []), ("bearer" = [])))]
pub async fn audit_log(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEventView>>> {
    auth.require_admin()?;
    let prefix = q.action.as_deref().filter(|a| {
        a.len() <= 64
            && a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
    });
    Ok(Json(
        audit::list(
            &state.pool,
            auth.0.org_id(),
            prefix,
            q.limit.unwrap_or(100).clamp(1, 500),
            q.before_id,
        )
        .await?,
    ))
}

/// Export the organization's knowledge and metadata as JSON (no raw
/// transcripts, no credentials).
#[utoipa::path(get, path = "/v1/admin/export", operation_id = "export_organization", tag = "admin", responses((status = 200, body = serde_json::Value)), security(("cookie" = []), ("bearer" = [])))]
pub async fn export(
    State(state): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let org = auth.0.org_id();
    let q = |sql: &'static str| {
        sqlx::query_scalar::<_, serde_json::Value>(sql)
            .bind(org)
            .fetch_one(&state.pool)
    };
    let repositories = q("SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY r.full_name), '[]') FROM repositories r WHERE organization_id = $1").await?;
    let solutions = q("SELECT COALESCE(jsonb_agg(to_jsonb(s) - 'search_tsv' ORDER BY s.created_at), '[]') FROM solutions s WHERE organization_id = $1").await?;
    let evidence = q("SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY e.created_at), '[]') FROM evidence e WHERE organization_id = $1").await?;
    let sessions = q("SELECT COALESCE(jsonb_agg(to_jsonb(s) - 'request_hash' - 'idempotency_key' ORDER BY s.created_at), '[]') FROM sessions s WHERE organization_id = $1").await?;
    let outcomes = q("SELECT COALESCE(jsonb_agg(to_jsonb(o) ORDER BY o.created_at), '[]') FROM outcomes o WHERE organization_id = $1").await?;
    let workflow = q("SELECT COALESCE(jsonb_agg(to_jsonb(w) ORDER BY w.created_at), '[]') FROM workflow_improvements w WHERE organization_id = $1").await?;
    let audit_events = q("SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id), '[]') FROM audit_events a WHERE organization_id = $1").await?;
    audit::record(
        &state.pool,
        Some(org),
        &Actor::from_auth(&auth.0),
        "organization.exported",
        "organization",
        Some(org.to_string()),
        serde_json::json!({}),
    )
    .await?;
    Ok(Json(serde_json::json!({
        "format": "antibody-export-v1",
        "exported_at": Utc::now(),
        "repositories": repositories,
        "solutions": solutions,
        "evidence": evidence,
        "sessions": sessions,
        "outcomes": outcomes,
        "workflow_improvements": workflow,
        "audit_events": audit_events,
    })))
}
