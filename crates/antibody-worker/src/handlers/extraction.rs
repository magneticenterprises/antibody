//! Flow A: extraction.

use super::correlation;
use crate::extract::{self, Extraction, LlmExtractError};
use crate::{JobError, WorkerContext};
use antibody_core::api::SessionIngestRequest;
use antibody_core::model::{CaptureMode, EvidenceKind, EvidenceResult, JobKind};
use antibody_core::normalize;
use antibody_db::audit::{self, Actor};
use antibody_db::evidence::{self, NewEvidence};
use antibody_db::jobs::{self, NewJob};
use antibody_db::solutions::{self, ExtractedSolution, UpsertOutcome};
use antibody_db::{orgs, repos, sessions};
use uuid::Uuid;

pub(super) async fn extract_session(
    ctx: &WorkerContext,
    session_id: Uuid,
    attempt: i32,
    max_attempts: i32,
) -> Result<(), JobError> {
    let Some(session) = sessions::get_system(&ctx.pool, session_id).await? else {
        // Deleted before processing: nothing to do.
        return Ok(());
    };
    if session.extraction_status == "succeeded" || session.extraction_status == "skipped" {
        // The extraction transaction may have committed before correlation
        // failed. Resume from durable links, even after raw payload retention.
        return correlation::correlate_session(ctx, &session).await;
    }
    let Some(payload) = sessions::load_payload(&ctx.pool, session_id).await? else {
        sessions::mark_extraction_failed(
            &ctx.pool,
            session_id,
            "payload no longer available (retention)",
            true,
        )
        .await?;
        return Ok(());
    };
    let req: SessionIngestRequest = match serde_json::from_value(payload) {
        Ok(r) => r,
        Err(e) => {
            sessions::mark_extraction_failed(
                &ctx.pool,
                session_id,
                "stored payload is unreadable",
                true,
            )
            .await?;
            return Err(JobError::Permanent(format!("unreadable payload: {e}")));
        }
    };
    sessions::mark_extraction_running(&ctx.pool, session_id).await?;
    let org = orgs::get(&ctx.pool, session.organization_id)
        .await?
        .ok_or_else(|| JobError::Permanent("organization missing".into()))?;

    let final_attempt = attempt >= max_attempts - 1;
    let extraction: Extraction = match ctx.providers.llm_for(org.external_providers_approved) {
        None => extract::heuristic(&req),
        Some(llm) => match extract::with_llm(llm.as_ref(), &req).await {
            Ok(ex) => ex,
            Err(LlmExtractError::Provider(e)) if e.is_transient() && !final_attempt => {
                sessions::mark_extraction_failed(
                    &ctx.pool,
                    session_id,
                    "model provider unavailable; will retry",
                    false,
                )
                .await?;
                return Err(JobError::Retry(format!("llm provider: {e}")));
            }
            Err(e) => {
                tracing::warn!(session = %session_id, error = ?e, "LLM extraction failed; using heuristic extractor");
                let mut ex = extract::heuristic(&req);
                ex.extractor = format!("{} (llm fallback)", ex.extractor);
                ex
            }
        },
    };

    let component_id = match &session.component_path {
        Some(p) => repos::match_component(&ctx.pool, session.repository_id, p)
            .await?
            .map(|c| c.id),
        None => None,
    };
    let fix_revision = if req.worktree_dirty {
        None
    } else {
        session.revision.clone()
    };

    let mut tx = ctx.pool.begin().await?;
    if !sessions::lock_for_extraction(
        &mut tx,
        session.organization_id,
        session.repository_id,
        session_id,
    )
    .await?
    {
        return Ok(());
    }
    let mut solution_ids = Vec::new();
    for c in &extraction.candidates {
        let outcome = solutions::upsert_extracted(
            &mut tx,
            &ExtractedSolution {
                org_id: session.organization_id,
                repo_id: session.repository_id,
                component_id,
                session_id,
                owner_principal_id: session.principal_id,
                title: c.title.clone(),
                symptom: c.symptom.clone(),
                root_cause: c.root_cause.clone(),
                remedy: c.remedy.clone(),
                error_signatures: c.error_signatures.clone(),
                symbols: c.symbols.clone(),
                paths: c.paths.clone(),
                applicability: c.applicability.clone(),
                fix_revision: fix_revision.clone(),
                fix_branch: (!req.worktree_dirty)
                    .then(|| session.branch.clone())
                    .flatten(),
                confidence: c.confidence,
                extractor: extraction.extractor.clone(),
            },
        )
        .await?;
        let id = outcome.id();
        if c.claims_tests_passed && !matches!(outcome, UpsertOutcome::Unchanged { .. }) {
            evidence::insert(
                &mut tx,
                NewEvidence {
                    org_id: session.organization_id,
                    solution_id: id,
                    kind: EvidenceKind::AgentClaim,
                    result: EvidenceResult::Pass,
                    trusted: false,
                    tested_revision: fix_revision.clone(),
                    check_name: None,
                    environment: Some(format!("{} session", session.agent_type)),
                    source_artifact_id: None,
                    detail: Some("the agent stated that tests passed (not verification)".into()),
                    dedupe_key: format!("claim:{session_id}"),
                },
            )
            .await?;
        }
        jobs::enqueue(
            &mut *tx,
            NewJob::new(
                JobKind::EmbedSolution,
                format!("solution:{id}"),
                serde_json::json!({ "solution_id": id }),
            )
            .org(session.organization_id),
        )
        .await?;
        solution_ids.push((id, outcome));
    }

    for w in &extraction.workflow {
        let fp = normalize::short_hash(
            format!(
                "{}|{}",
                session.repository_id,
                normalize::query_terms(&w.proposed_change, 24).join(" ")
            )
            .as_bytes(),
        );
        sqlx::query(
            "INSERT INTO workflow_improvements (id, organization_id, repository_id, source_session_id, target_scope, title,
                                               proposed_change, fingerprint, owner_principal_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (organization_id, fingerprint) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(session.organization_id)
        .bind(session.repository_id)
        .bind(session_id)
        .bind(&w.scope)
        .bind(&w.title)
        .bind(&w.proposed_change)
        .bind(&fp)
        .bind(session.principal_id)
        .execute(&mut *tx)
        .await?;
    }

    let status = if extraction.candidates.is_empty() {
        "skipped"
    } else {
        "succeeded"
    };
    sqlx::query(
        "UPDATE sessions SET extraction_status = $2, extractor = $3, extraction_error = NULL, processed_at = now() WHERE id = $1",
    )
    .bind(session_id)
    .bind(status)
    .bind(&extraction.extractor)
    .execute(&mut *tx)
    .await?;
    // Structured-only capture: the transcript is discarded once extraction is done.
    if session.capture_mode == CaptureMode::StructuredOnly.as_str() {
        sqlx::query("DELETE FROM session_payloads WHERE session_id = $1")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(
        &mut *tx,
        Some(session.organization_id),
        &Actor::system("worker"),
        "session.extracted",
        "session",
        Some(session_id.to_string()),
        serde_json::json!({
            "extractor": extraction.extractor,
            "solutions": solution_ids.iter().map(|(id, o)| serde_json::json!({
                "id": id,
                "result": match o { UpsertOutcome::Created { .. } => "created", UpsertOutcome::Corroborated { .. } => "corroborated", UpsertOutcome::Unchanged { .. } => "unchanged" },
            })).collect::<Vec<_>>(),
            "workflow_candidates": extraction.workflow.len(),
        }),
    )
    .await?;
    tx.commit().await?;

    // Events may have arrived before the session (e.g. CI already passed on
    // the fix revision): correlate with what we have already observed.
    correlation::correlate_session(ctx, &session).await
}
