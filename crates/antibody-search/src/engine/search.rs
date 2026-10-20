//! Hybrid search: validation, candidate scoring and retrieval recording.

use super::stages::embed_query;
use super::{MAX_ERROR_CHARS, MAX_TASK_CHARS, SearchEngine, SearchError, Signals};
use crate::views::{source_link, verification_summary};
use antibody_core::api::{ContextSearchRequest, ContextSearchResponse, SourceLink, Suggestion};
use antibody_core::applicability::{self, CallerContext};
use antibody_core::error::DomainError;
use antibody_core::model::{ApplicabilityStatus, RetrievalChannel, SolutionState};
use antibody_core::{MAX_SUGGESTIONS, UNTRUSTED_NOTICE, normalize, redact, text};
use antibody_db::access::AccessScope;
use antibody_db::retrievals::{self, CandidateRef, NewRetrieval};
use antibody_db::solutions::{self, SolutionRow};
use antibody_db::{evidence, sources};
use chrono::Utc;
use std::collections::HashMap;
use std::time::Instant;
use uuid::Uuid;

impl SearchEngine {
    fn validate(req: &ContextSearchRequest) -> Result<(), DomainError> {
        if req.task.trim().is_empty() && req.error.as_deref().is_none_or(|e| e.trim().is_empty()) {
            return Err(DomainError::invalid("task or error is required"));
        }
        if req.task.chars().count() > MAX_TASK_CHARS {
            return Err(DomainError::invalid(format!(
                "task exceeds {MAX_TASK_CHARS} characters"
            )));
        }
        if req
            .error
            .as_deref()
            .is_some_and(|e| e.chars().count() > MAX_ERROR_CHARS)
        {
            return Err(DomainError::invalid(format!(
                "error exceeds {MAX_ERROR_CHARS} characters"
            )));
        }
        if req.files.len() > 200 || req.dependencies.len() > 500 {
            return Err(DomainError::invalid("too many files or dependencies"));
        }
        if let Some(l) = req.limit
            && l == 0
        {
            return Err(DomainError::invalid("limit must be between 1 and 3"));
        }
        Ok(())
    }

    /// Hybrid search within `scope`. Never errors on degraded stages: the
    /// response is marked `degraded` instead (fail open).
    pub async fn search(
        &self,
        scope: &AccessScope,
        req: &ContextSearchRequest,
        channel: RetrievalChannel,
        external_providers_approved: bool,
    ) -> Result<ContextSearchResponse, SearchError> {
        self.search_with(scope, req, channel, external_providers_approved, true)
            .await
    }

    /// Like [`search`](Self::search); `record = false` is a preview that is
    /// not stored as a retrieval (so it never counts as usage and cannot
    /// receive outcomes).
    pub async fn search_with(
        &self,
        scope: &AccessScope,
        req: &ContextSearchRequest,
        channel: RetrievalChannel,
        external_providers_approved: bool,
        record_retrieval: bool,
    ) -> Result<ContextSearchResponse, SearchError> {
        let started = Instant::now();
        Self::validate(req)?;
        let limit = req
            .limit
            .map(|l| l as usize)
            .unwrap_or(MAX_SUGGESTIONS)
            .clamp(1, MAX_SUGGESTIONS);
        let mut degraded_reasons = Vec::new();

        let target_repo = match req.repository.as_deref() {
            Some(name) => {
                let found = scope.find_by_name(name).map(|r| r.id);
                if found.is_none() {
                    degraded_reasons.push("repository not found or not accessible; searched other accessible repositories".to_string());
                }
                found
            }
            None => None,
        };
        let readable = scope.readable_ids();

        // Every entry point shares redacted search text, including the
        // embedding provider, indexed signals and stored fingerprint.
        let task = redact::redact(&req.task).text;
        let error_text = redact::redact(req.error.as_deref().unwrap_or_default()).text;
        let mut signatures = normalize::extract_error_signatures(&error_text, 8);
        if signatures.is_empty() && !error_text.trim().is_empty() && error_text.lines().count() <= 3
        {
            let n = normalize::normalize_error_signature(error_text.lines().next().unwrap_or(""));
            if n.len() >= 6 {
                signatures.push(n);
            }
        }
        for s in normalize::extract_error_signatures(&task, 4) {
            if !signatures.contains(&s) {
                signatures.push(s);
            }
        }
        let sig_hashes: Vec<String> = signatures
            .iter()
            .map(|s| normalize::signature_hash(s))
            .collect();
        let combined = format!("{task}\n{error_text}");
        let symbols = normalize::extract_symbols(&combined, 24);
        let mut paths: Vec<String> = req
            .files
            .iter()
            .map(|f| normalize::normalize_path(f))
            .collect();
        paths.extend(normalize::extract_paths(&combined, 24));
        paths.sort();
        paths.dedup();
        let mut term_source = combined.clone();
        if let Some(c) = &req.component_path {
            term_source.push(' ');
            term_source.push_str(c);
        }
        let terms = normalize::query_terms(&term_source, 32);
        let tsquery = normalize::or_tsquery(&terms);

        let mut signals: HashMap<Uuid, Signals> = HashMap::new();
        let mut suggestions = Vec::new();
        let mut suppressed = 0u32;
        let mut candidate_refs = Vec::new();

        if !readable.is_empty() {
            // Embed the query concurrently with the indexed stages.
            let embedder = self.providers.embedding_for(external_providers_approved);
            let embed_fut = embed_query(embedder.clone(), &combined, self.cfg.embed_timeout);
            let indexed_fut = self.indexed_stages(
                scope.organization_id,
                &readable,
                &sig_hashes,
                &symbols,
                &paths,
                &tsquery,
            );
            let (embedded, indexed) = tokio::join!(embed_fut, indexed_fut);

            match indexed {
                Ok(rows) => {
                    for (id, sig, sym, path, rank) in rows {
                        let s = signals.entry(id).or_default();
                        s.signature_match |= sig;
                        s.symbol_overlap = s.symbol_overlap.max(sym);
                        s.path_overlap = s.path_overlap.max(path);
                        s.fts_rank = s.fts_rank.max(rank);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "indexed search stage failed");
                    degraded_reasons.push("indexed search timed out or failed".into());
                }
            }

            match (embedder, embedded) {
                (None, _) => degraded_reasons.push("semantic search unavailable: no embedding provider configured for this organization".into()),
                (Some(_), Err(reason)) => degraded_reasons.push(reason),
                (Some(p), Ok(vector)) => match self.vector_stage(scope.organization_id, &readable, p.as_ref(), vector).await {
                    Ok(rows) => {
                        for (id, sim) in rows {
                            signals.entry(id).or_default().vector_similarity = sim;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "vector search stage failed");
                        degraded_reasons.push("vector search failed".into());
                    }
                },
            }

            let max_rank = signals.values().map(|s| s.fts_rank).fold(0.0f32, f32::max);
            let mut base: Vec<(Uuid, f32, Vec<String>)> = signals
                .iter()
                .filter_map(|(id, s)| {
                    let mut reasons = Vec::new();
                    let mut score = 0.0f32;
                    if s.signature_match {
                        score += 1.0;
                        reasons.push("exact error signature".to_string());
                    }
                    if s.symbol_overlap > 0 {
                        score += 0.12 * s.symbol_overlap.min(3) as f32;
                        reasons.push(format!("{} shared symbol(s)", s.symbol_overlap));
                    }
                    if s.path_overlap > 0 {
                        score += 0.1 * s.path_overlap.min(3) as f32;
                        reasons.push(format!("{} shared file path(s)", s.path_overlap));
                    }
                    if s.fts_rank > 0.0 && max_rank > 0.0 {
                        score += 0.5 * (s.fts_rank / max_rank);
                        reasons.push("full-text match".to_string());
                    }
                    if s.vector_similarity >= self.cfg.min_vector_similarity {
                        score += 0.8 * (s.vector_similarity - 0.3).max(0.0);
                        reasons.push(format!("semantic similarity {:.2}", s.vector_similarity));
                    }
                    (score > 0.0).then_some((*id, score, reasons))
                })
                .collect();
            base.sort_by(|a, b| b.1.total_cmp(&a.1));
            base.truncate(15);

            let ids: Vec<Uuid> = base.iter().map(|b| b.0).collect();
            // Re-fetch with the scope filter: defense in depth.
            let rows =
                solutions::get_many_in_scope(&self.pool, scope.organization_id, &readable, &ids)
                    .await?;
            let rows: HashMap<Uuid, SolutionRow> = rows.into_iter().map(|r| (r.id, r)).collect();
            let trusted = evidence::trusted_summary(&self.pool, &ids).await?;
            let trusted: HashMap<Uuid, (i64, Option<String>, Option<String>)> = trusted
                .into_iter()
                .map(|(id, n, c, r)| (id, (n, c, r)))
                .collect();
            let srcs = sources::for_solutions(&self.pool, &ids).await?;

            let mut scored: Vec<(f32, Suggestion)> = Vec::new();
            for (id, base_score, reasons) in base {
                let Some(row) = rows.get(&id) else { continue };
                let state = row.state_enum();
                if matches!(state, SolutionState::Withdrawn | SolutionState::Superseded) {
                    continue;
                }
                let same_repo = target_repo.is_none_or(|t| t == row.repository_id);
                let ctx = CallerContext {
                    dependencies: &req.dependencies,
                    component_path: req.component_path.as_deref(),
                    files: &req.files,
                    platform: req.platform.as_deref(),
                    same_repository: same_repo,
                };
                let assessment = applicability::evaluate(
                    &row.applicability.0,
                    row.needs_revalidation,
                    row.revalidation_reason.as_deref(),
                    &ctx,
                );
                let (tcount, tcheck, trev) = trusted.get(&id).cloned().unwrap_or((0, None, None));
                let evidence_factor = match state {
                    SolutionState::Verified => 1.0,
                    SolutionState::Corroborated => 0.85,
                    _ => 0.65,
                } + (tcount.min(3) as f32) * 0.03;
                let age_days = (Utc::now() - row.updated_at).num_days().max(0) as f32;
                let recency = 0.8 + 0.2 * (-age_days / 180.0).exp();
                let row_sources: Vec<_> = srcs.iter().filter(|s| s.solution_id == id).collect();
                let mut reasons = reasons;
                let mut availability = 1.0;
                if row_sources
                    .iter()
                    .any(|s| matches!(s.state.as_deref(), Some("deleted") | Some("unavailable")))
                {
                    availability = 0.9;
                    reasons.push("some sources are no longer available".into());
                }
                if !same_repo {
                    reasons.push(format!("from {}", row.repository));
                }
                let score =
                    base_score * evidence_factor * assessment.score_factor * recency * availability;
                candidate_refs.push((id, score));
                if assessment.status == ApplicabilityStatus::Incompatible
                    && !req.include_incompatible
                {
                    suppressed += 1;
                    continue;
                }
                if score < 0.12 {
                    continue;
                }
                let verification =
                    verification_summary(row, tcount, tcheck.as_deref(), trev.as_deref());
                let mut links: Vec<SourceLink> = row_sources
                    .iter()
                    .map(|s| source_link(s, self.cfg.public_url.as_deref()))
                    .collect();
                links.truncate(4);
                scored.push((
                    score,
                    Suggestion {
                        solution_id: id,
                        title: text::one_line(&row.title, 160),
                        issue: text::one_line(&row.symptom, 300),
                        reuse_action: text::one_line(&row.remedy, 500),
                        repository: row.repository.clone(),
                        cross_repository: !same_repo,
                        source_links: links,
                        applicability: assessment,
                        verification,
                        match_reasons: reasons,
                        score: (score * 1000.0).round() / 1000.0,
                        detail_url: self
                            .cfg
                            .public_url
                            .as_ref()
                            .map(|u| format!("{}/knowledge/{}", u.trim_end_matches('/'), id)),
                    },
                ));
            }
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            suggestions = scored.into_iter().take(limit).map(|(_, s)| s).collect();
        }

        let retrieval_id = NewRetrieval::new_id();
        let returned: Vec<Uuid> = suggestions.iter().map(|s| s.solution_id).collect();
        candidate_refs.sort_by(|a, b| b.1.total_cmp(&a.1));
        let candidates: Vec<CandidateRef> = candidate_refs
            .iter()
            .take(20)
            .enumerate()
            .map(|(i, (id, score))| CandidateRef {
                solution_id: *id,
                score: *score,
                rank: i as u32,
                returned: returned.contains(id),
            })
            .collect();
        let context_tokens =
            text::estimate_tokens(&serde_json::to_string(&suggestions).unwrap_or_default()) as i32;
        let latency_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
        let degraded = degraded_reasons
            .iter()
            .any(|r| !r.starts_with("semantic search unavailable"));
        let record = NewRetrieval {
            id: retrieval_id,
            org_id: scope.organization_id,
            principal_id: scope.principal_id,
            channel,
            repository_id: target_repo,
            task_fingerprint: normalize::task_fingerprint(&combined, req.repository.as_deref()),
            agent_session_id: req
                .agent_session_id
                .as_deref()
                .map(|s| text::one_line(s, 128)),
            candidates,
            result_count: suggestions.len() as i32,
            latency_ms,
            context_tokens,
            degraded,
        };
        if record_retrieval && let Err(e) = retrievals::insert(&self.pool, &record).await {
            tracing::warn!(error = %e, "failed to record retrieval");
        }
        Ok(ContextSearchResponse {
            retrieval_id,
            suggestions,
            degraded,
            degraded_reasons,
            suppressed_incompatible: suppressed,
            latency_ms: latency_ms as u32,
            notice: UNTRUSTED_NOTICE.to_string(),
        })
    }
}
