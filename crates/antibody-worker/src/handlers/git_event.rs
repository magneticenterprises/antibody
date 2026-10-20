//! Flow C/D: Git events (push, pull request, repository changes).

use super::{GitEventJob, RevalidateJob, checks, trusted_evidence};
use crate::{JobError, WorkerContext};
use antibody_core::model::{EvidenceKind, EvidenceResult, JobKind, SourceKind};
use antibody_core::state::SolutionEvent;
use antibody_db::audit::{self, Actor};
use antibody_db::jobs::{self, NewJob};
use antibody_db::repos;
use antibody_db::solutions;
use antibody_db::sources::{self, NewSource};
use antibody_integrations::git;
use antibody_integrations::github::GitEvent;
use uuid::Uuid;

pub(super) async fn process_git_event(
    ctx: &WorkerContext,
    job: GitEventJob,
) -> Result<(), JobError> {
    let GitEventJob {
        org_id,
        repo_id,
        delivery_id,
        received_at,
        event,
    } = job;
    match event {
        GitEvent::Ping | GitEvent::Unsupported { .. } => Ok(()),
        e @ GitEvent::Push { .. } => on_push(ctx, org_id, repo_id, e).await,
        e @ GitEvent::PullRequest { .. } => on_pull_request(ctx, org_id, repo_id, e).await,
        e @ GitEvent::CheckCompleted { .. } => {
            checks::on_check_completed(ctx, org_id, repo_id, e).await
        }
        e @ GitEvent::DeploymentStatus { .. } => {
            checks::on_deployment_status(ctx, org_id, repo_id, e).await
        }
        e @ GitEvent::RepositoryChanged { .. } => {
            on_repository_changed(ctx, org_id, repo_id, delivery_id, received_at, e).await
        }
        e @ GitEvent::PermissionChanged { .. } => {
            on_permission_changed(ctx, org_id, repo_id, e).await
        }
    }
}

async fn on_push(
    ctx: &WorkerContext,
    org_id: Uuid,
    repo_id: Uuid,
    event: GitEvent,
) -> Result<(), JobError> {
    let GitEvent::Push {
        branch,
        commits,
        default_branch_push,
        after,
        ..
    } = event
    else {
        unreachable!("process_git_event routes only Push here");
    };
    let mut dep_dirs: Vec<String> = Vec::new();
    for c in &commits {
        let mut tx = ctx.pool.begin().await?;
        let source_id = sources::upsert(
            &mut tx,
            &NewSource {
                org_id,
                repo_id,
                kind: SourceKind::Commit,
                external_id: c.sha.clone(),
                url: c.url.clone(),
                revision: Some(c.sha.clone()),
                title: c.message.lines().next().map(str::to_string),
                state: Some(if default_branch_push {
                    "merged".into()
                } else {
                    "pushed".into()
                }),
                metadata: serde_json::json!({ "branch": branch }),
            },
        )
        .await?;
        tx.commit().await?;

        // Commits that carry a fix (by revision or branch).
        let linked =
            solutions::ids_for_fix(&ctx.pool, repo_id, std::slice::from_ref(&c.sha), None).await?;
        for sid in linked {
            let mut tx = ctx.pool.begin().await?;
            sources::link(&mut tx, sid, source_id, "fix_commit").await?;
            if default_branch_push {
                trusted_evidence(
                    &mut tx,
                    org_id,
                    sid,
                    EvidenceKind::Merge,
                    EvidenceResult::Info,
                    Some(c.sha.clone()),
                    None,
                    Some(source_id),
                    format!("merge:{}", c.sha),
                    "fix commit landed on the default branch",
                )
                .await?;
                solutions::apply_event(&mut tx, sid, &SolutionEvent::Merged).await?;
            }
            tx.commit().await?;
        }

        // Reverts invalidate the reverted fix.
        let reverted = git::reverted_commits(&c.message);
        if !reverted.is_empty() {
            let mut targets = Vec::new();
            for r in &reverted {
                let ids: Vec<Uuid> = sqlx::query_scalar(
                            "SELECT DISTINCT s.id FROM solutions s
                               LEFT JOIN solution_sources ss ON ss.solution_id = s.id
                               LEFT JOIN source_artifacts a ON a.id = ss.source_artifact_id AND a.kind IN ('commit', 'pull_request')
                              WHERE s.repository_id = $1
                                AND ((length($2) >= 7 AND left(s.fix_revision, length($2)) = $2)
                                     OR (length($2) >= 7 AND left(a.revision, length($2)) = $2))",
                        )
                        .bind(repo_id)
                        .bind(r)
                        .fetch_all(&ctx.pool)
                        .await?;
                targets.extend(ids);
                sources::mark_state(&ctx.pool, repo_id, SourceKind::Commit, r, "reverted").await?;
            }
            targets.sort();
            targets.dedup();
            for sid in targets {
                let mut tx = ctx.pool.begin().await?;
                trusted_evidence(
                    &mut tx,
                    org_id,
                    sid,
                    EvidenceKind::Revert,
                    EvidenceResult::Fail,
                    Some(c.sha.clone()),
                    None,
                    Some(source_id),
                    format!("revert:{}", c.sha),
                    "fix was reverted",
                )
                .await?;
                solutions::apply_event(&mut tx, sid, &SolutionEvent::Reverted).await?;
                audit::record(
                    &mut *tx,
                    Some(org_id),
                    &Actor::system("webhook:github"),
                    "solution.invalidated",
                    "solution",
                    Some(sid.to_string()),
                    serde_json::json!({ "reason": "revert", "commit": c.sha }),
                )
                .await?;
                tx.commit().await?;
            }
        }

        if default_branch_push {
            for p in &c.changed_paths {
                if git::is_dependency_manifest(p) {
                    dep_dirs.push(git::manifest_dir(p));
                }
            }
        }
    }
    if !dep_dirs.is_empty() {
        dep_dirs.sort();
        dep_dirs.dedup();
        jobs::enqueue(
            &ctx.pool,
            NewJob::new(
                JobKind::Revalidate,
                format!("revalidate:{repo_id}:{after}"),
                serde_json::to_value(RevalidateJob {
                    org_id,
                    repo_id,
                    reason: "dependency_change".into(),
                    dirs: dep_dirs,
                    revision: Some(after.clone()),
                })
                .unwrap_or_default(),
            )
            .org(org_id),
        )
        .await?;
    }
    Ok(())
}

async fn on_pull_request(
    ctx: &WorkerContext,
    org_id: Uuid,
    repo_id: Uuid,
    event: GitEvent,
) -> Result<(), JobError> {
    let GitEvent::PullRequest {
        number,
        action,
        merged,
        merge_commit_sha,
        head_sha,
        head_ref,
        title,
        url,
        updated_at,
        ..
    } = event
    else {
        unreachable!("process_git_event routes only PullRequest here");
    };
    let state = if merged {
        "merged"
    } else if action == "closed" {
        "closed"
    } else {
        "open"
    };
    let mut tx = ctx.pool.begin().await?;
    let source_id = sources::upsert(
        &mut tx,
        &NewSource {
            org_id,
            repo_id,
            kind: SourceKind::PullRequest,
            external_id: number.to_string(),
            url,
            revision: merge_commit_sha.clone().or(Some(head_sha.clone())),
            title: Some(title),
            state: Some(state.into()),
            metadata: serde_json::json!({ "head_ref": head_ref, "head_sha": head_sha, "provider_updated_at": updated_at }),
        },
    )
    .await?;
    if !sources::observation_is_current(&mut tx, source_id, updated_at).await? {
        tx.commit().await?;
        return Ok(());
    }
    tx.commit().await?;
    let linked = solutions::ids_for_fix(
        &ctx.pool,
        repo_id,
        std::slice::from_ref(&head_sha),
        Some(&head_ref),
    )
    .await?;
    for sid in linked {
        let mut tx = ctx.pool.begin().await?;
        sources::link(&mut tx, sid, source_id, "pull_request").await?;
        if merged {
            // Merged is corroboration, not verification or deployment.
            trusted_evidence(
                &mut tx,
                org_id,
                sid,
                EvidenceKind::Merge,
                EvidenceResult::Info,
                merge_commit_sha.clone(),
                None,
                Some(source_id),
                format!("pr:{number}"),
                "pull request merged",
            )
            .await?;
            solutions::apply_event(&mut tx, sid, &SolutionEvent::Merged).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

async fn on_repository_changed(
    ctx: &WorkerContext,
    org_id: Uuid,
    repo_id: Uuid,
    delivery_id: String,
    received_at: Option<chrono::DateTime<chrono::Utc>>,
    event: GitEvent,
) -> Result<(), JobError> {
    let GitEvent::RepositoryChanged { action, repo, .. } = event else {
        unreachable!("process_git_event routes only RepositoryChanged here");
    };
    let mut tx = ctx.pool.begin().await?;
    // Use durable receipt time for retries; processing time must never make an
    // old delivery look newer than a lifecycle event already applied.
    let received_at = match received_at {
        Some(at) => at,
        None => sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT received_at FROM webhook_deliveries WHERE organization_id = $1 AND provider = 'github' AND delivery_id = $2
             UNION ALL
             SELECT created_at FROM jobs WHERE organization_id = $1 AND kind = 'process_git_event' AND payload->>'delivery_id' = $2
             ORDER BY 1 LIMIT 1",
        ).bind(org_id).bind(&delivery_id).fetch_optional(&mut *tx).await?
        .ok_or_else(|| JobError::Permanent("repository lifecycle job is missing its original receipt time".into()))?,
    };
    if !repos::apply_provider_lifecycle(
        &mut tx,
        org_id,
        repo_id,
        &action,
        &repo.full_name,
        repo.url.as_deref(),
        repo.default_branch.as_deref(),
        repo.updated_at,
        received_at,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(());
    }
    match action.as_str() {
        "deleted" | "archived" => {
            sqlx::query("UPDATE source_artifacts SET metadata = metadata || jsonb_build_object('unavailable_previous_state', state), state = 'unavailable' WHERE repository_id = $1 AND kind <> 'session' AND state IS DISTINCT FROM 'unavailable'")
                .bind(repo_id).execute(&mut *tx).await?;
        }
        "unarchived" => {
            sqlx::query("UPDATE source_artifacts SET state = metadata->>'unavailable_previous_state', metadata = metadata - 'unavailable_previous_state' WHERE repository_id = $1 AND state = 'unavailable' AND metadata ? 'unavailable_previous_state' AND EXISTS (SELECT 1 FROM repositories WHERE id = $1 AND status = 'active')")
                .bind(repo_id).execute(&mut *tx).await?;
        }
        _ => {}
    }
    audit::record(
        &mut *tx,
        Some(org_id),
        &Actor::system("webhook:github"),
        "repository.changed",
        "repository",
        Some(repo_id.to_string()),
        serde_json::json!({ "action": action, "delivery": delivery_id }),
    )
    .await?;
    if matches!(
        action.as_str(),
        "privatized" | "publicized" | "transferred" | "renamed"
    ) {
        jobs::enqueue(
            &mut *tx,
            NewJob::new(
                JobKind::Revalidate,
                format!("revalidate:{repo_id}:{delivery_id}"),
                serde_json::to_value(RevalidateJob {
                    org_id,
                    repo_id,
                    reason: "permission_change".into(),
                    dirs: vec![],
                    revision: None,
                })
                .unwrap_or_default(),
            )
            .org(org_id),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn on_permission_changed(
    ctx: &WorkerContext,
    org_id: Uuid,
    repo_id: Uuid,
    event: GitEvent,
) -> Result<(), JobError> {
    let GitEvent::PermissionChanged { action, .. } = event else {
        unreachable!("process_git_event routes only PermissionChanged here");
    };
    audit::record(&ctx.pool, Some(org_id), &Actor::system("webhook:github"), "repository.permissions_changed", "repository", Some(repo_id.to_string()), serde_json::json!({ "action": action, "note": "Antibody access is governed by its own grants; review repository grants." })).await?;
    Ok(())
}
