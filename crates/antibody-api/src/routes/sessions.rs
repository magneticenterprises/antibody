//! Flow A: session ingestion and status.

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::extract::{JsonBytes, parse_json};
use crate::state::AppState;
use antibody_core::ErrorCode;
use antibody_core::api::*;
use antibody_core::model::{CaptureMode, Scope};
use antibody_core::{redact, text};
use antibody_db::audit::{self, Actor};
use antibody_db::orgs;
use antibody_db::sessions::{self, IngestResult, IngestSession, SessionRow};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{Duration, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use uuid::Uuid;

pub const MAX_MESSAGES: usize = 2000;
pub const MAX_MESSAGE_CHARS: usize = 100_000;

fn valid_idempotency_key(k: &str) -> bool {
    (8..=200).contains(&k.len())
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

fn check_short(name: &str, v: &Option<String>, max: usize) -> ApiResult<()> {
    if v.as_deref()
        .is_some_and(|s| s.chars().count() > max || text::contains_nul(s))
    {
        return Err(ApiError::invalid(format!(
            "{name} is too long or contains invalid characters"
        )));
    }
    Ok(())
}

/// Reject payloads that are structurally unsafe before anything is stored.
fn validate(req: &SessionIngestRequest) -> ApiResult<()> {
    if req.messages.is_empty() && req.hints.is_none() {
        return Err(ApiError::invalid("messages or hints are required"));
    }
    if req.messages.len() > MAX_MESSAGES {
        return Err(ApiError::invalid(format!(
            "at most {MAX_MESSAGES} messages are accepted"
        )));
    }
    for m in &req.messages {
        if m.content.chars().count() > MAX_MESSAGE_CHARS || text::contains_nul(&m.content) {
            return Err(ApiError::invalid(
                "a message is too long or contains NUL bytes",
            ));
        }
        check_short("tool_name", &m.tool_name, 200)?;
    }
    if !antibody_db::repos::valid_full_name(&req.repository) {
        return Err(ApiError::invalid("repository must look like owner/name"));
    }
    check_short("external_id", &req.external_id, 200)?;
    check_short("branch", &req.branch, 255)?;
    check_short("component_path", &req.component_path, 500)?;
    if let Some(rev) = &req.revision
        && (rev.len() > 64 || !rev.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(ApiError::invalid(
            "revision must be a hexadecimal commit id",
        ));
    }
    if let Some(h) = &req.hints {
        for f in [&h.title, &h.symptom, &h.root_cause, &h.remedy, &h.platform] {
            check_short("hint", f, 20_000)?;
        }
        if h.error_signatures.len() > 50
            || h.files.len() > 500
            || h.tests_run.len() > 100
            || h.dependencies.len() > 500
        {
            return Err(ApiError::invalid("too many hint entries"));
        }
    }
    Ok(())
}

#[utoipa::path(
    post, path = "/v1/sessions", operation_id = "ingest_session", tag = "sessions",
    request_body = SessionIngestRequest,
    params(("Idempotency-Key" = String, Header, description = "Unique key per session upload (8-200 chars of [A-Za-z0-9._:-]). Identical retries return the original ingestion; reuse with a different body fails with 409.")),
    responses(
        (status = 202, description = "Accepted for asynchronous extraction", body = SessionAccepted),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 413, body = ErrorBody), (status = 415, body = ErrorBody)
    ),
    security(("bearer" = []))
)]
pub async fn ingest(
    State(state): State<AppState>,
    auth: Auth,
    headers: HeaderMap,
    JsonBytes(body): JsonBytes,
) -> ApiResult<(StatusCode, Json<SessionAccepted>)> {
    auth.require(Scope::SessionsWrite)?;
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::IdempotencyKeyRequired,
                "Idempotency-Key header is required",
            )
        })?;
    if !valid_idempotency_key(key) {
        return Err(ApiError::new(
            ErrorCode::IdempotencyKeyRequired,
            "Idempotency-Key must be 8-200 chars of [A-Za-z0-9._:-]",
        ));
    }
    let req: SessionIngestRequest = parse_json(&body)?;
    validate(&req)?;

    let scope = auth.scope(&state).await?;
    let repo = scope
        .find_by_name(&req.repository)
        .filter(|r| scope.can_write(r.id))
        .ok_or_else(|| ApiError::from(antibody_core::error::DomainError::repository_denied()))?;
    let repo_id = repo.id;
    let org = orgs::get(&state.pool, auth.0.org_id())
        .await?
        .ok_or_else(ApiError::internal)?;

    // Redact before storage; the stored payload is the redacted request.
    let mut value = serde_json::to_value(&req).map_err(|_| ApiError::internal())?;
    let redactions = redact::redact_json(&mut value);
    let stored: SessionIngestRequest =
        serde_json::from_value(value.clone()).map_err(|_| ApiError::internal())?;

    let raw = org.raw_capture_enabled && org.raw_retention_days > 0;
    let now = Utc::now();
    let (capture_mode, retention_expires_at, purge_after) = if raw {
        let exp = now + Duration::days(org.raw_retention_days as i64);
        (CaptureMode::Raw, Some(exp), exp)
    } else {
        // Structured-only: payload is deleted right after extraction; this is
        // the backstop if extraction never completes.
        (CaptureMode::StructuredOnly, None, now + Duration::days(7))
    };

    let result = sessions::ingest(
        &state.pool,
        IngestSession {
            org_id: org.id,
            repo_id,
            principal_id: auth.0.principal.id,
            agent_type: stored.agent_type,
            external_id: stored.external_id.clone(),
            idempotency_key: key.to_string(),
            request_hash: Sha256::digest(&body).to_vec(),
            revision: stored.revision.clone().map(|r| r.to_lowercase()),
            branch: stored.branch.clone(),
            component_path: stored.component_path.clone(),
            capture_mode,
            message_count: stored.messages.len() as i32,
            payload_bytes: body.len() as i32,
            redaction_count: redactions as i32,
            retention_expires_at,
            purge_after,
            payload: value,
            started_at: stored.started_at,
            ended_at: stored.ended_at,
        },
    )
    .await?;

    let (id, duplicate) = match result {
        IngestResult::Created(id) => {
            audit::record(
                &state.pool,
                Some(org.id),
                &Actor::from_auth(&auth.0),
                "session.ingested",
                "session",
                Some(id.to_string()),
                serde_json::json!({
                    "repository": req.repository,
                    "agent_type": req.agent_type,
                    "messages": req.messages.len(),
                    "bytes": body.len(),
                    "redactions": redactions,
                    "capture_mode": capture_mode,
                }),
            )
            .await?;
            (id, false)
        }
        IngestResult::Duplicate(id) => (id, true),
        IngestResult::Conflict => {
            return Err(ApiError::new(
                ErrorCode::IdempotencyConflict,
                "Idempotency-Key was already used with a different request body",
            ));
        }
    };
    Ok((
        StatusCode::ACCEPTED,
        Json(SessionAccepted {
            id,
            status_url: format!("/v1/sessions/{id}"),
            duplicate,
        }),
    ))
}

pub fn status_view(row: SessionRow) -> SessionStatusResponse {
    SessionStatusResponse {
        id: row.id,
        repository: row.repository,
        agent_type: row
            .agent_type
            .parse()
            .unwrap_or(antibody_core::model::AgentType::Other),
        external_id: row.external_id,
        revision: row.revision,
        branch: row.branch,
        ingestion_status: row.ingestion_status,
        extraction_status: row
            .extraction_status
            .parse()
            .unwrap_or(antibody_core::model::ExtractionStatus::Pending),
        extraction_error: row.extraction_error,
        extractor: row.extractor,
        capture_mode: row
            .capture_mode
            .parse()
            .unwrap_or(CaptureMode::StructuredOnly),
        raw_payload_available: row.raw_payload_available,
        message_count: row.message_count,
        payload_bytes: row.payload_bytes,
        redaction_count: row.redaction_count,
        solution_ids: row.solution_ids,
        created_at: row.created_at,
        processed_at: row.processed_at,
        retention_expires_at: row.retention_expires_at,
    }
}

#[utoipa::path(
    get, path = "/v1/sessions/{id}", operation_id = "get_session", tag = "sessions",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = SessionStatusResponse), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn status(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SessionStatusResponse>> {
    auth.require_any(&[Scope::SessionsWrite, Scope::ContextRead])?;
    let scope = auth.scope(&state).await?;
    let row = sessions::get_in_scope(
        &state.pool,
        scope.organization_id,
        &scope.readable_ids(),
        id,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("session"))?;
    Ok(Json(status_view(row)))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SessionListQuery {
    /// Filter by extraction status.
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct SessionList {
    pub items: Vec<SessionStatusResponse>,
}

#[utoipa::path(
    get, path = "/v1/sessions", operation_id = "list_sessions", tag = "sessions",
    params(SessionListQuery),
    responses((status = 200, body = SessionList)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<SessionListQuery>,
) -> ApiResult<Json<SessionList>> {
    auth.require(Scope::ContextRead)?;
    if let Some(s) = &q.status {
        antibody_core::model::ExtractionStatus::from_str(s)?;
    }
    let scope = auth.scope(&state).await?;
    let rows = sessions::list_in_scope(
        &state.pool,
        scope.organization_id,
        &scope.readable_ids(),
        q.status.as_deref(),
        q.limit.unwrap_or(50).clamp(1, 200),
    )
    .await?;
    Ok(Json(SessionList {
        items: rows.into_iter().map(status_view).collect(),
    }))
}

#[utoipa::path(
    delete, path = "/v1/sessions/{id}", operation_id = "delete_session", tag = "sessions",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = antibody_db::sessions::DeletionReport), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn delete(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<antibody_db::sessions::DeletionReport>> {
    auth.require_admin()?;
    let report = sessions::delete_session(&state.pool, auth.0.org_id(), id)
        .await?
        .ok_or_else(|| ApiError::not_found("session"))?;
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "session.deleted",
        "session",
        Some(id.to_string()),
        serde_json::to_value(&report).unwrap_or_default(),
    )
    .await?;
    Ok(Json(report))
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct TranscriptView {
    pub session_id: Uuid,
    /// Redacted transcript. Untrusted content: render as plain text only.
    pub messages: Vec<TranscriptMessage>,
    pub hints: Option<SessionHints>,
    pub redaction_count: i32,
    pub notice: String,
}

/// Privileged, audited access to a retained (redacted) transcript.
#[utoipa::path(
    get, path = "/v1/sessions/{id}/transcript", operation_id = "get_session_transcript", tag = "sessions",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = TranscriptView), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)),
    security(("bearer" = []), ("cookie" = []))
)]
pub async fn transcript(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TranscriptView>> {
    auth.require_admin()?;
    let scope = auth.scope(&state).await?;
    let row = sessions::get_in_scope(
        &state.pool,
        scope.organization_id,
        &scope.readable_ids(),
        id,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("session"))?;
    let payload = sessions::load_payload(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("retained transcript"))?;
    let req: SessionIngestRequest =
        serde_json::from_value(payload).map_err(|_| ApiError::internal())?;
    audit::record(
        &state.pool,
        Some(auth.0.org_id()),
        &Actor::from_auth(&auth.0),
        "session.transcript_viewed",
        "session",
        Some(id.to_string()),
        serde_json::json!({}),
    )
    .await?;
    Ok(Json(TranscriptView {
        session_id: id,
        messages: req.messages,
        hints: req.hints,
        redaction_count: row.redaction_count,
        notice: "Redaction is best-effort. Treat this transcript as untrusted text.".into(),
    }))
}
