//! Insert or corroborate solutions extracted from sessions.

use super::{SELECT, SolutionRow, apply_event};
use crate::{DbResult, new_id};
use antibody_core::applicability::Applicability;
use antibody_core::model::{EvidenceKind, EvidenceResult};
use antibody_core::normalize;
use antibody_core::state::SolutionEvent;
use sqlx::PgConnection;
use uuid::Uuid;

/// Structured knowledge extracted from one session.
#[derive(Debug, Clone)]
pub struct ExtractedSolution {
    pub org_id: Uuid,
    pub repo_id: Uuid,
    pub component_id: Option<Uuid>,
    pub session_id: Uuid,
    pub owner_principal_id: Option<Uuid>,
    pub title: String,
    pub symptom: String,
    pub root_cause: Option<String>,
    pub remedy: String,
    pub error_signatures: Vec<String>,
    pub symbols: Vec<String>,
    pub paths: Vec<String>,
    pub applicability: Applicability,
    pub fix_revision: Option<String>,
    pub fix_branch: Option<String>,
    pub confidence: f32,
    pub extractor: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Created {
        id: Uuid,
        version: i32,
    },
    Corroborated {
        id: Uuid,
    },
    /// This session was already linked (idempotent re-run).
    Unchanged {
        id: Uuid,
    },
}

impl UpsertOutcome {
    pub fn id(&self) -> Uuid {
        match self {
            UpsertOutcome::Created { id, .. }
            | UpsertOutcome::Corroborated { id }
            | UpsertOutcome::Unchanged { id } => *id,
        }
    }
}

fn keywords(sigs: &[String], symbols: &[String], paths: &[String]) -> String {
    let mut words = Vec::new();
    for s in sigs.iter().chain(symbols).chain(paths) {
        for w in s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if w.len() >= 2 {
                words.push(w.to_ascii_lowercase());
            }
        }
    }
    words.sort();
    words.dedup();
    words.join(" ")
}

fn union(a: &[String], b: &[String], max: usize) -> Vec<String> {
    let mut out: Vec<String> = a.to_vec();
    for x in b {
        if !out.contains(x) && out.len() < max {
            out.push(x.clone());
        }
    }
    out
}

/// Insert a new solution or corroborate an existing one with the same
/// fingerprint (same problem, same applicability). A different
/// applicability for the same problem creates a new version.
///
/// The caller must use a transaction scoped to one repository and acquire no
/// session, solution, or job locks before the shared repository lock.
pub async fn upsert_extracted(
    conn: &mut PgConnection,
    x: &ExtractedSolution,
) -> DbResult<UpsertOutcome> {
    if !crate::sessions::lock_for_extraction(&mut *conn, x.org_id, x.repo_id, x.session_id).await? {
        return Err(sqlx::Error::RowNotFound.into());
    }
    let lineage = normalize::lineage_key(&x.error_signatures, &x.title);
    let applicability_key = x.applicability.key();
    let fingerprint = normalize::solution_fingerprint(&lineage, &applicability_key);
    let sig_hashes: Vec<String> = x
        .error_signatures
        .iter()
        .map(|s| normalize::signature_hash(s))
        .collect();

    let existing = {
        let sql = format!(
            "{SELECT} WHERE s.organization_id = $1 AND s.repository_id = $2 AND s.fingerprint = $3 FOR UPDATE OF s"
        );
        let row: Option<SolutionRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(x.org_id)
            .bind(x.repo_id)
            .bind(&fingerprint)
            .fetch_optional(&mut *conn)
            .await?;
        match row {
            Some(row) => Some(row),
            None => {
                // Earlier fingerprints omitted applicability fields. Compare
                // the stored structured value with the current canonical key
                // before allocating a new version, then upgrade in place.
                let sql = format!(
                    "{SELECT} WHERE s.organization_id = $1 AND s.repository_id = $2 AND s.lineage_key = $3
                     ORDER BY s.version, s.id FOR UPDATE OF s"
                );
                let candidates: Vec<SolutionRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
                    .bind(x.org_id)
                    .bind(x.repo_id)
                    .bind(&lineage)
                    .fetch_all(&mut *conn)
                    .await?;
                let matched = candidates
                    .into_iter()
                    .find(|row| row.applicability.key() == applicability_key);
                if let Some(row) = &matched {
                    sqlx::query("UPDATE solutions SET fingerprint = $2 WHERE id = $1")
                        .bind(row.id)
                        .bind(&fingerprint)
                        .execute(&mut *conn)
                        .await?;
                }
                matched
            }
        }
    };

    if let Some(row) = existing {
        let linked = sqlx::query(
            "INSERT INTO solution_sessions (solution_id, session_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(row.id)
        .bind(x.session_id)
        .execute(&mut *conn)
        .await?;
        if linked.rows_affected() == 0 {
            return Ok(UpsertOutcome::Unchanged { id: row.id });
        }
        let sigs = union(&row.error_signatures, &x.error_signatures, 20);
        let hashes: Vec<String> = sigs.iter().map(|s| normalize::signature_hash(s)).collect();
        let symbols = union(&row.symbols, &x.symbols, 40);
        let paths = union(&row.paths, &x.paths, 40);
        sqlx::query(
            "UPDATE solutions SET corroboration_count = corroboration_count + 1,
                    error_signatures = $2, signature_hashes = $3, symbols = $4, paths = $5, keywords = $6,
                    fix_revision = COALESCE(fix_revision, $7), fix_branch = COALESCE(fix_branch, $8),
                    confidence = GREATEST(confidence, $9), updated_at = now()
              WHERE id = $1",
        )
        .bind(row.id)
        .bind(&sigs)
        .bind(&hashes)
        .bind(&symbols)
        .bind(&paths)
        .bind(keywords(&sigs, &symbols, &paths))
        .bind(&x.fix_revision)
        .bind(&x.fix_branch)
        .bind(x.confidence)
        .execute(&mut *conn)
        .await?;
        crate::evidence::insert(
            &mut *conn,
            crate::evidence::NewEvidence {
                org_id: x.org_id,
                solution_id: row.id,
                kind: EvidenceKind::Corroboration,
                result: EvidenceResult::Info,
                trusted: false,
                tested_revision: x.fix_revision.clone(),
                check_name: None,
                environment: None,
                source_artifact_id: None,
                detail: Some("independently extracted from another session".into()),
                dedupe_key: format!("corroboration:{}", x.session_id),
            },
        )
        .await?;
        apply_event(&mut *conn, row.id, &SolutionEvent::Corroborated).await?;
        link_session_source(&mut *conn, x, row.id).await?;
        return Ok(UpsertOutcome::Corroborated { id: row.id });
    }

    let version: i32 = sqlx::query_scalar(
        "SELECT COALESCE(max(version), 0) + 1 FROM solutions WHERE organization_id = $1 AND repository_id = $2 AND lineage_key = $3",
    )
    .bind(x.org_id)
    .bind(x.repo_id)
    .bind(&lineage)
    .fetch_one(&mut *conn)
    .await?;
    let id = new_id();
    sqlx::query(
        "INSERT INTO solutions (id, organization_id, repository_id, component_id, lineage_key, fingerprint, version,
                                title, symptom, root_cause, remedy, error_signatures, signature_hashes, symbols, paths,
                                keywords, applicability, fix_revision, fix_branch, owner_principal_id, confidence, extractor,
                                merge_state)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22,
                 CASE WHEN $18::text IS NULL THEN 'unknown' ELSE 'unmerged' END)",
    )
    .bind(id)
    .bind(x.org_id)
    .bind(x.repo_id)
    .bind(x.component_id)
    .bind(&lineage)
    .bind(&fingerprint)
    .bind(version)
    .bind(&x.title)
    .bind(&x.symptom)
    .bind(&x.root_cause)
    .bind(&x.remedy)
    .bind(&x.error_signatures)
    .bind(&sig_hashes)
    .bind(&x.symbols)
    .bind(&x.paths)
    .bind(keywords(&x.error_signatures, &x.symbols, &x.paths))
    .bind(sqlx::types::Json(&x.applicability))
    .bind(&x.fix_revision)
    .bind(&x.fix_branch)
    .bind(x.owner_principal_id)
    .bind(x.confidence)
    .bind(&x.extractor)
    .execute(&mut *conn)
    .await?;
    sqlx::query("INSERT INTO solution_sessions (solution_id, session_id) VALUES ($1, $2)")
        .bind(id)
        .bind(x.session_id)
        .execute(&mut *conn)
        .await?;
    link_session_source(&mut *conn, x, id).await?;
    Ok(UpsertOutcome::Created { id, version })
}

async fn link_session_source(
    conn: &mut PgConnection,
    x: &ExtractedSolution,
    solution_id: Uuid,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO solution_sources (solution_id, source_artifact_id, relation)
         SELECT $1, a.id, 'origin_session' FROM source_artifacts a
          WHERE a.repository_id = $2 AND a.kind = 'session' AND a.external_id = $3
         ON CONFLICT DO NOTHING",
    )
    .bind(solution_id)
    .bind(x.repo_id)
    .bind(x.session_id.to_string())
    .execute(&mut *conn)
    .await?;
    Ok(())
}
