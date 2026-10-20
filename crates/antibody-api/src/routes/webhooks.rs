//! Signed Git/CI webhooks (Flows C and D). Signatures are verified against
//! the raw request bytes before any parsing; authenticated payload hashes prevent replay.

use crate::error::{ApiError, ApiResult};
use crate::routes::context::external_approved;
use crate::state::AppState;
use antibody_core::ErrorCode;
use antibody_core::api::*;
use antibody_core::model::{JobKind, RepositoryProvider, RetrievalChannel};
use antibody_core::{UNTRUSTED_NOTICE, normalize, redact, text};
use antibody_db::jobs::{self, NewJob};
use antibody_db::orgs::Organization;
use antibody_db::{ci, orgs, repos, webhooks};
use antibody_integrations::github::{self, GitEvent};
use antibody_integrations::{ci as ci_sig, signing};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;

fn header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn bad_signature() -> ApiError {
    ApiError::new(ErrorCode::InvalidSignature, "signature verification failed")
}

fn valid_delivery_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Resolve the organization a webhook targets. Unknown organizations are
/// indistinguishable from bad signatures.
async fn resolve_org(state: &AppState, slug: Option<&str>) -> ApiResult<Organization> {
    match slug {
        Some(s) => orgs::get_by_slug(&state.pool, s)
            .await?
            .ok_or_else(bad_signature),
        None => {
            let all = orgs::list(&state.pool).await?;
            if all.len() == 1 {
                Ok(all.into_iter().next().expect("one org"))
            } else {
                Err(bad_signature())
            }
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct WebhookQuery {
    /// Organization slug (optional when the server hosts one organization).
    pub org: Option<String>,
}

/// Redact all retained event fields after authentication and before durable writes.
fn sanitized<T: serde::Serialize + serde::de::DeserializeOwned>(value: T) -> ApiResult<T> {
    let mut value = serde_json::to_value(value).map_err(|e| ApiError::invalid(e.to_string()))?;
    redact::redact_json(&mut value);
    serde_json::from_value(value).map_err(|e| ApiError::invalid(e.to_string()))
}

#[utoipa::path(
    post, path = "/v1/webhooks/github", operation_id = "github_webhook", tag = "webhooks",
    params(WebhookQuery,
        ("X-GitHub-Event" = String, Header), ("X-GitHub-Delivery" = String, Header),
        ("X-Hub-Signature-256" = String, Header, description = "sha256 HMAC of the raw body with the organization's webhook secret")),
    request_body(content = serde_json::Value, description = "GitHub webhook payload"),
    responses(
        (status = 202, body = WebhookAck), (status = 200, body = WebhookAck),
        (status = 401, body = ErrorBody), (status = 409, description = "Replayed delivery", body = ErrorBody), (status = 413, body = ErrorBody)
    )
)]
pub async fn github(
    State(state): State<AppState>,
    Query(q): Query<WebhookQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<WebhookAck>)> {
    let org = resolve_org(&state, q.org.as_deref()).await?;
    let sig = header(&headers, "x-hub-signature-256").ok_or_else(bad_signature)?;
    let secret = state.github_webhook_secret(org.id, org.webhook_secret_version);
    if !signing::verify_prefixed(secret.as_bytes(), &body, sig) {
        return Err(bad_signature());
    }
    let event_name = header(&headers, "x-github-event")
        .ok_or_else(|| ApiError::invalid("X-GitHub-Event header is required"))?
        .to_string();
    let delivery = header(&headers, "x-github-delivery")
        .filter(|d| valid_delivery_id(d))
        .ok_or_else(|| ApiError::invalid("valid X-GitHub-Delivery header is required"))?
        .to_string();
    let event = sanitized(
        github::parse_event(&event_name, &body).map_err(|e| ApiError::invalid(e.to_string()))?,
    )?;

    let ack = |accepted: bool, detail: Option<String>| WebhookAck {
        accepted,
        duplicate: false,
        event: event_name.clone(),
        detail,
    };
    if matches!(event, GitEvent::Ping) {
        return Ok((StatusCode::OK, Json(ack(true, Some("pong".into())))));
    }
    let repo = match event.repo() {
        Some(r) => {
            let previous = match &event {
                GitEvent::RepositoryChanged {
                    previous_full_name, ..
                } => previous_full_name.as_deref(),
                _ => None,
            };
            repos::find_for_webhook(
                &state.pool,
                org.id,
                RepositoryProvider::Github,
                r.provider_id.as_deref(),
                &r.full_name,
                previous,
                matches!(event, GitEvent::RepositoryChanged { .. }),
            )
            .await?
        }
        None => None,
    };
    // Ignored payloads remain deliverable after registration, and an unsigned
    // unsupported event header cannot consume the signed body's replay key.
    if matches!(event, GitEvent::Unsupported { .. }) {
        return Ok((
            StatusCode::OK,
            Json(ack(false, Some("event type not used by Antibody".into()))),
        ));
    }
    if repo.is_none() && !matches!(event, GitEvent::Ping) {
        return Ok((
            StatusCode::OK,
            Json(ack(
                false,
                Some("repository is not registered with Antibody".into()),
            )),
        ));
    }
    let job = match (&event, &repo) {
        (GitEvent::Ping | GitEvent::Unsupported { .. }, _) => None,
        (_, None) => None,
        (_, Some(repo)) => Some(
            NewJob::new(
                JobKind::ProcessGitEvent,
                format!("github:{}:{}", org.id, webhooks::payload_hash(&body)),
                serde_json::json!({ "org_id": org.id, "repo_id": repo.id, "delivery_id": delivery, "event": event }),
            )
            .org(org.id),
        ),
    };
    let has_job = job.is_some();
    let mut tx = state.pool.begin().await?;
    if !webhooks::record_delivery(
        &mut tx,
        "github",
        &delivery,
        org.id,
        &event_name,
        &webhooks::payload_hash(&body),
    )
    .await?
    {
        return Err(ApiError::new(
            ErrorCode::ReplayedDelivery,
            "delivery was already processed",
        ));
    }
    if let Some(r) = &repo {
        repos::bind_provider_id(
            &mut tx,
            org.id,
            r.id,
            event.repo().and_then(|r| r.provider_id.as_deref()),
        )
        .await?;
        sqlx::query("UPDATE repositories SET last_event_at = now() WHERE id = $1")
            .bind(r.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(mut job) = job {
        let received_at: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT transaction_timestamp()")
                .fetch_one(&mut *tx)
                .await?;
        job.payload["received_at"] = serde_json::json!(received_at);
        redact::redact_json(&mut job.payload);
        jobs::enqueue(&mut *tx, job).await?;
    }
    tx.commit().await?;
    Ok(match (&event, repo, has_job) {
        (GitEvent::Ping, _, _) => (StatusCode::OK, Json(ack(true, Some("pong".into())))),
        (GitEvent::Unsupported { .. }, _, _) => (
            StatusCode::OK,
            Json(ack(false, Some("event type not used by Antibody".into()))),
        ),
        (_, None, _) => (
            StatusCode::OK,
            Json(ack(
                false,
                Some("repository is not registered with Antibody".into()),
            )),
        ),
        (_, Some(_), _) => (StatusCode::ACCEPTED, Json(ack(true, None))),
    })
}

#[utoipa::path(
    post, path = "/v1/webhooks/ci", operation_id = "ci_failure_webhook", tag = "webhooks",
    params(
        ("X-Antibody-Org" = String, Header), ("X-Antibody-Delivery" = String, Header),
        ("X-Antibody-Timestamp" = String, Header, description = "Unix seconds; must be within 5 minutes"),
        ("X-Antibody-Signature" = String, Header, description = "sha256= HMAC over `{timestamp}.{raw body}` with the organization's CI secret")),
    request_body = CiFailureEvent,
    responses((status = 200, description = "Prior diagnoses; never blocks CI", body = CiLookupResponse), (status = 401, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn ci_failure(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<CiLookupResponse>> {
    let org = resolve_org(&state, header(&headers, "x-antibody-org")).await?;
    let ts = header(&headers, "x-antibody-timestamp").ok_or_else(bad_signature)?;
    let sig = header(&headers, "x-antibody-signature").ok_or_else(bad_signature)?;
    let secret = state.ci_webhook_secret(org.id, org.webhook_secret_version);
    ci_sig::verify(
        secret.as_bytes(),
        ts,
        &body,
        sig,
        chrono::Utc::now().timestamp(),
    )
    .map_err(|_| bad_signature())?;
    let delivery = header(&headers, "x-antibody-delivery")
        .filter(|d| valid_delivery_id(d))
        .ok_or_else(|| ApiError::invalid("valid X-Antibody-Delivery header is required"))?
        .to_string();
    let event: CiFailureEvent = sanitized(crate::extract::parse_json::<CiFailureEvent>(&body)?)?;
    if event.check_name.trim().is_empty()
        || event.check_name.len() > 200
        || event.revision.len() > 64
        || event.revision.is_empty()
    {
        return Err(ApiError::invalid("check_name and revision are required"));
    }
    let repo = repos::find_any_provider(&state.pool, org.id, &event.repository)
        .await?
        .ok_or_else(|| ApiError::not_found("repository"))?;
    let mut tx = state.pool.begin().await?;
    if !webhooks::record_delivery(
        &mut tx,
        "ci",
        &delivery,
        org.id,
        "ci_failure",
        &webhooks::payload_hash(&body),
    )
    .await?
    {
        return Err(ApiError::new(
            ErrorCode::ReplayedDelivery,
            "delivery was already processed",
        ));
    }
    sqlx::query("UPDATE repositories SET last_event_at = now() WHERE id = $1")
        .bind(repo.id)
        .execute(&mut *tx)
        .await?;

    let excerpt = event
        .excerpt
        .as_deref()
        .map(|e| redact::redact(&text::clean(e, 4000)).text)
        .filter(|e| !e.is_empty());
    let signature = event
        .signature
        .as_deref()
        .map(normalize::normalize_error_signature)
        .filter(|s| s.len() >= 6)
        .or_else(|| {
            excerpt
                .as_deref()
                .and_then(|e| normalize::extract_error_signatures(e, 1).into_iter().next())
        })
        .unwrap_or_else(|| {
            normalize::normalize_error_signature(&format!("{} failed", event.check_name))
        });
    let (ci_id, duplicate) = ci::insert_in(
        &mut tx,
        ci::NewCiEvent {
            org_id: org.id,
            repo_id: repo.id,
            source: "generic",
            revision: &event.revision.to_lowercase(),
            check_name: &event.check_name,
            conclusion: "failure",
            signature: &signature,
            excerpt: excerpt.as_deref(),
            url: event.url.as_deref().filter(|u| u.len() <= 500),
            component_path: event.component_path.as_deref(),
        },
    )
    .await?;
    tx.commit().await?;
    if duplicate {
        return Ok(Json(CiLookupResponse {
            ci_event_id: ci_id,
            duplicate: true,
            signature,
            suggestions: vec![],
            blocking: false,
            notice: "Duplicate event (same repository, revision, check and signature); see the dashboard for earlier results.".into(),
        }));
    }
    let scope =
        antibody_db::access::ci_scope(&state.pool, org.id, repo.id, org.ci_cross_repository_search)
            .await?;
    let req = ContextSearchRequest {
        task: format!("CI check `{}` failed: {}", event.check_name, signature),
        repository: Some(repo.full_name.clone()),
        revision: Some(event.revision.clone()),
        error: excerpt.clone().or(Some(signature.clone())),
        component_path: event.component_path.clone(),
        ..Default::default()
    };
    let approved = external_approved(&state, org.id).await?;
    let resp = state
        .search
        .search(&scope, &req, RetrievalChannel::Ci, approved)
        .await?;
    ci::set_result(
        &state.pool,
        ci_id,
        Some(resp.retrieval_id),
        resp.suggestions.len() as i32,
        "searched",
    )
    .await?;
    Ok(Json(CiLookupResponse {
        ci_event_id: ci_id,
        duplicate: false,
        signature,
        suggestions: resp.suggestions,
        blocking: false,
        notice: UNTRUSTED_NOTICE.into(),
    }))
}
