//! Resume Git/CI correlation from durable session links after extraction.

use super::trusted_evidence;
use crate::{JobError, WorkerContext};
use antibody_core::model::{EvidenceKind, EvidenceResult};
use antibody_core::state::SolutionEvent;
use antibody_db::{evidence, sessions, solutions, sources};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct Artifact {
    id: Uuid,
    kind: String,
    external_id: String,
    revision: Option<String>,
    state: Option<String>,
    title: Option<String>,
}

pub(super) async fn correlate_session(
    ctx: &WorkerContext,
    session: &sessions::SessionRow,
) -> Result<(), JobError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT solution_id FROM solution_sessions WHERE session_id = $1 ORDER BY solution_id",
    )
    .bind(session.id)
    .fetch_all(&ctx.pool)
    .await?;
    for id in ids {
        let mut tx = ctx.pool.begin().await?;
        if !sessions::lock_for_extraction(
            &mut tx,
            session.organization_id,
            session.repository_id,
            session.id,
        )
        .await?
        {
            return Ok(());
        }
        let Some(sol) = solutions::get_system(&mut tx, id).await? else {
            continue;
        };
        // A branch or dirty session's base revision is not fix provenance.
        let Some(revision) = sol.fix_revision else {
            continue;
        };
        let git: Vec<Artifact> = sqlx::query_as(
            "SELECT id, kind, external_id, revision, state, title FROM source_artifacts
              WHERE repository_id = $1 AND kind IN ('commit', 'pull_request')
                AND (revision = $2 OR (kind = 'pull_request' AND metadata->>'head_sha' = $2))
              ORDER BY observed_at, id",
        )
        .bind(session.repository_id)
        .bind(&revision)
        .fetch_all(&mut *tx)
        .await?;
        let mut revisions = vec![revision];
        for a in git {
            sources::link(
                &mut tx,
                id,
                a.id,
                if a.kind == "pull_request" {
                    "pull_request"
                } else {
                    "fix_commit"
                },
            )
            .await?;
            if let Some(rev) = &a.revision {
                revisions.push(rev.clone());
            }
            if a.state.as_deref() == Some("merged") {
                trusted_evidence(
                    &mut tx,
                    session.organization_id,
                    id,
                    EvidenceKind::Merge,
                    EvidenceResult::Info,
                    a.revision,
                    None,
                    Some(a.id),
                    if a.kind == "pull_request" {
                        format!("pr:{}", a.external_id)
                    } else {
                        format!("merge:{}", a.external_id)
                    },
                    "fix merged",
                )
                .await?;
                solutions::apply_event(&mut tx, id, &SolutionEvent::Merged).await?;
            }
        }
        let checks: Vec<Artifact> = sqlx::query_as(
            "SELECT id, kind, external_id, revision, state, title FROM source_artifacts
              WHERE repository_id = $1 AND kind IN ('check_run', 'workflow_run', 'deployment')
                AND revision = ANY($2)
              ORDER BY observed_at, id",
        )
        .bind(session.repository_id)
        .bind(&revisions)
        .fetch_all(&mut *tx)
        .await?;
        for a in checks {
            if a.kind == "deployment" {
                if a.state.as_deref() == Some("success") {
                    sources::link(&mut tx, id, a.id, "deployment").await?;
                    trusted_evidence(
                        &mut tx,
                        session.organization_id,
                        id,
                        EvidenceKind::Deployment,
                        EvidenceResult::Info,
                        a.revision,
                        None,
                        Some(a.id),
                        format!("deploy:{}", a.external_id),
                        "deployed (not verification)",
                    )
                    .await?;
                    solutions::apply_event(&mut tx, id, &SolutionEvent::Deployed).await?;
                }
                continue;
            }
            let passed = a.state.as_deref() == Some("success");
            if !passed
                && !a
                    .state
                    .as_deref()
                    .is_some_and(antibody_integrations::github::is_failure)
            {
                continue;
            }
            sources::link(&mut tx, id, a.id, "check_run").await?;
            let inserted = trusted_evidence(
                &mut tx,
                session.organization_id,
                id,
                EvidenceKind::CiCheck,
                if passed {
                    EvidenceResult::Pass
                } else {
                    EvidenceResult::Fail
                },
                a.revision,
                a.title,
                Some(a.id),
                format!("{}:{}", a.kind, a.external_id),
                if passed {
                    "signed CI check passed"
                } else {
                    "signed CI check failed"
                },
            )
            .await?;
            if inserted && evidence::check_is_current(&mut tx, a.id).await? {
                solutions::apply_event(
                    &mut tx,
                    id,
                    &if passed {
                        SolutionEvent::TrustedCheckPassed
                    } else {
                        SolutionEvent::TrustedCheckFailed
                    },
                )
                .await?;
            }
        }
        tx.commit().await?;
    }
    Ok(())
}
