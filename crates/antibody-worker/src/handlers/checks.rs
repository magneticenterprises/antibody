//! Flow C/D: CI checks, deployments and CI failure lookup.

use super::trusted_evidence;
use crate::{JobError, WorkerContext};
use antibody_core::api::ContextSearchRequest;
use antibody_core::model::{EvidenceKind, EvidenceResult, RetrievalChannel, SourceKind};
use antibody_core::state::SolutionEvent;
use antibody_core::{normalize, text};
use antibody_db::audit::{self, Actor};
use antibody_db::solutions;
use antibody_db::sources::{self, NewSource};
use antibody_db::{ci, orgs, outcomes};
use antibody_integrations::github::{self, GitEvent};
use uuid::Uuid;

pub(super) async fn on_check_completed(
    ctx: &WorkerContext,
    org_id: Uuid,
    repo_id: Uuid,
    event: GitEvent,
) -> Result<(), JobError> {
    let GitEvent::CheckCompleted {
        kind,
        id,
        run_attempt,
        completed_at,
        name,
        conclusion,
        head_sha,
        head_branch,
        url,
        excerpt,
        ..
    } = event
    else {
        unreachable!("process_git_event routes only CheckCompleted here");
    };
    let source_kind = if kind == "workflow_run" {
        SourceKind::WorkflowRun
    } else {
        SourceKind::CheckRun
    };
    let external_id = format!(
        "{id}:attempt:{run_attempt}:{}:{conclusion}",
        completed_at.map(|at| at.to_rfc3339()).unwrap_or_default()
    );
    let mut tx = ctx.pool.begin().await?;
    // Serialize observations and transitions within the repository, in the
    // same order as extraction/deletion. A delayed attempt cannot race a rerun.
    let present: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM repositories WHERE id = $1 AND organization_id = $2 FOR NO KEY UPDATE",
    )
    .bind(repo_id)
    .bind(org_id)
    .fetch_optional(&mut *tx)
    .await?;
    if present.is_none() {
        return Ok(());
    }
    let source_id = sources::upsert(
        &mut tx,
        &NewSource {
            org_id,
            repo_id,
            kind: source_kind,
            external_id: external_id.clone(),
            url: url.clone(),
            revision: Some(head_sha.clone()),
            title: Some(name.clone()),
            state: Some(conclusion.clone()),
            metadata: serde_json::json!({
                "head_branch": head_branch, "run_id": id, "run_attempt": run_attempt,
                "check_identity": text::one_line(&name, 200),
                "completed_at": completed_at,
            }),
        },
    )
    .await?;
    let current = antibody_db::evidence::check_is_current(&mut tx, source_id).await?;
    let passed = github::is_success(&conclusion);
    if passed || github::is_failure(&conclusion) {
        let linked: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT s.id FROM solutions s
               LEFT JOIN solution_sources ss ON ss.solution_id = s.id
               LEFT JOIN source_artifacts a ON a.id = ss.source_artifact_id
              WHERE s.repository_id = $1
                AND (s.fix_revision = $2 OR
                     (ss.relation IN ('fix_commit', 'pull_request') AND
                      (a.revision = s.fix_revision OR a.metadata->>'head_sha' = s.fix_revision) AND
                      (a.revision = $2 OR a.metadata->>'head_sha' = $2)))
              ORDER BY s.id",
        )
        .bind(repo_id)
        .bind(&head_sha)
        .fetch_all(&mut *tx)
        .await?;
        for sid in linked {
            sources::link(&mut tx, sid, source_id, "check_run").await?;
            if trusted_evidence(
                &mut tx,
                org_id,
                sid,
                EvidenceKind::CiCheck,
                if passed {
                    EvidenceResult::Pass
                } else {
                    EvidenceResult::Fail
                },
                Some(head_sha.clone()),
                Some(name.clone()),
                Some(source_id),
                format!("{kind}:{external_id}"),
                if passed {
                    "signed CI check passed"
                } else {
                    "signed CI check failed"
                },
            )
            .await?
                && current
            {
                let before = solutions::get_system(&mut tx, sid).await?.map(|s| s.state);
                let after = solutions::apply_event(
                    &mut tx,
                    sid,
                    &if passed {
                        SolutionEvent::TrustedCheckPassed
                    } else {
                        SolutionEvent::TrustedCheckFailed
                    },
                )
                .await?;
                if before.as_deref() != Some("verified") && after.state.as_str() == "verified" {
                    audit::record(
                        &mut *tx,
                        Some(org_id),
                        &Actor::system("webhook:github"),
                        "solution.verified",
                        "solution",
                        Some(sid.to_string()),
                        serde_json::json!({ "check": name, "revision": head_sha }),
                    )
                    .await?;
                }
            }
        }
        if current {
            outcomes::reconcile_by_ci(&mut *tx, repo_id, &head_sha, &name, passed).await?;
        }
    }
    tx.commit().await?;

    if github::is_failure(&conclusion) {
        let raw = excerpt.unwrap_or_default();
        let redacted = antibody_core::redact::redact(&text::clean(&raw, 4000)).text;
        let signature = normalize::extract_error_signatures(&redacted, 1)
            .into_iter()
            .next()
            .unwrap_or_else(|| normalize::normalize_error_signature(&format!("{name} failed")));
        let (ci_id, _) = ci::insert(
            &ctx.pool,
            ci::NewCiEvent {
                org_id,
                repo_id,
                source: &format!("github_{kind}"),
                revision: &head_sha,
                check_name: &name,
                conclusion: &conclusion,
                signature: &signature,
                excerpt: Some(redacted.as_str()).filter(|e| !e.is_empty()),
                url: url.as_deref(),
                component_path: None,
            },
        )
        .await?;
        // A previous lookup can fail after the CI row was committed. Retrying
        // that event must resume the lookup instead of treating it as done.
        if ci::get(&ctx.pool, ci_id)
            .await?
            .is_some_and(|e| e.status != "searched")
        {
            ci_lookup(ctx, ci_id).await?;
        }
    }
    Ok(())
}

pub(super) async fn on_deployment_status(
    ctx: &WorkerContext,
    org_id: Uuid,
    repo_id: Uuid,
    event: GitEvent,
) -> Result<(), JobError> {
    let GitEvent::DeploymentStatus {
        id,
        state,
        sha,
        environment,
        url,
        ..
    } = event
    else {
        unreachable!("process_git_event routes only DeploymentStatus here");
    };
    let mut tx = ctx.pool.begin().await?;
    let source_id = sources::upsert(
        &mut tx,
        &NewSource {
            org_id,
            repo_id,
            kind: SourceKind::Deployment,
            external_id: id.clone(),
            url,
            revision: Some(sha.clone()),
            title: environment.clone(),
            state: Some(state.clone()),
            metadata: serde_json::json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    if state == "success" {
        let linked =
            solutions::ids_linked_to_revisions(&ctx.pool, repo_id, std::slice::from_ref(&sha))
                .await?;
        for sid in linked {
            let mut tx = ctx.pool.begin().await?;
            sources::link(&mut tx, sid, source_id, "deployment").await?;
            trusted_evidence(
                &mut tx,
                org_id,
                sid,
                EvidenceKind::Deployment,
                EvidenceResult::Info,
                Some(sha.clone()),
                None,
                Some(source_id),
                format!("deploy:{id}"),
                "deployed (not verification)",
            )
            .await?;
            solutions::apply_event(&mut tx, sid, &SolutionEvent::Deployed).await?;
            tx.commit().await?;
        }
    }
    Ok(())
}

/// Run a CI failure through the shared retrieval engine.
pub async fn ci_lookup(ctx: &WorkerContext, ci_event_id: Uuid) -> Result<(), JobError> {
    let Some(ev) = ci::get(&ctx.pool, ci_event_id).await? else {
        return Ok(());
    };
    let org = orgs::get(&ctx.pool, ev.organization_id)
        .await?
        .ok_or_else(|| JobError::Permanent("org missing".into()))?;
    let scope = antibody_db::access::ci_scope(
        &ctx.pool,
        ev.organization_id,
        ev.repository_id,
        org.ci_cross_repository_search,
    )
    .await?;
    let req = ContextSearchRequest {
        task: format!("CI check `{}` failed: {}", ev.check_name, ev.signature),
        repository: Some(ev.repository.clone()),
        revision: Some(ev.revision.clone()),
        error: ev.excerpt.clone().or(Some(ev.signature.clone())),
        component_path: ev.component_path.clone(),
        ..Default::default()
    };
    match ctx
        .search
        .search(
            &scope,
            &req,
            RetrievalChannel::Ci,
            org.external_providers_approved,
        )
        .await
    {
        Ok(resp) => {
            ci::set_result(
                &ctx.pool,
                ci_event_id,
                Some(resp.retrieval_id),
                resp.suggestions.len() as i32,
                "searched",
            )
            .await?;
            Ok(())
        }
        Err(e) => {
            ci::set_result(&ctx.pool, ci_event_id, None, 0, "failed").await?;
            Err(JobError::Retry(e.to_string()))
        }
    }
}
