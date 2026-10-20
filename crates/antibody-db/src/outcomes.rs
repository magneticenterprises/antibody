//! Outcome tracking. Outcomes are what agents *report*; only a signed CI
//! event can upgrade a claimed check to `ci_verified`. Nothing here infers
//! savings from search hits.

use crate::evidence::{self, NewEvidence};
use crate::{DbResult, new_id};
use antibody_core::api::OutcomeRequest;
use antibody_core::model::{EvidenceKind, EvidenceResult, OutcomeVerification};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

pub struct NewOutcome<'a> {
    pub org_id: Uuid,
    pub principal_id: Uuid,
    pub repository_id: Option<Uuid>,
    pub request: &'a OutcomeRequest,
}

#[derive(Debug, Clone, Copy)]
pub struct RecordedOutcome {
    pub id: Uuid,
    pub verification: OutcomeVerification,
    pub duplicate: bool,
}

pub async fn record(pool: &PgPool, o: NewOutcome<'_>) -> DbResult<RecordedOutcome> {
    let req = o.request;
    let verification = match &req.check {
        Some(c) if c.result == EvidenceResult::Pass => OutcomeVerification::Claimed,
        _ => OutcomeVerification::None,
    };
    let mut tx = pool.begin().await?;
    let id = new_id();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO outcomes (id, organization_id, retrieval_id, solution_id, principal_id, outcome, verification,
                               repository_id, check_name, check_revision, check_url, check_result, notes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (retrieval_id, solution_id) WHERE retrieval_id IS NOT NULL DO NOTHING
         RETURNING id",
    )
    .bind(id)
    .bind(o.org_id)
    .bind(req.retrieval_id)
    .bind(req.solution_id)
    .bind(o.principal_id)
    .bind(req.outcome.as_str())
    .bind(verification.as_str())
    .bind(o.repository_id)
    .bind(req.check.as_ref().map(|c| antibody_core::text::one_line(&c.name, 200)))
    .bind(req.check.as_ref().map(|c| antibody_core::text::one_line(&c.revision, 64)))
    .bind(req.check.as_ref().and_then(|c| c.url.clone()))
    .bind(req.check.as_ref().map(|c| c.result.as_str()))
    .bind(req.notes.as_deref().map(|n| antibody_core::redact::redact(&antibody_core::text::clean(n, 2000)).text))
    .fetch_optional(&mut *tx)
    .await?;

    let Some(id) = inserted else {
        let existing: (Uuid, String) = sqlx::query_as(
            "SELECT id, verification FROM outcomes WHERE retrieval_id = $1 AND solution_id = $2",
        )
        .bind(req.retrieval_id)
        .bind(req.solution_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(RecordedOutcome {
            id: existing.0,
            verification: crate::parse_enum(&existing.1)?,
            duplicate: true,
        });
    };

    // Agent reports are recorded as untrusted evidence. They never verify.
    evidence::insert(
        &mut tx,
        NewEvidence {
            org_id: o.org_id,
            solution_id: req.solution_id,
            kind: if req.check.is_some() {
                EvidenceKind::AgentClaim
            } else {
                EvidenceKind::Outcome
            },
            result: match (&req.check, req.outcome) {
                (Some(c), _) => c.result,
                _ => EvidenceResult::Info,
            },
            trusted: false,
            tested_revision: req.check.as_ref().map(|c| c.revision.clone()),
            check_name: req.check.as_ref().map(|c| c.name.clone()),
            environment: None,
            source_artifact_id: None,
            detail: Some(format!("agent reported outcome: {}", req.outcome)),
            dedupe_key: format!("outcome:{id}"),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(RecordedOutcome {
        id,
        verification,
        duplicate: false,
    })
}

/// Reconcile a claimed check with that same signed check on its revision.
/// A failing rerun revokes prior CI confirmation; unrelated checks do nothing.
pub async fn reconcile_by_ci<'e>(
    exec: impl PgExecutor<'e>,
    repo_id: Uuid,
    revision: &str,
    check_name: &str,
    passed: bool,
) -> DbResult<u64> {
    let res = sqlx::query(
        "UPDATE outcomes SET verification = CASE WHEN $4 THEN 'ci_verified' ELSE 'claimed' END,
                             verified_at = CASE WHEN $4 THEN now() ELSE NULL END
          WHERE repository_id = $1 AND verification IN ('claimed', 'ci_verified')
            AND check_result = 'pass' AND check_name = $3
            AND length(check_revision) >= 7 AND left(lower($2), length(check_revision)) = lower(check_revision)",
    )
    .bind(repo_id)
    .bind(revision)
    .bind(antibody_core::text::one_line(check_name, 200))
    .bind(passed)
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}
