//! Overview metrics. Counts only what was observed: retrievals and reported
//! outcomes. "Confirmed reuse" requires a `used` outcome whose check was
//! confirmed by a signed CI event. No savings are estimated here.

use crate::DbResult;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct Overview {
    pub repositories: i64,
    pub sessions_total: i64,
    pub sessions_7d: i64,
    pub extraction_pending: i64,
    pub extraction_failed: i64,
    pub solutions_total: i64,
    pub solutions_candidate: i64,
    pub solutions_corroborated: i64,
    pub solutions_verified: i64,
    pub solutions_needing_revalidation: i64,
    pub retrievals_7d: i64,
    pub retrievals_with_results_7d: i64,
    pub retrieval_context_tokens_7d: i64,
    pub retrieval_p95_latency_ms_7d: Option<f64>,
    pub outcomes_used: i64,
    pub outcomes_rejected: i64,
    pub outcomes_inapplicable: i64,
    /// `used` outcomes whose claimed check a signed CI event confirmed.
    pub confirmed_reuse: i64,
    pub ci_events_7d: i64,
    pub ci_events_with_matches_7d: i64,
}

pub async fn overview(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    include_unscoped: bool,
) -> DbResult<Overview> {
    let mut o = Overview::default();
    (o.repositories,) = sqlx::query_as(
        "SELECT count(*) FROM repositories WHERE organization_id = $1 AND id = ANY($2)",
    )
    .bind(org_id)
    .bind(repo_ids)
    .fetch_one(pool)
    .await?;
    (
        o.sessions_total,
        o.sessions_7d,
        o.extraction_pending,
        o.extraction_failed,
    ) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE created_at > now() - interval '7 days'),
                count(*) FILTER (WHERE extraction_status IN ('pending', 'running')),
                count(*) FILTER (WHERE extraction_status = 'failed')
           FROM sessions WHERE organization_id = $1 AND repository_id = ANY($2)",
    )
    .bind(org_id)
    .bind(repo_ids)
    .fetch_one(pool)
    .await?;
    (o.solutions_total, o.solutions_candidate, o.solutions_corroborated, o.solutions_verified, o.solutions_needing_revalidation) =
        sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE state = 'candidate'), count(*) FILTER (WHERE state = 'corroborated'),
                    count(*) FILTER (WHERE state = 'verified'), count(*) FILTER (WHERE needs_revalidation)
               FROM solutions WHERE organization_id = $1 AND repository_id = ANY($2)",
        )
        .bind(org_id)
        .bind(repo_ids)
        .fetch_one(pool)
        .await?;
    (o.retrievals_7d, o.retrievals_with_results_7d, o.retrieval_context_tokens_7d, o.retrieval_p95_latency_ms_7d) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE result_count > 0), COALESCE(sum(context_tokens), 0)::bigint,
                percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms)
           FROM retrievals r
          WHERE r.organization_id = $1 AND r.created_at > now() - interval '7 days'
            AND (r.repository_id = ANY($2) OR ($3 AND r.repository_id IS NULL))
            AND ($3 OR NOT EXISTS (
                SELECT 1 FROM jsonb_array_elements(r.candidates) candidate
                LEFT JOIN solutions s ON s.id = (candidate->>'solution_id')::uuid
                  AND s.organization_id = r.organization_id
                WHERE s.id IS NULL OR NOT (s.repository_id = ANY($2))
            ))",
    )
    .bind(org_id)
    .bind(repo_ids)
    .bind(include_unscoped)
    .fetch_one(pool)
    .await?;
    (o.outcomes_used, o.outcomes_rejected, o.outcomes_inapplicable, o.confirmed_reuse) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE outcome = 'used'), count(*) FILTER (WHERE outcome = 'rejected'),
                count(*) FILTER (WHERE outcome = 'inapplicable'),
                count(*) FILTER (WHERE outcome = 'used' AND verification = 'ci_verified')
           FROM outcomes o JOIN solutions s ON s.id = o.solution_id
          WHERE o.organization_id = $1 AND s.organization_id = $1
            AND (o.repository_id = ANY($2) OR ($3 AND o.repository_id IS NULL))
            AND s.repository_id = ANY($2)",
    )
    .bind(org_id)
    .bind(repo_ids)
    .bind(include_unscoped)
    .fetch_one(pool)
    .await?;
    (o.ci_events_7d, o.ci_events_with_matches_7d) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE match_count > 0) FROM ci_events
          WHERE organization_id = $1 AND repository_id = ANY($2) AND created_at > now() - interval '7 days'",
    )
    .bind(org_id)
    .bind(repo_ids)
    .fetch_one(pool)
    .await?;
    Ok(o)
}
