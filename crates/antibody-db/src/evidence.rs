use crate::{DbResult, new_id};
use antibody_core::model::{EvidenceKind, EvidenceResult};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewEvidence {
    pub org_id: Uuid,
    pub solution_id: Uuid,
    pub kind: EvidenceKind,
    pub result: EvidenceResult,
    /// True only for signed webhook events and audited admin actions.
    pub trusted: bool,
    pub tested_revision: Option<String>,
    pub check_name: Option<String>,
    pub environment: Option<String>,
    pub source_artifact_id: Option<Uuid>,
    pub detail: Option<String>,
    pub dedupe_key: String,
}

/// Insert evidence; returns false if the same evidence was already recorded.
pub async fn insert(conn: &mut PgConnection, e: NewEvidence) -> DbResult<bool> {
    let res = sqlx::query(
        "INSERT INTO evidence (id, organization_id, solution_id, kind, result, trusted, tested_revision, check_name,
                               environment, source_artifact_id, detail, dedupe_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         ON CONFLICT (solution_id, dedupe_key) DO NOTHING",
    )
    .bind(new_id())
    .bind(e.org_id)
    .bind(e.solution_id)
    .bind(e.kind.as_str())
    .bind(e.result.as_str())
    .bind(e.trusted)
    .bind(&e.tested_revision)
    .bind(&e.check_name)
    .bind(&e.environment)
    .bind(e.source_artifact_id)
    .bind(e.detail.as_deref().map(|d| antibody_core::text::truncate_chars(d, 1000)))
    .bind(&e.dedupe_key)
    .execute(conn)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Attempts are separate immutable observations. Late delivery of an older
/// attempt remains evidence, but must not replace the latest result.
pub async fn check_is_current(conn: &mut PgConnection, source_id: Uuid) -> DbResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT NOT EXISTS (
             SELECT 1 FROM source_artifacts newer
              WHERE newer.repository_id = a.repository_id
                AND newer.kind IN ('check_run', 'workflow_run')
                AND newer.revision = a.revision
                AND COALESCE(newer.metadata->>'check_identity', left(newer.title, 200)) = COALESCE(a.metadata->>'check_identity', left(a.title, 200))
                AND CASE WHEN newer.kind = a.kind AND
                    COALESCE(newer.metadata->>'run_id', newer.external_id) = COALESCE(a.metadata->>'run_id', a.external_id)
                  THEN (COALESCE((newer.metadata->>'run_attempt')::bigint, 1),
                        COALESCE((newer.metadata->>'completed_at')::timestamptz, '-infinity'::timestamptz))
                     > (COALESCE((a.metadata->>'run_attempt')::bigint, 1),
                        COALESCE((a.metadata->>'completed_at')::timestamptz, '-infinity'::timestamptz))
                    OR (COALESCE((newer.metadata->>'run_attempt')::bigint, 1) = COALESCE((a.metadata->>'run_attempt')::bigint, 1)
                        AND (newer.metadata->>'completed_at')::timestamptz IS NOT DISTINCT FROM (a.metadata->>'completed_at')::timestamptz
                        AND ((newer.state IN ('failure', 'timed_out', 'startup_failure') AND a.state = 'success')
                             OR (newer.state = a.state AND newer.metadata ? 'run_id' AND NOT a.metadata ? 'run_id')))
                  ELSE COALESCE((newer.metadata->>'completed_at')::timestamptz, '-infinity'::timestamptz)
                     > COALESCE((a.metadata->>'completed_at')::timestamptz, '-infinity'::timestamptz)
                    OR ((newer.metadata->>'completed_at')::timestamptz IS NOT DISTINCT FROM (a.metadata->>'completed_at')::timestamptz
                        AND newer.state IN ('failure', 'timed_out', 'startup_failure') AND a.state = 'success')
                END
         ) FROM source_artifacts a WHERE a.id = $1",
    )
    .bind(source_id)
    .fetch_one(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EvidenceRow {
    pub id: Uuid,
    pub solution_id: Uuid,
    pub kind: String,
    pub result: String,
    pub trusted: bool,
    pub tested_revision: Option<String>,
    pub check_name: Option<String>,
    pub environment: Option<String>,
    pub detail: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub source_kind: Option<String>,
    pub source_url: Option<String>,
    pub source_title: Option<String>,
    pub source_revision: Option<String>,
    pub source_state: Option<String>,
}

pub async fn for_solutions(pool: &PgPool, ids: &[Uuid]) -> DbResult<Vec<EvidenceRow>> {
    Ok(sqlx::query_as(
        "SELECT e.id, e.solution_id, e.kind, e.result, e.trusted, e.tested_revision, e.check_name, e.environment,
                e.detail, e.observed_at, a.kind AS source_kind, a.url AS source_url, a.title AS source_title,
                a.revision AS source_revision, a.state AS source_state
           FROM evidence e LEFT JOIN source_artifacts a ON a.id = e.source_artifact_id
          WHERE e.solution_id = ANY($1)
          ORDER BY e.observed_at DESC",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?)
}

/// Evidence visible with the source solution and any consuming repository.
/// Ordinary source evidence is preserved. Outcome evidence uses its retrieval
/// as provenance because legacy outcomes were attributed to the source repo.
pub async fn for_solutions_in_scope(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    ids: &[Uuid],
    include_unscoped: bool,
) -> DbResult<Vec<EvidenceRow>> {
    Ok(sqlx::query_as(
        "SELECT e.id, e.solution_id, e.kind, e.result, e.trusted, e.tested_revision, e.check_name, e.environment,
                e.detail, e.observed_at, a.kind AS source_kind, a.url AS source_url, a.title AS source_title,
                a.revision AS source_revision, a.state AS source_state
           FROM evidence e
           JOIN solutions s ON s.id = e.solution_id AND s.organization_id = e.organization_id
           LEFT JOIN source_artifacts a ON a.id = e.source_artifact_id
           LEFT JOIN outcomes o ON e.dedupe_key = 'outcome:' || o.id::text
             AND o.solution_id = e.solution_id AND o.organization_id = e.organization_id
           LEFT JOIN retrievals r ON r.id = o.retrieval_id AND r.organization_id = o.organization_id
          WHERE e.organization_id = $1 AND s.repository_id = ANY($2) AND e.solution_id = ANY($3)
            AND (e.dedupe_key NOT LIKE 'outcome:%'
                 OR (o.id IS NOT NULL
                     AND (r.repository_id = ANY($2) OR ($4 AND r.repository_id IS NULL))))
          ORDER BY e.observed_at DESC",
    )
    .bind(org_id)
    .bind(repo_ids)
    .bind(ids)
    .bind(include_unscoped)
    .fetch_all(pool)
    .await?)
}

/// Count of trusted passing evidence per solution, plus latest trusted check.
pub async fn trusted_summary(
    pool: &PgPool,
    ids: &[Uuid],
) -> DbResult<Vec<(Uuid, i64, Option<String>, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT solution_id,
                count(*) FILTER (WHERE trusted AND result = 'pass'),
                (array_agg(check_name ORDER BY observed_at DESC) FILTER (WHERE trusted AND result = 'pass' AND kind IN ('ci_check', 'manual_verification')))[1],
                (array_agg(tested_revision ORDER BY observed_at DESC) FILTER (WHERE trusted AND result = 'pass' AND kind IN ('ci_check', 'manual_verification')))[1]
           FROM evidence e WHERE solution_id = ANY($1)
             AND (e.kind <> 'ci_check' OR EXISTS (
                 SELECT 1 FROM solutions s WHERE s.id = e.solution_id AND s.fix_revision IS NOT NULL
                   AND (e.tested_revision = s.fix_revision OR EXISTS (
                       SELECT 1 FROM solution_sources ss JOIN source_artifacts a ON a.id = ss.source_artifact_id
                        WHERE ss.solution_id = s.id AND ss.relation IN ('fix_commit', 'pull_request')
                          AND (a.revision = s.fix_revision OR a.metadata->>'head_sha' = s.fix_revision)
                          AND (a.revision = e.tested_revision OR a.metadata->>'head_sha' = e.tested_revision)))))
             AND NOT EXISTS (
                 SELECT 1 FROM source_artifacts a JOIN source_artifacts newer
                   ON newer.repository_id = a.repository_id
                AND newer.kind IN ('check_run', 'workflow_run')
                AND newer.revision = a.revision
                AND COALESCE(newer.metadata->>'check_identity', left(newer.title, 200)) = COALESCE(a.metadata->>'check_identity', left(a.title, 200))
                AND CASE WHEN newer.kind = a.kind AND
                    COALESCE(newer.metadata->>'run_id', newer.external_id) = COALESCE(a.metadata->>'run_id', a.external_id)
                  THEN (COALESCE((newer.metadata->>'run_attempt')::bigint, 1),
                        COALESCE((newer.metadata->>'completed_at')::timestamptz, '-infinity'::timestamptz))
                     > (COALESCE((a.metadata->>'run_attempt')::bigint, 1),
                        COALESCE((a.metadata->>'completed_at')::timestamptz, '-infinity'::timestamptz))
                    OR (COALESCE((newer.metadata->>'run_attempt')::bigint, 1) = COALESCE((a.metadata->>'run_attempt')::bigint, 1)
                        AND (newer.metadata->>'completed_at')::timestamptz IS NOT DISTINCT FROM (a.metadata->>'completed_at')::timestamptz
                        AND ((newer.state IN ('failure', 'timed_out', 'startup_failure') AND a.state = 'success')
                             OR (newer.state = a.state AND newer.metadata ? 'run_id' AND NOT a.metadata ? 'run_id')))
                  ELSE COALESCE((newer.metadata->>'completed_at')::timestamptz, '-infinity'::timestamptz)
                     > COALESCE((a.metadata->>'completed_at')::timestamptz, '-infinity'::timestamptz)
                    OR ((newer.metadata->>'completed_at')::timestamptz IS NOT DISTINCT FROM (a.metadata->>'completed_at')::timestamptz
                        AND newer.state IN ('failure', 'timed_out', 'startup_failure') AND a.state = 'success')
                END
                  WHERE a.id = e.source_artifact_id)
          GROUP BY solution_id",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?)
}
