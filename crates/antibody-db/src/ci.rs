use crate::{DbResult, new_id};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

pub struct NewCiEvent<'a> {
    pub org_id: Uuid,
    pub repo_id: Uuid,
    pub source: &'a str,
    pub revision: &'a str,
    pub check_name: &'a str,
    pub conclusion: &'a str,
    pub signature: &'a str,
    pub excerpt: Option<&'a str>,
    pub url: Option<&'a str>,
    pub component_path: Option<&'a str>,
}

/// Insert a CI event, deduplicated by (repository, revision, check, signature).
/// Returns (id, duplicate).
pub async fn insert(pool: &PgPool, e: NewCiEvent<'_>) -> DbResult<(Uuid, bool)> {
    let mut conn = pool.acquire().await?;
    insert_in(&mut conn, e).await
}

/// The API uses the same transaction for replay protection and event storage.
pub async fn insert_in(conn: &mut PgConnection, e: NewCiEvent<'_>) -> DbResult<(Uuid, bool)> {
    let clean = |s: &str| antibody_core::redact::redact(s).text;
    let revision = clean(e.revision);
    let check_name = clean(e.check_name);
    let signature = clean(e.signature);
    let hash = antibody_core::normalize::signature_hash(&signature);
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO ci_events (id, organization_id, repository_id, source, revision, check_name, conclusion, signature,
                                signature_hash, excerpt, url, component_path)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         ON CONFLICT (repository_id, revision, check_name, signature_hash) DO NOTHING
         RETURNING id",
    )
    .bind(new_id())
    .bind(e.org_id)
    .bind(e.repo_id)
    .bind(clean(e.source))
    .bind(&revision)
    .bind(&check_name)
    .bind(clean(e.conclusion))
    .bind(&signature)
    .bind(&hash)
    .bind(e.excerpt.map(clean))
    .bind(e.url.map(clean))
    .bind(e.component_path.map(clean))
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(id) = inserted {
        return Ok((id, false));
    }
    let id: Uuid = sqlx::query_scalar(
        "SELECT id FROM ci_events WHERE repository_id = $1 AND revision = $2 AND check_name = $3 AND signature_hash = $4",
    )
    .bind(e.repo_id)
    .bind(&revision)
    .bind(&check_name)
    .bind(&hash)
    .fetch_one(conn)
    .await?;
    Ok((id, true))
}

pub async fn set_result(
    pool: &PgPool,
    id: Uuid,
    retrieval_id: Option<Uuid>,
    match_count: i32,
    status: &str,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE ci_events SET retrieval_id = $2, match_count = $3, status = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(retrieval_id)
    .bind(match_count)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CiEventRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub repository_id: Uuid,
    pub repository: String,
    pub source: String,
    pub revision: String,
    pub check_name: String,
    pub conclusion: String,
    pub signature: String,
    pub excerpt: Option<String>,
    pub url: Option<String>,
    pub component_path: Option<String>,
    pub retrieval_id: Option<Uuid>,
    pub match_count: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub candidate_ids: Vec<Uuid>,
}

const SELECT: &str = "SELECT c.id, c.organization_id, c.repository_id, r.full_name AS repository, c.source, c.revision,
        c.check_name, c.conclusion, c.signature, c.excerpt, c.url, c.component_path, c.retrieval_id, c.match_count,
        c.status, c.created_at,
        COALESCE((SELECT array_agg((x->>'solution_id')::uuid) FROM retrievals rt,
                  jsonb_array_elements(rt.candidates) x WHERE rt.id = c.retrieval_id AND (x->>'returned')::bool), '{}') AS candidate_ids
   FROM ci_events c JOIN repositories r ON r.id = c.repository_id";

pub async fn get(pool: &PgPool, id: Uuid) -> DbResult<Option<CiEventRow>> {
    let sql = format!("{SELECT} WHERE c.id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn list_in_scope(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    limit: i64,
) -> DbResult<Vec<CiEventRow>> {
    let sql = format!(
        "{SELECT} WHERE c.organization_id = $1 AND c.repository_id = ANY($2) ORDER BY c.created_at DESC LIMIT $3"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(repo_ids)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}
