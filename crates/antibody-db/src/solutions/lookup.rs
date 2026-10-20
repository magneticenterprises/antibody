//! Id lookups, lineage/session queries, embedding content and hard delete.

use super::SolutionRow;
use crate::DbResult;
use antibody_core::applicability::Applicability;
use sqlx::PgPool;
use uuid::Uuid;

/// Solutions whose recorded fix revision matches. Session revisions can be
/// the base of uncommitted changes; branch names can be reused. Neither proves
/// that a commit contains the fix.
pub async fn ids_for_fix(
    pool: &PgPool,
    repo_id: Uuid,
    revisions: &[String],
    _branch: Option<&str>,
) -> DbResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM solutions WHERE repository_id = $1 AND fix_revision = ANY($2)",
    )
    .bind(repo_id)
    .bind(revisions)
    .fetch_all(pool)
    .await?)
}

/// Solutions linked to a commit/PR source artifact by revision.
pub async fn ids_linked_to_revisions(
    pool: &PgPool,
    repo_id: Uuid,
    revisions: &[String],
) -> DbResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT ss.solution_id FROM solution_sources ss
           JOIN source_artifacts a ON a.id = ss.source_artifact_id
           JOIN solutions s ON s.id = ss.solution_id
          WHERE a.repository_id = $1 AND a.revision = ANY($2)
            AND ss.relation IN ('fix_commit', 'pull_request')
            AND (a.revision = s.fix_revision OR a.metadata->>'head_sha' = s.fix_revision)
         UNION
         SELECT id FROM solutions WHERE repository_id = $1 AND fix_revision = ANY($2)",
    )
    .bind(repo_id)
    .bind(revisions)
    .fetch_all(pool)
    .await?)
}

pub async fn live_ids_in_repo(pool: &PgPool, repo_id: Uuid) -> DbResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM solutions WHERE repository_id = $1 AND state NOT IN ('withdrawn', 'superseded')",
    )
    .bind(repo_id)
    .fetch_all(pool)
    .await?)
}

/// Other versions in the same lineage (same problem, different applicability).
pub async fn other_versions(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    lineage: &str,
    exclude: Uuid,
) -> DbResult<Vec<(Uuid, i32, String, sqlx::types::Json<Applicability>)>> {
    Ok(sqlx::query_as(
        "SELECT id, version, state, applicability FROM solutions
          WHERE organization_id = $1 AND repository_id = ANY($2) AND lineage_key = $3 AND id <> $4
          ORDER BY version",
    )
    .bind(org_id)
    .bind(repo_ids)
    .bind(lineage)
    .bind(exclude)
    .fetch_all(pool)
    .await?)
}

pub async fn session_ids(pool: &PgPool, solution_id: Uuid) -> DbResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT session_id FROM solution_sessions WHERE solution_id = $1 ORDER BY created_at",
    )
    .bind(solution_id)
    .fetch_all(pool)
    .await?)
}

/// Content used for embeddings and its hash.
pub fn embedding_content(row: &SolutionRow) -> String {
    let mut s = format!("{}\n{}\n", row.title, row.symptom);
    if let Some(rc) = &row.root_cause {
        s.push_str(rc);
        s.push('\n');
    }
    s.push_str(&row.remedy);
    s.push('\n');
    s.push_str(&row.error_signatures.join("\n"));
    s
}

/// Hard-delete one solution and everything derived from it (evidence,
/// embeddings, source links, outcomes via cascade) and cancel its pending
/// embedding jobs. Used for leak response; prefer withdraw otherwise.
pub async fn delete(pool: &PgPool, org_id: Uuid, id: Uuid) -> DbResult<bool> {
    let mut tx = pool.begin().await?;
    let repo_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT repository_id FROM solutions WHERE id = $1 AND organization_id = $2",
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(repo_id) = repo_id else {
        return Ok(false);
    };
    if !crate::sessions::lock_repository(&mut tx, org_id, repo_id).await? {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE jobs SET state = 'cancelled', finished_at = now(), updated_at = now(), payload = '{}'::jsonb
          WHERE dedupe_key = $1 AND state IN ('queued', 'running', 'dead')",
    )
    .bind(format!("solution:{id}"))
    .execute(&mut *tx)
    .await?;
    let res = sqlx::query("DELETE FROM solutions WHERE id = $1 AND organization_id = $2")
        .bind(id)
        .bind(org_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(res.rows_affected() == 1)
}
