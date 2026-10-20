//! Targeted revalidation and retention sweeps.

use super::{RevalidateJob, trusted_evidence};
use crate::{JobError, WorkerContext};
use antibody_core::model::{EvidenceKind, EvidenceResult};
use antibody_core::state::SolutionEvent;
use antibody_db::audit::{self, Actor};
use antibody_db::retention;
use antibody_db::solutions;

/// Targeted revalidation (Flow D). Never changes code; only flags evidence.
pub(super) async fn revalidate(ctx: &WorkerContext, job: RevalidateJob) -> Result<(), JobError> {
    let ids = solutions::live_ids_in_repo(&ctx.pool, job.repo_id).await?;
    let mut flagged = 0;
    for sid in ids {
        let mut tx = ctx.pool.begin().await?;
        let Some(sol) = solutions::get_system(&mut tx, sid).await? else {
            continue;
        };
        let affected = match job.reason.as_str() {
            "dependency_change" => {
                let root_change = job.dirs.iter().any(|d| d.is_empty());
                let under_dir = |p: &str| {
                    job.dirs
                        .iter()
                        .any(|d| !d.is_empty() && p.starts_with(&d.to_lowercase()))
                };
                let has_deps = !sol.applicability.0.dependencies.is_empty();
                // Only fixes that depend on versions, or that live under the
                // changed manifest's directory, are affected.
                has_deps
                    && (root_change
                        || sol.paths.iter().any(|p| under_dir(p))
                        || sol.applicability.0.components.iter().any(|c| under_dir(c)))
            }
            "permission_change" => false,
            _ => false,
        };
        if affected {
            trusted_evidence(
                &mut tx,
                job.org_id,
                sid,
                EvidenceKind::DependencyChange,
                EvidenceResult::Info,
                job.revision.clone(),
                None,
                None,
                format!("deps:{}", job.revision.clone().unwrap_or_default()),
                "dependency manifest changed on the default branch",
            )
            .await?;
            solutions::apply_event(&mut tx, sid, &SolutionEvent::DependencyChanged).await?;
            flagged += 1;
        }
        tx.commit().await?;
    }
    audit::record(
        &ctx.pool,
        Some(job.org_id),
        &Actor::system("worker"),
        "repository.revalidated",
        "repository",
        Some(job.repo_id.to_string()),
        serde_json::json!({ "reason": job.reason, "flagged": flagged }),
    )
    .await?;
    Ok(())
}

pub(super) async fn retention_sweep(ctx: &WorkerContext) -> Result<(), JobError> {
    let report = retention::sweep(&ctx.pool).await?;
    if report.payloads_purged > 0 {
        audit::record(
            &ctx.pool,
            None,
            &Actor::system("worker"),
            "retention.swept",
            "session_payload",
            None,
            serde_json::to_value(&report).unwrap_or_default(),
        )
        .await?;
    }
    tracing::info!(?report, "retention sweep complete");
    Ok(())
}
