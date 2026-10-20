//! Indexed, full-text and vector candidate stages.

use super::SearchEngine;
use antibody_core::text;
use antibody_providers::EmbeddingProvider;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

impl SearchEngine {
    /// Exact (signature/symbol/path) and full-text candidates, scope-filtered,
    /// under a server-side statement timeout.
    pub(super) async fn indexed_stages(
        &self,
        org_id: Uuid,
        readable: &[Uuid],
        sig_hashes: &[String],
        symbols: &[String],
        paths: &[String],
        tsquery: &str,
    ) -> Result<Vec<(Uuid, bool, i32, i32, f32)>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let timeout_ms = self.cfg.query_timeout.as_millis().max(50);
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SET LOCAL statement_timeout = {timeout_ms}"
        )))
        .execute(&mut *tx)
        .await?;
        let mut out: Vec<(Uuid, bool, i32, i32, f32)> = Vec::new();
        if !sig_hashes.is_empty() || !symbols.is_empty() || !paths.is_empty() {
            let rows: Vec<(Uuid, bool, i32, i32)> = sqlx::query_as(
                "SELECT s.id,
                        s.signature_hashes && $3 AS sig,
                        cardinality(ARRAY(SELECT unnest(s.symbols) INTERSECT SELECT unnest($4::text[])))::int AS sym,
                        cardinality(ARRAY(SELECT unnest(s.paths) INTERSECT SELECT unnest($5::text[])))::int AS path
                   FROM solutions s
                  WHERE s.organization_id = $1
                    AND s.repository_id = ANY($2)
                    AND s.state NOT IN ('withdrawn', 'superseded')
                    AND (s.signature_hashes && $3 OR s.symbols && $4 OR s.paths && $5)
                  LIMIT $6",
            )
            .bind(org_id)
            .bind(readable)
            .bind(sig_hashes)
            .bind(symbols)
            .bind(paths)
            .bind(self.cfg.candidate_limit)
            .fetch_all(&mut *tx)
            .await?;
            out.extend(
                rows.into_iter()
                    .map(|(id, sig, sym, path)| (id, sig, sym, path, 0.0)),
            );
        }
        if !tsquery.is_empty() {
            let rows: Vec<(Uuid, f32)> = sqlx::query_as(
                "SELECT s.id, ts_rank_cd(s.search_tsv, q)::real AS rank
                   FROM solutions s,
                        LATERAL (SELECT to_tsquery('english', $3) || to_tsquery('simple', $3) AS q) qq
                  WHERE s.organization_id = $1
                    AND s.repository_id = ANY($2)
                    AND s.state NOT IN ('withdrawn', 'superseded')
                    AND s.search_tsv @@ q
                  ORDER BY rank DESC
                  LIMIT $4",
            )
            .bind(org_id)
            .bind(readable)
            .bind(tsquery)
            .bind(self.cfg.candidate_limit)
            .fetch_all(&mut *tx)
            .await?;
            out.extend(rows.into_iter().map(|(id, rank)| (id, false, 0, 0, rank)));
        }
        tx.commit().await?;
        Ok(out)
    }

    pub(super) async fn vector_stage(
        &self,
        org_id: Uuid,
        readable: &[Uuid],
        provider: &dyn EmbeddingProvider,
        vector: Vec<f32>,
    ) -> Result<Vec<(Uuid, f32)>, sqlx::Error> {
        let d = provider.descriptor();
        let dims = vector.len() as i32;
        let mut tx = self.pool.begin().await?;
        let timeout_ms = self.cfg.query_timeout.as_millis().max(50);
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SET LOCAL statement_timeout = {timeout_ms}"
        )))
        .execute(&mut *tx)
        .await?;
        let rows: Vec<(Uuid, f64)> = sqlx::query_as(
            "SELECT e.solution_id, 1 - (e.embedding <=> $6) AS sim
               FROM embeddings e JOIN solutions s ON s.id = e.solution_id
              WHERE e.organization_id = $1 AND s.organization_id = $1
                AND s.repository_id = ANY($2)
                AND s.state NOT IN ('withdrawn', 'superseded')
                AND e.provider = $3 AND e.model = $4 AND e.model_version = $5 AND e.dimensions = $7
              ORDER BY e.embedding <=> $6
              LIMIT $8",
        )
        .bind(org_id)
        .bind(readable)
        .bind(&d.provider)
        .bind(&d.model)
        .bind(&d.model_version)
        .bind(pgvector::Vector::from(vector))
        .bind(dims)
        .bind(self.cfg.candidate_limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.into_iter().map(|(id, sim)| (id, sim as f32)).collect())
    }
}

pub(super) async fn embed_query(
    provider: Option<Arc<dyn EmbeddingProvider>>,
    text: &str,
    timeout: Duration,
) -> Result<Vec<f32>, String> {
    let Some(p) = provider else {
        return Err("no embedding provider".into());
    };
    let input = vec![text::truncate_chars(text, 4000)];
    match tokio::time::timeout(timeout, p.embed(&input)).await {
        Ok(Ok(mut v)) if !v.is_empty() => Ok(v.remove(0)),
        Ok(Ok(_)) => Err("embedding provider returned no vector".into()),
        // Provider errors can echo request text or credentials. Expose only
        // a stable failure category to callers.
        Ok(Err(_)) => Err("embedding provider error; semantic stage skipped".into()),
        Err(_) => Err("embedding provider timed out; semantic stage skipped".into()),
    }
}
