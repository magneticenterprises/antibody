//! Solution detail, propagation candidates and outcome reporting.

use super::{SearchEngine, SearchError};
use crate::views::{source_link, verification_summary};
use antibody_core::UNTRUSTED_NOTICE;
use antibody_core::api::{
    EvidenceView, OutcomeRecorded, OutcomeRequest, PropagationCandidate, PropagationList,
    SolutionDetail, SolutionVersionRef, SourceLink,
};
use antibody_core::error::DomainError;
use antibody_core::model::{EvidenceKind, EvidenceResult, SolutionState};
use antibody_db::access::AccessScope;
use antibody_db::{evidence, outcomes, retrievals, solutions, sources};
use uuid::Uuid;

impl SearchEngine {
    /// Full solution detail with evidence, if readable by the caller.
    pub async fn get_solution(
        &self,
        scope: &AccessScope,
        id: Uuid,
        is_admin: bool,
        include_unscoped: bool,
    ) -> Result<SolutionDetail, SearchError> {
        let readable = scope.readable_ids();
        let row = solutions::get_in_scope(&self.pool, scope.organization_id, &readable, id)
            .await?
            .ok_or_else(|| DomainError::not_found("solution"))?;
        let ev = evidence::for_solutions_in_scope(
            &self.pool,
            scope.organization_id,
            &readable,
            &[id],
            is_admin && include_unscoped,
        )
        .await?;
        let srcs = sources::for_solutions(&self.pool, &[id]).await?;
        let trusted = evidence::trusted_summary(&self.pool, &[id]).await?;
        let (tcount, tcheck, trev) = trusted
            .into_iter()
            .next()
            .map(|(_, n, c, r)| (n, c, r))
            .unwrap_or((0, None, None));
        let others = solutions::other_versions(
            &self.pool,
            scope.organization_id,
            &readable,
            &row.lineage_key,
            id,
        )
        .await?;
        let session_ids = solutions::session_ids(&self.pool, id).await?;
        let public_url = self.cfg.public_url.as_deref();
        let evidence_views = ev
            .into_iter()
            .map(|e| EvidenceView {
                id: e.id,
                kind: e.kind.parse().unwrap_or(EvidenceKind::Outcome),
                result: e.result.parse().unwrap_or(EvidenceResult::Info),
                trusted: e.trusted,
                tested_revision: e.tested_revision,
                check_name: e.check_name,
                environment: e.environment,
                detail: e.detail,
                source: e.source_kind.map(|k| SourceLink {
                    kind: k.parse().unwrap_or(antibody_core::model::SourceKind::Url),
                    label: e.source_title.unwrap_or_else(|| k.clone()),
                    url: e.source_url,
                    revision: e.source_revision,
                    state: e.source_state,
                }),
                observed_at: e.observed_at,
            })
            .collect();
        let suppressed = matches!(
            row.state_enum(),
            SolutionState::Withdrawn | SolutionState::Superseded
        );
        Ok(SolutionDetail {
            id: row.id,
            repository: row.repository.clone(),
            repository_id: row.repository_id,
            component: row.component.clone(),
            title: row.title.clone(),
            symptom: row.symptom.clone(),
            root_cause: row.root_cause.clone(),
            remedy: row.remedy.clone(),
            error_signatures: row.error_signatures.clone(),
            symbols: row.symbols.clone(),
            paths: row.paths.clone(),
            applicability: row.applicability.0.clone(),
            verification: verification_summary(&row, tcount, tcheck.as_deref(), trev.as_deref()),
            version: row.version,
            confidence: row.confidence,
            extractor: row.extractor.clone(),
            corroboration_count: row.corroboration_count,
            fix_revision: row.fix_revision.clone(),
            fix_branch: row.fix_branch.clone(),
            superseded_by: row.superseded_by,
            sources: srcs.iter().map(|s| source_link(s, public_url)).collect(),
            evidence: evidence_views,
            other_versions: others
                .into_iter()
                .map(|(id, version, state, app)| SolutionVersionRef {
                    id,
                    version,
                    state: state.parse().unwrap_or(SolutionState::Candidate),
                    applicability: app.0,
                })
                .collect(),
            session_ids,
            created_at: row.created_at,
            updated_at: row.updated_at,
            can_withdraw: is_admin && row.state_enum() != SolutionState::Withdrawn,
            can_supersede: is_admin && !suppressed,
            notice: UNTRUSTED_NOTICE.to_string(),
        })
    }

    /// Sibling repositories/components that may benefit from a fix (Flow D).
    /// Only locations within the caller's scope are considered.
    pub async fn propagation(
        &self,
        scope: &AccessScope,
        id: Uuid,
    ) -> Result<PropagationList, SearchError> {
        let readable = scope.readable_ids();
        let row = solutions::get_in_scope(&self.pool, scope.organization_id, &readable, id)
            .await?
            .ok_or_else(|| DomainError::not_found("solution"))?;
        let mut out: Vec<PropagationCandidate> = Vec::new();

        // Same normalized error seen failing CI elsewhere.
        let ci: Vec<(Uuid, String, i64)> = sqlx::query_as(
            "SELECT r.id, r.full_name, count(*)
               FROM ci_events c JOIN repositories r ON r.id = c.repository_id
              WHERE c.organization_id = $1 AND c.repository_id = ANY($2) AND c.repository_id <> $3
                AND c.signature_hash = ANY($4)
              GROUP BY r.id, r.full_name ORDER BY count(*) DESC LIMIT 10",
        )
        .bind(scope.organization_id)
        .bind(&readable)
        .bind(row.repository_id)
        .bind(&row.signature_hashes)
        .fetch_all(&self.pool)
        .await?;
        for (rid, name, n) in ci {
            out.push(PropagationCandidate {
                repository_id: rid,
                repository: name,
                component: None,
                reason: format!("{n} CI failure(s) with the same error signature"),
                related_solution_id: None,
                matching_ci_failures: n,
            });
        }

        // Same problem recorded elsewhere without verification.
        let related: Vec<(Uuid, String, Uuid, String)> = sqlx::query_as(
            "SELECT r.id, r.full_name, s.id, s.state
               FROM solutions s JOIN repositories r ON r.id = s.repository_id
              WHERE s.organization_id = $1 AND s.repository_id = ANY($2) AND s.repository_id <> $3
                AND s.lineage_key = $4 AND s.state NOT IN ('verified', 'withdrawn')
              LIMIT 10",
        )
        .bind(scope.organization_id)
        .bind(&readable)
        .bind(row.repository_id)
        .bind(&row.lineage_key)
        .fetch_all(&self.pool)
        .await?;
        for (rid, name, sid, state) in related {
            if let Some(c) = out.iter_mut().find(|c| c.repository_id == rid) {
                c.related_solution_id = Some(sid);
                continue;
            }
            out.push(PropagationCandidate {
                repository_id: rid,
                repository: name,
                component: None,
                reason: format!("same problem recorded there with a {state} fix"),
                related_solution_id: Some(sid),
                matching_ci_failures: 0,
            });
        }

        // Sibling components in the same (mono)repository.
        if let Some(cid) = row.component_id {
            let comps =
                antibody_db::repos::components_for(&self.pool, &[row.repository_id]).await?;
            let ecosystem = comps
                .iter()
                .find(|c| c.id == cid)
                .and_then(|c| c.ecosystem.clone());
            for c in comps
                .into_iter()
                .filter(|c| c.id != cid && (ecosystem.is_none() || c.ecosystem == ecosystem))
                .take(10)
            {
                out.push(PropagationCandidate {
                    repository_id: row.repository_id,
                    repository: row.repository.clone(),
                    component: Some(c.name),
                    reason: "sibling component in the same repository; check whether it shares the affected code path".into(),
                    related_solution_id: None,
                    matching_ci_failures: 0,
                });
            }
        }
        out.truncate(20);
        Ok(PropagationList {
            solution_id: id,
            candidates: out,
            notice: "Candidates only. Antibody never applies changes; open a change in each location after review.".into(),
        })
    }

    /// Record an agent's outcome for a solution it was shown.
    pub async fn report_outcome(
        &self,
        scope: &AccessScope,
        req: &OutcomeRequest,
    ) -> Result<OutcomeRecorded, SearchError> {
        let principal_id = scope
            .principal_id
            .ok_or_else(|| DomainError::forbidden("outcomes require a principal"))?;
        let readable = scope.readable_ids();
        solutions::get_in_scope(
            &self.pool,
            scope.organization_id,
            &readable,
            req.solution_id,
        )
        .await?
        .ok_or_else(|| DomainError::not_found("solution"))?;
        let retrieval = retrievals::get(&self.pool, scope.organization_id, req.retrieval_id)
            .await?
            .ok_or_else(|| DomainError::not_found("retrieval"))?;
        if retrieval.principal_id != Some(principal_id) {
            return Err(DomainError::not_found("retrieval").into());
        }
        let consumer_repo = retrieval.repository_id;
        if consumer_repo.is_some_and(|repo| !scope.can_read(repo)) {
            return Err(DomainError::not_found("retrieval").into());
        }
        if consumer_repo.is_none() && req.check.is_some() {
            return Err(
                DomainError::invalid("check claims require a retrieval with a repository").into(),
            );
        }
        if !retrieval
            .candidates
            .0
            .iter()
            .any(|c| c.solution_id == req.solution_id && c.returned)
        {
            return Err(
                DomainError::invalid("solution was not suggested by this retrieval").into(),
            );
        }
        if let Some(n) = &req.notes
            && n.chars().count() > 4000
        {
            return Err(DomainError::invalid("notes exceed 4000 characters").into());
        }
        if let Some(c) = &req.check
            && (c.name.trim().is_empty() || c.revision.trim().is_empty() || c.revision.len() > 64)
        {
            return Err(
                DomainError::invalid("check name and revision (<= 64 chars) are required").into(),
            );
        }
        let recorded = outcomes::record(
            &self.pool,
            outcomes::NewOutcome {
                org_id: scope.organization_id,
                principal_id,
                repository_id: consumer_repo,
                request: req,
            },
        )
        .await?;
        Ok(OutcomeRecorded {
            id: recorded.id,
            verification: recorded.verification,
            duplicate: recorded.duplicate,
        })
    }
}
