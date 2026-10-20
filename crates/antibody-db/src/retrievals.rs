use crate::{DbResult, new_id};
use antibody_core::model::RetrievalChannel;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CandidateRef {
    pub solution_id: Uuid,
    pub score: f32,
    pub rank: u32,
    pub returned: bool,
}

pub struct NewRetrieval {
    pub id: Uuid,
    pub org_id: Uuid,
    pub principal_id: Option<Uuid>,
    pub channel: RetrievalChannel,
    pub repository_id: Option<Uuid>,
    pub task_fingerprint: String,
    pub agent_session_id: Option<String>,
    pub candidates: Vec<CandidateRef>,
    pub result_count: i32,
    pub latency_ms: i32,
    pub context_tokens: i32,
    pub degraded: bool,
}

impl NewRetrieval {
    pub fn new_id() -> Uuid {
        new_id()
    }
}

pub async fn insert(pool: &PgPool, r: &NewRetrieval) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO retrievals (id, organization_id, principal_id, channel, repository_id, task_fingerprint, agent_session_id,
                                 candidates, result_count, latency_ms, context_tokens, degraded)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(r.id)
    .bind(r.org_id)
    .bind(r.principal_id)
    .bind(r.channel.as_str())
    .bind(r.repository_id)
    .bind(&r.task_fingerprint)
    .bind(&r.agent_session_id)
    .bind(sqlx::types::Json(&r.candidates))
    .bind(r.result_count)
    .bind(r.latency_ms)
    .bind(r.context_tokens)
    .bind(r.degraded)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether this principal already received context for an agent session.
pub async fn seen_agent_session(
    pool: &PgPool,
    org_id: Uuid,
    principal_id: Uuid,
    agent_session_id: &str,
) -> DbResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM retrievals WHERE organization_id = $1 AND principal_id = $2 AND agent_session_id = $3)",
    )
    .bind(org_id)
    .bind(principal_id)
    .bind(agent_session_id)
    .fetch_one(pool)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RetrievalRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub principal_id: Option<Uuid>,
    pub repository_id: Option<Uuid>,
    pub candidates: sqlx::types::Json<Vec<CandidateRef>>,
}

pub async fn get(pool: &PgPool, org_id: Uuid, id: Uuid) -> DbResult<Option<RetrievalRow>> {
    Ok(sqlx::query_as(
        "SELECT id, organization_id, principal_id, repository_id, candidates FROM retrievals WHERE id = $1 AND organization_id = $2",
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct RetrievalView {
    pub id: Uuid,
    pub channel: String,
    pub repository: Option<String>,
    pub principal: Option<String>,
    pub result_count: i32,
    pub latency_ms: i32,
    pub context_tokens: i32,
    pub degraded: bool,
    pub created_at: DateTime<Utc>,
    pub outcomes_used: i64,
    pub outcomes_rejected: i64,
    pub outcomes_inapplicable: i64,
}

pub async fn list_recent(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    include_unscoped: bool,
    limit: i64,
) -> DbResult<Vec<RetrievalView>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.channel, rp.full_name AS repository, p.display_name AS principal, r.result_count, r.latency_ms,
                r.context_tokens, r.degraded, r.created_at,
                count(o.id) FILTER (WHERE o.outcome = 'used') AS outcomes_used,
                count(o.id) FILTER (WHERE o.outcome = 'rejected') AS outcomes_rejected,
                count(o.id) FILTER (WHERE o.outcome = 'inapplicable') AS outcomes_inapplicable
           FROM retrievals r
           LEFT JOIN repositories rp ON rp.id = r.repository_id
           LEFT JOIN principals p ON p.id = r.principal_id
           LEFT JOIN outcomes o ON o.retrieval_id = r.id
          WHERE r.organization_id = $1
            AND (r.repository_id = ANY($2) OR ($3 AND r.repository_id IS NULL))
            AND ($3 OR NOT EXISTS (
                SELECT 1 FROM jsonb_array_elements(r.candidates) candidate
                LEFT JOIN solutions s ON s.id = (candidate->>'solution_id')::uuid
                  AND s.organization_id = r.organization_id
                WHERE s.id IS NULL OR NOT (s.repository_id = ANY($2))
            ))
          GROUP BY r.id, rp.full_name, p.display_name
          ORDER BY r.created_at DESC
          LIMIT $4",
    )
    .bind(org_id)
    .bind(repo_ids)
    .bind(include_unscoped)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}
