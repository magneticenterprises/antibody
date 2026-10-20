//! Session ingestion storage. Session metadata, the redacted payload and
//! the extraction job are written in one transaction.

use crate::jobs::{self, NewJob};
use crate::{DbResult, new_id};
use antibody_core::model::{AgentType, CaptureMode, JobKind};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

pub struct IngestSession {
    pub org_id: Uuid,
    pub repo_id: Uuid,
    pub principal_id: Uuid,
    pub agent_type: AgentType,
    pub external_id: Option<String>,
    pub idempotency_key: String,
    pub request_hash: Vec<u8>,
    pub revision: Option<String>,
    pub branch: Option<String>,
    pub component_path: Option<String>,
    pub capture_mode: CaptureMode,
    pub message_count: i32,
    pub payload_bytes: i32,
    pub redaction_count: i32,
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub purge_after: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestResult {
    Created(Uuid),
    /// Identical retry: the original ingestion id.
    Duplicate(Uuid),
    /// Same idempotency key, different body.
    Conflict,
}

pub async fn ingest(pool: &PgPool, s: IngestSession) -> DbResult<IngestResult> {
    let mut tx = pool.begin().await?;
    let id = new_id();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO sessions (id, organization_id, repository_id, principal_id, agent_type, external_id,
                               idempotency_key, request_hash, revision, branch, component_path, capture_mode,
                               message_count, payload_bytes, redaction_count, retention_expires_at, started_at, ended_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
         ON CONFLICT (organization_id, idempotency_key) DO NOTHING
         RETURNING id",
    )
    .bind(id)
    .bind(s.org_id)
    .bind(s.repo_id)
    .bind(s.principal_id)
    .bind(s.agent_type.as_str())
    .bind(&s.external_id)
    .bind(&s.idempotency_key)
    .bind(&s.request_hash)
    .bind(&s.revision)
    .bind(&s.branch)
    .bind(&s.component_path)
    .bind(s.capture_mode.as_str())
    .bind(s.message_count)
    .bind(s.payload_bytes)
    .bind(s.redaction_count)
    .bind(s.retention_expires_at)
    .bind(s.started_at)
    .bind(s.ended_at)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(id) = inserted else {
        tx.rollback().await?;
        let existing: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
            "SELECT id, request_hash FROM sessions WHERE organization_id = $1 AND idempotency_key = $2",
        )
        .bind(s.org_id)
        .bind(&s.idempotency_key)
        .fetch_optional(pool)
        .await?;
        return Ok(match existing {
            Some((eid, hash)) if hash == s.request_hash => IngestResult::Duplicate(eid),
            Some(_) => IngestResult::Conflict,
            // Deleted between the conflict and the read; treat as conflict.
            None => IngestResult::Conflict,
        });
    };

    sqlx::query(
        "INSERT INTO session_payloads (session_id, organization_id, payload, purge_after) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(s.org_id)
    .bind(&s.payload)
    .bind(s.purge_after)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO source_artifacts (id, organization_id, repository_id, kind, external_id, revision, title, state)
         VALUES ($1, $2, $3, 'session', $4, $5, $6, 'ingested')
         ON CONFLICT (repository_id, kind, external_id) DO NOTHING",
    )
    .bind(new_id())
    .bind(s.org_id)
    .bind(s.repo_id)
    .bind(id.to_string())
    .bind(&s.revision)
    .bind(format!("{} session", s.agent_type))
    .execute(&mut *tx)
    .await?;

    jobs::enqueue(
        &mut *tx,
        NewJob::new(
            JobKind::ExtractSession,
            format!("session:{id}"),
            serde_json::json!({ "session_id": id }),
        )
        .org(s.org_id)
        .session(id),
    )
    .await?;

    tx.commit().await?;
    Ok(IngestResult::Created(id))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub repository_id: Uuid,
    pub repository: String,
    pub principal_id: Option<Uuid>,
    pub agent_type: String,
    pub external_id: Option<String>,
    pub revision: Option<String>,
    pub branch: Option<String>,
    pub component_path: Option<String>,
    pub capture_mode: String,
    pub ingestion_status: String,
    pub extraction_status: String,
    pub extraction_error: Option<String>,
    pub extractor: Option<String>,
    pub message_count: i32,
    pub payload_bytes: i32,
    pub redaction_count: i32,
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub raw_payload_available: bool,
    pub solution_ids: Vec<Uuid>,
}

const SELECT: &str = "SELECT s.id, s.organization_id, s.repository_id, r.full_name AS repository, s.principal_id,
        s.agent_type, s.external_id, s.revision, s.branch, s.component_path, s.capture_mode, s.ingestion_status,
        s.extraction_status, s.extraction_error, s.extractor, s.message_count, s.payload_bytes, s.redaction_count,
        s.retention_expires_at, s.created_at, s.processed_at,
        EXISTS (SELECT 1 FROM session_payloads p WHERE p.session_id = s.id) AS raw_payload_available,
        COALESCE((SELECT array_agg(ss.solution_id ORDER BY ss.created_at) FROM solution_sessions ss WHERE ss.session_id = s.id), '{}') AS solution_ids
   FROM sessions s JOIN repositories r ON r.id = s.repository_id";

/// Fetch a session only if its repository is in the caller's scope.
pub async fn get_in_scope(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    id: Uuid,
) -> DbResult<Option<SessionRow>> {
    let sql = format!(
        "{SELECT} WHERE s.id = $1 AND s.organization_id = $2 AND s.repository_id = ANY($3)"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(org_id)
        .bind(repo_ids)
        .fetch_optional(pool)
        .await?)
}

pub async fn get_system(pool: &PgPool, id: Uuid) -> DbResult<Option<SessionRow>> {
    let sql = format!("{SELECT} WHERE s.id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn list_in_scope(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    status: Option<&str>,
    limit: i64,
) -> DbResult<Vec<SessionRow>> {
    let sql = format!(
        "{SELECT} WHERE s.organization_id = $1 AND s.repository_id = ANY($2)
           AND ($3::text IS NULL OR s.extraction_status = $3)
         ORDER BY s.created_at DESC LIMIT $4"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(repo_ids)
        .bind(status)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

pub async fn load_payload(pool: &PgPool, session_id: Uuid) -> DbResult<Option<serde_json::Value>> {
    Ok(
        sqlx::query_scalar("SELECT payload FROM session_payloads WHERE session_id = $1")
            .bind(session_id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn mark_extraction_running(pool: &PgPool, session_id: Uuid) -> DbResult<()> {
    sqlx::query("UPDATE sessions SET extraction_status = 'running' WHERE id = $1 AND extraction_status IN ('pending', 'failed', 'running')")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_extraction_failed(
    pool: &PgPool,
    session_id: Uuid,
    error: &str,
    terminal: bool,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE sessions SET extraction_status = CASE WHEN $3 THEN 'failed' ELSE 'pending' END, extraction_error = $2
          WHERE id = $1 AND extraction_status NOT IN ('succeeded', 'skipped')",
    )
    .bind(session_id)
    .bind(antibody_core::text::truncate_chars(error, 1000))
    .bind(terminal)
    .execute(pool)
    .await?;
    Ok(())
}

/// Sessions on a branch or revision, used to correlate Git events.
pub async fn ids_for_branch_or_revision(
    pool: &PgPool,
    repo_id: Uuid,
    branch: Option<&str>,
    revisions: &[String],
) -> DbResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM sessions WHERE repository_id = $1
            AND (($2::text IS NOT NULL AND branch = $2) OR revision = ANY($3))",
    )
    .bind(repo_id)
    .bind(branch)
    .bind(revisions)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct DeletionReport {
    pub sessions_deleted: u64,
    pub solutions_deleted: u64,
    pub solutions_detached: u64,
    pub jobs_cancelled: u64,
}

/// Serialize derived-content writes within a repository for this transaction.
///
/// Call before locking sessions, solutions, or jobs, and keep the transaction
/// scoped to one repository. Repository-wide ordering also protects batches
/// that extract several lineages in different orders. NO KEY UPDATE lets
/// unrelated ingestion keep taking foreign-key KEY SHARE locks.
pub async fn lock_repository(
    conn: &mut PgConnection,
    org_id: Uuid,
    repo_id: Uuid,
) -> DbResult<bool> {
    let repository: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM repositories WHERE id = $1 AND organization_id = $2 FOR NO KEY UPDATE",
    )
    .bind(repo_id)
    .bind(org_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(repository.is_some())
}

/// Lock repository then session before committing extraction or deleting it.
/// The caller must retain the transaction until every derived write completes.
pub async fn lock_for_extraction(
    conn: &mut PgConnection,
    org_id: Uuid,
    repo_id: Uuid,
    session_id: Uuid,
) -> DbResult<bool> {
    if !lock_repository(&mut *conn, org_id, repo_id).await? {
        return Ok(false);
    }
    let session: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE id = $1 AND organization_id = $2 AND repository_id = $3 FOR UPDATE",
    )
    .bind(session_id)
    .bind(org_id)
    .bind(repo_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(session.is_some())
}

/// Delete a session and everything derived only from it: the raw payload,
/// solutions no other session supports (with their embeddings, evidence and
/// sources via cascade), workflow candidates, and pending jobs.
pub async fn delete_session(
    pool: &PgPool,
    org_id: Uuid,
    session_id: Uuid,
) -> DbResult<Option<DeletionReport>> {
    let mut tx = pool.begin().await?;
    let repo_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT repository_id FROM sessions WHERE id = $1 AND organization_id = $2",
    )
    .bind(session_id)
    .bind(org_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(repo_id) = repo_id else {
        return Ok(None);
    };
    // Take the shared extraction/deletion lock before inspecting supporting
    // sessions. A concurrent deletion must observe the first deletion's commit.
    if !lock_for_extraction(&mut tx, org_id, repo_id, session_id).await? {
        return Ok(None);
    }
    let mut report = DeletionReport::default();

    let orphaned: Vec<Uuid> = sqlx::query_scalar(
        "SELECT ss.solution_id FROM solution_sessions ss
          WHERE ss.session_id = $1
            AND NOT EXISTS (SELECT 1 FROM solution_sessions o WHERE o.solution_id = ss.solution_id AND o.session_id <> $1)",
    )
    .bind(session_id)
    .fetch_all(&mut *tx)
    .await?;
    let detached: i64 =
        sqlx::query_scalar("SELECT count(*) FROM solution_sessions WHERE session_id = $1")
            .bind(session_id)
            .fetch_one(&mut *tx)
            .await?;
    report.solutions_detached = (detached as u64).saturating_sub(orphaned.len() as u64);

    // Cancel pending jobs for the session and for embeddings of doomed solutions.
    report.jobs_cancelled = jobs::cancel_for_session(&mut *tx, session_id).await?;
    let orphan_keys: Vec<String> = orphaned.iter().map(|id| format!("solution:{id}")).collect();
    let cancelled = sqlx::query(
        "UPDATE jobs SET state = 'cancelled', finished_at = now(), updated_at = now(), payload = '{}'::jsonb
          WHERE dedupe_key = ANY($1) AND state IN ('queued', 'running', 'dead')",
    )
    .bind(&orphan_keys)
    .execute(&mut *tx)
    .await?;
    report.jobs_cancelled += cancelled.rows_affected();

    let res = sqlx::query("DELETE FROM solutions WHERE id = ANY($1) AND organization_id = $2")
        .bind(&orphaned)
        .bind(org_id)
        .execute(&mut *tx)
        .await?;
    report.solutions_deleted = res.rows_affected();

    sqlx::query(
        "DELETE FROM source_artifacts WHERE organization_id = $1 AND kind = 'session' AND external_id = $2",
    )
    .bind(org_id)
    .bind(session_id.to_string())
    .execute(&mut *tx)
    .await?;

    let res = sqlx::query("DELETE FROM sessions WHERE id = $1 AND organization_id = $2")
        .bind(session_id)
        .bind(org_id)
        .execute(&mut *tx)
        .await?;
    report.sessions_deleted = res.rows_affected();
    tx.commit().await?;
    Ok(Some(report))
}
