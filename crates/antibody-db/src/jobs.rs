//! PostgreSQL-backed durable job queue: atomic enqueue (inside the caller's
//! transaction), dedupe keys, leases with expiry, bounded retries with
//! exponential backoff and jitter, and a dead-letter state.

use crate::{DbResult, new_id};
use antibody_core::model::JobKind;
use chrono::{DateTime, Duration, Utc};
use rand::RngExt;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewJob {
    pub kind: JobKind,
    pub dedupe_key: String,
    pub payload: serde_json::Value,
    pub organization_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub run_at: Option<DateTime<Utc>>,
    pub max_attempts: i32,
}

impl NewJob {
    pub fn new(kind: JobKind, dedupe_key: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            kind,
            dedupe_key: dedupe_key.into(),
            payload,
            organization_id: None,
            session_id: None,
            run_at: None,
            max_attempts: 8,
        }
    }
    pub fn org(mut self, org: Uuid) -> Self {
        self.organization_id = Some(org);
        self
    }
    pub fn session(mut self, session: Uuid) -> Self {
        self.session_id = Some(session);
        self
    }
    pub fn run_at(mut self, at: DateTime<Utc>) -> Self {
        self.run_at = Some(at);
        self
    }
    pub fn max_attempts(mut self, n: i32) -> Self {
        self.max_attempts = n;
        self
    }
}

/// Enqueue a job. Returns `None` when an active job with the same
/// `(kind, dedupe_key)` already exists. Pass a transaction to make enqueue
/// atomic with the write that triggers it.
pub async fn enqueue<'e>(exec: impl PgExecutor<'e>, job: NewJob) -> DbResult<Option<Uuid>> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO jobs (id, organization_id, session_id, kind, dedupe_key, payload, run_at, max_attempts)
         VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, now()), $8)
         ON CONFLICT (kind, dedupe_key) WHERE state IN ('queued', 'running') DO NOTHING
         RETURNING id",
    )
    .bind(new_id())
    .bind(job.organization_id)
    .bind(job.session_id)
    .bind(job.kind.as_str())
    .bind(&job.dedupe_key)
    .bind(&job.payload)
    .bind(job.run_at)
    .bind(job.max_attempts)
    .fetch_optional(exec)
    .await?;
    Ok(id)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub kind: String,
    pub dedupe_key: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

/// Claim up to `limit` ready jobs (queued and due, or running with an
/// expired lease). `FOR UPDATE SKIP LOCKED` lets many workers share the queue.
pub async fn claim(
    pool: &PgPool,
    worker_id: &str,
    limit: i64,
    lease: Duration,
) -> DbResult<Vec<ClaimedJob>> {
    let lease_secs = lease.num_milliseconds() as f64 / 1000.0;
    Ok(sqlx::query_as(
        "UPDATE jobs j
            SET state = 'running',
                lease_owner = $1,
                lease_expires_at = now() + make_interval(secs => $3),
                attempts = j.attempts + 1,
                updated_at = now()
          WHERE j.id IN (
                SELECT id FROM jobs
                 WHERE (state = 'queued' AND run_at <= now())
                    OR (state = 'running' AND lease_expires_at < now() AND attempts < max_attempts)
                 ORDER BY run_at
                 LIMIT $2
                 FOR UPDATE SKIP LOCKED)
        RETURNING j.id, j.organization_id, j.session_id, j.kind, j.dedupe_key, j.payload, j.attempts, j.max_attempts",
    )
    .bind(worker_id)
    .bind(limit)
    .bind(lease_secs)
    .fetch_all(pool)
    .await?)
}

/// Extend a lease for long-running work. Returns false if the lease was lost.
pub async fn heartbeat(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    lease: Duration,
) -> DbResult<bool> {
    let lease_secs = lease.num_milliseconds() as f64 / 1000.0;
    let res = sqlx::query(
        "UPDATE jobs SET lease_expires_at = now() + make_interval(secs => $3), updated_at = now()
          WHERE id = $1 AND lease_owner = $2 AND state = 'running'",
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_secs)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn complete(pool: &PgPool, job_id: Uuid, worker_id: &str) -> DbResult<bool> {
    let res = sqlx::query(
        "UPDATE jobs SET state = 'succeeded', lease_owner = NULL, lease_expires_at = NULL,
                         finished_at = now(), updated_at = now(), last_error = NULL
          WHERE id = $1 AND lease_owner = $2 AND state = 'running'",
    )
    .bind(job_id)
    .bind(worker_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Exponential backoff with full jitter: base * 2^(attempt-1), capped.
pub fn backoff(attempt: i32, base: Duration, cap: Duration) -> Duration {
    let exp = base
        .num_milliseconds()
        .saturating_mul(1i64 << attempt.clamp(0, 20).saturating_sub(1));
    let capped = exp.min(cap.num_milliseconds()).max(base.num_milliseconds());
    let jitter = rand::rng().random_range(capped / 2..=capped);
    Duration::milliseconds(jitter)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOutcome {
    Retrying,
    Dead,
    LeaseLost,
}

/// Record a failure: requeue with backoff, or dead-letter after max attempts.
/// `permanent` dead-letters immediately (e.g. invalid payload).
pub async fn fail(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    error: &str,
    permanent: bool,
    retry_in: Duration,
) -> DbResult<FailOutcome> {
    let error = antibody_core::text::truncate_chars(error, 2000);
    let state: Option<String> = sqlx::query_scalar(
        "UPDATE jobs SET
            state = CASE WHEN $4 OR attempts >= max_attempts THEN 'dead' ELSE 'queued' END,
            run_at = CASE WHEN $4 OR attempts >= max_attempts THEN run_at ELSE now() + make_interval(secs => $5) END,
            finished_at = CASE WHEN $4 OR attempts >= max_attempts THEN now() ELSE NULL END,
            lease_owner = NULL, lease_expires_at = NULL, last_error = $3, updated_at = now()
          WHERE id = $1 AND lease_owner = $2 AND state = 'running'
        RETURNING state",
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(&error)
    .bind(permanent)
    .bind(retry_in.num_milliseconds() as f64 / 1000.0)
    .fetch_optional(pool)
    .await?;
    Ok(match state.as_deref() {
        Some("dead") => FailOutcome::Dead,
        Some(_) => FailOutcome::Retrying,
        None => FailOutcome::LeaseLost,
    })
}

/// Dead-letter jobs whose lease expired after their final attempt (the
/// worker crashed or was killed while processing them).
pub async fn reap_expired(pool: &PgPool) -> DbResult<u64> {
    let res = sqlx::query(
        "UPDATE jobs SET state = 'dead', finished_at = now(), updated_at = now(), lease_owner = NULL,
                         last_error = COALESCE(last_error, 'lease expired on final attempt')
          WHERE state = 'running' AND lease_expires_at < now() AND attempts >= max_attempts",
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Requeue a dead job (admin action).
pub async fn retry_dead(pool: &PgPool, org_id: Uuid, job_id: Uuid) -> DbResult<bool> {
    let res = sqlx::query(
        "UPDATE jobs SET state = 'queued', attempts = 0, run_at = now(), finished_at = NULL, updated_at = now()
          WHERE id = $1 AND organization_id = $2 AND state = 'dead'",
    )
    .bind(job_id)
    .bind(org_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Cancel pending jobs for a session (used by deletion).
pub async fn cancel_for_session<'e>(exec: impl PgExecutor<'e>, session_id: Uuid) -> DbResult<u64> {
    let res = sqlx::query(
        "UPDATE jobs SET state = 'cancelled', finished_at = now(), updated_at = now(), payload = '{}'::jsonb
          WHERE session_id = $1 AND state IN ('queued', 'running', 'dead')",
    )
    .bind(session_id)
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct JobView {
    pub id: Uuid,
    pub kind: String,
    pub state: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub run_at: DateTime<Utc>,
    pub last_error: Option<String>,
    pub session_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn list(
    pool: &PgPool,
    org_id: Uuid,
    state: Option<&str>,
    limit: i64,
) -> DbResult<Vec<JobView>> {
    Ok(sqlx::query_as(
        "SELECT id, kind, state, attempts, max_attempts, run_at, last_error, session_id, created_at, updated_at
           FROM jobs
          WHERE organization_id = $1 AND ($2::text IS NULL OR state = $2)
          ORDER BY updated_at DESC
          LIMIT $3",
    )
    .bind(org_id)
    .bind(state)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct QueueStats {
    pub queued: i64,
    pub running: i64,
    pub dead: i64,
    /// Age in seconds of the oldest due, queued job.
    pub oldest_queued_age_seconds: Option<f64>,
}

pub async fn stats(pool: &PgPool, org_id: Option<Uuid>) -> DbResult<QueueStats> {
    let row: (i64, i64, i64, Option<f64>) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE state = 'queued'),
                count(*) FILTER (WHERE state = 'running'),
                count(*) FILTER (WHERE state = 'dead'),
                EXTRACT(EPOCH FROM now() - min(run_at) FILTER (WHERE state = 'queued' AND run_at <= now()))::float8
           FROM jobs WHERE ($1::uuid IS NULL OR organization_id = $1)",
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;
    Ok(QueueStats {
        queued: row.0,
        running: row.1,
        dead: row.2,
        oldest_queued_age_seconds: row.3,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let base = Duration::seconds(2);
        let cap = Duration::seconds(60);
        for _ in 0..50 {
            let b1 = backoff(1, base, cap);
            assert!(
                b1 >= Duration::seconds(1) && b1 <= Duration::seconds(2),
                "{b1:?}"
            );
            let b10 = backoff(10, base, cap);
            assert!(b10 >= Duration::seconds(30) && b10 <= cap, "{b10:?}");
        }
    }
}
