//! Embeddings.

use crate::{JobError, WorkerContext};
use antibody_core::model::JobKind;
use antibody_core::normalize;
use antibody_db::jobs::{self, NewJob};
use antibody_db::orgs;
use antibody_db::solutions;
use uuid::Uuid;

pub(super) async fn embed_solution(ctx: &WorkerContext, solution_id: Uuid) -> Result<(), JobError> {
    let mut conn = ctx.pool.acquire().await?;
    let Some(sol) = solutions::get_system(&mut conn, solution_id).await? else {
        return Ok(());
    };
    drop(conn);
    let org = orgs::get(&ctx.pool, sol.organization_id)
        .await?
        .ok_or_else(|| JobError::Permanent("org missing".into()))?;
    let Some(provider) = ctx.providers.embedding_for(org.external_providers_approved) else {
        // No usable provider: full-text and exact search still work.
        return Ok(());
    };
    let d = provider.descriptor().clone();
    let content = solutions::embedding_content(&sol);
    let hash = normalize::sha256_hex(content.as_bytes());
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT content_hash FROM embeddings WHERE solution_id = $1 AND provider = $2 AND model = $3 AND model_version = $4",
    )
    .bind(solution_id)
    .bind(&d.provider)
    .bind(&d.model)
    .bind(&d.model_version)
    .fetch_optional(&ctx.pool)
    .await?;
    if existing.as_deref() == Some(hash.as_str()) {
        return Ok(());
    }
    let vectors = provider.embed(&[content]).await.map_err(|e| {
        if e.is_transient() {
            JobError::Retry(e.to_string())
        } else {
            JobError::Permanent(e.to_string())
        }
    })?;
    let vector = vectors
        .into_iter()
        .next()
        .ok_or_else(|| JobError::Retry("no vector returned".into()))?;
    let dims = vector.len() as i32;
    sqlx::query(
        "INSERT INTO embeddings (id, organization_id, solution_id, content_hash, provider, model, model_version, dimensions, embedding)
         SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9 WHERE EXISTS (SELECT 1 FROM solutions WHERE id = $3)
         ON CONFLICT (solution_id, provider, model, model_version) DO UPDATE
            SET content_hash = EXCLUDED.content_hash, dimensions = EXCLUDED.dimensions,
                embedding = EXCLUDED.embedding, created_at = now()",
    )
    .bind(Uuid::now_v7())
    .bind(sol.organization_id)
    .bind(solution_id)
    .bind(&hash)
    .bind(&d.provider)
    .bind(&d.model)
    .bind(&d.model_version)
    .bind(dims)
    .bind(pgvector::Vector::from(vector))
    .execute(&ctx.pool)
    .await?;
    Ok(())
}

/// Enqueue embeddings for solutions missing one for the current provider.
pub async fn backfill_embeddings(ctx: &WorkerContext) -> anyhow::Result<u64> {
    let Some(p) = ctx.providers.embedding.as_ref() else {
        return Ok(0);
    };
    let d = p.descriptor();
    let ids: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT s.id, s.organization_id FROM solutions s
          WHERE s.state NOT IN ('withdrawn')
            AND NOT EXISTS (SELECT 1 FROM embeddings e WHERE e.solution_id = s.id AND e.provider = $1 AND e.model = $2 AND e.model_version = $3)
          LIMIT 5000",
    )
    .bind(&d.provider)
    .bind(&d.model)
    .bind(&d.model_version)
    .fetch_all(&ctx.pool)
    .await?;
    let mut n = 0;
    for (id, org) in ids {
        if jobs::enqueue(
            &ctx.pool,
            NewJob::new(
                JobKind::EmbedSolution,
                format!("solution:{id}"),
                serde_json::json!({ "solution_id": id }),
            )
            .org(org),
        )
        .await?
        .is_some()
        {
            n += 1;
        }
    }
    Ok(n)
}
