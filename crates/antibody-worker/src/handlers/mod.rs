//! Job handlers. Every handler is idempotent: re-running a job after a crash
//! or lease expiry produces the same end state.

mod checks;
mod correlation;
mod embed;
mod extraction;
mod git_event;
mod maintenance;

pub use checks::ci_lookup;
pub use embed::backfill_embeddings;

use crate::{JobError, WorkerContext};
use antibody_core::model::{EvidenceKind, EvidenceResult, JobKind};
use antibody_db::evidence::{self, NewEvidence};
use antibody_db::jobs::ClaimedJob;
use antibody_integrations::github::GitEvent;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct GitEventJob {
    pub org_id: Uuid,
    pub repo_id: Uuid,
    pub delivery_id: String,
    #[serde(default)]
    pub received_at: Option<chrono::DateTime<chrono::Utc>>,
    pub event: GitEvent,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RevalidateJob {
    pub org_id: Uuid,
    pub repo_id: Uuid,
    pub reason: String,
    /// Directories whose manifests changed ("" = repository root).
    #[serde(default)]
    pub dirs: Vec<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

pub async fn dispatch(ctx: &WorkerContext, job: &ClaimedJob) -> Result<(), JobError> {
    let kind = JobKind::from_str(&job.kind).map_err(|e| JobError::Permanent(e.to_string()))?;
    match kind {
        JobKind::ExtractSession => {
            let id = payload_uuid(&job.payload, "session_id")?;
            extraction::extract_session(ctx, id, job.attempts, job.max_attempts).await
        }
        JobKind::EmbedSolution => {
            embed::embed_solution(ctx, payload_uuid(&job.payload, "solution_id")?).await
        }
        JobKind::ProcessGitEvent => {
            let p: GitEventJob = serde_json::from_value(job.payload.clone())
                .map_err(|e| JobError::Permanent(e.to_string()))?;
            git_event::process_git_event(ctx, p).await
        }
        JobKind::CiLookup => ci_lookup(ctx, payload_uuid(&job.payload, "ci_event_id")?).await,
        JobKind::Revalidate => {
            let p: RevalidateJob = serde_json::from_value(job.payload.clone())
                .map_err(|e| JobError::Permanent(e.to_string()))?;
            maintenance::revalidate(ctx, p).await
        }
        JobKind::RetentionSweep => maintenance::retention_sweep(ctx).await,
    }
}

fn payload_uuid(v: &serde_json::Value, key: &str) -> Result<Uuid, JobError> {
    v.get(key)
        .and_then(|x| x.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| JobError::Permanent(format!("payload missing {key}")))
}

// Shared by extraction and Git/CI handlers.
#[allow(clippy::too_many_arguments)]
async fn trusted_evidence(
    conn: &mut PgConnection,
    org_id: Uuid,
    solution_id: Uuid,
    kind: EvidenceKind,
    result: EvidenceResult,
    revision: Option<String>,
    check_name: Option<String>,
    source: Option<Uuid>,
    dedupe: String,
    detail: &str,
) -> Result<bool, JobError> {
    Ok(evidence::insert(
        conn,
        NewEvidence {
            org_id,
            solution_id,
            kind,
            result,
            trusted: true,
            tested_revision: revision,
            check_name,
            environment: None,
            source_artifact_id: source,
            detail: Some(detail.to_string()),
            dedupe_key: dedupe,
        },
    )
    .await?)
}
