use crate::{DbResult, new_id};
use antibody_core::model::SourceKind;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewSource {
    pub org_id: Uuid,
    pub repo_id: Uuid,
    pub kind: SourceKind,
    pub external_id: String,
    pub url: Option<String>,
    pub revision: Option<String>,
    pub title: Option<String>,
    pub state: Option<String>,
    pub metadata: serde_json::Value,
}

/// Insert or refresh a source artifact. Provider timestamps, when available,
/// order mutable artifacts. Terminal commit/PR state cannot regress on a replay.
pub async fn upsert(conn: &mut PgConnection, s: &NewSource) -> DbResult<Uuid> {
    let clean = |s: &str| antibody_core::redact::redact(s).text;
    let mut metadata = s.metadata.clone();
    antibody_core::redact::redact_json(&mut metadata);
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO source_artifacts (id, organization_id, repository_id, kind, external_id, url, revision, title, state, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (repository_id, kind, external_id) DO UPDATE
            SET url = COALESCE(EXCLUDED.url, source_artifacts.url),
                revision = COALESCE(EXCLUDED.revision, source_artifacts.revision),
                title = COALESCE(EXCLUDED.title, source_artifacts.title),
                state = COALESCE(EXCLUDED.state, source_artifacts.state),
                metadata = source_artifacts.metadata || EXCLUDED.metadata,
                observed_at = now()
         WHERE (source_artifacts.metadata->>'provider_updated_at' IS NULL
                OR (EXCLUDED.metadata->>'provider_updated_at')::timestamptz >=
                   (source_artifacts.metadata->>'provider_updated_at')::timestamptz)
           AND NOT (source_artifacts.kind = 'commit' AND source_artifacts.state IN ('merged', 'reverted')
                    AND COALESCE(EXCLUDED.state, '') NOT IN ('reverted', source_artifacts.state))
           AND NOT (source_artifacts.kind = 'pull_request' AND source_artifacts.state = 'merged'
                    AND COALESCE(EXCLUDED.state, '') <> 'merged')
         RETURNING id",
    )
    .bind(new_id())
    .bind(s.org_id)
    .bind(s.repo_id)
    .bind(s.kind.as_str())
    .bind(clean(&s.external_id))
    .bind(s.url.as_deref().map(clean))
    .bind(s.revision.as_deref().map(clean))
    .bind(s.title.as_deref().map(|t| antibody_core::text::one_line(&clean(t), 300)))
    .bind(s.state.as_deref().map(clean))
    .bind(&metadata)
    .fetch_optional(&mut *conn)
    .await?;
    match inserted {
        Some(id) => Ok(id),
        None => Ok(sqlx::query_scalar(
            "SELECT id FROM source_artifacts WHERE repository_id = $1 AND kind = $2 AND external_id = $3",
        ).bind(s.repo_id).bind(s.kind.as_str()).bind(clean(&s.external_id)).fetch_one(conn).await?),
    }
}

/// Whether a timestamped source observation is current after upsert. Call in
/// the same transaction before applying evidence/state derived from the event.
pub async fn observation_is_current(
    conn: &mut PgConnection,
    id: Uuid,
    updated_at: Option<DateTime<Utc>>,
) -> DbResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT metadata->>'provider_updated_at' IS NULL OR
                COALESCE((metadata->>'provider_updated_at')::timestamptz <= $2, false)
         FROM source_artifacts WHERE id = $1",
    )
    .bind(id)
    .bind(updated_at)
    .fetch_one(conn)
    .await?)
}

pub async fn link(
    conn: &mut PgConnection,
    solution_id: Uuid,
    source_id: Uuid,
    relation: &str,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO solution_sources (solution_id, source_artifact_id, relation) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(solution_id)
    .bind(source_id)
    .bind(relation)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn mark_state(
    pool: &PgPool,
    repo_id: Uuid,
    kind: SourceKind,
    external_id: &str,
    state: &str,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE source_artifacts SET state = $4, observed_at = now() WHERE repository_id = $1 AND kind = $2 AND external_id = $3",
    )
    .bind(repo_id)
    .bind(kind.as_str())
    .bind(external_id)
    .bind(state)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SourceRow {
    pub solution_id: Uuid,
    pub relation: String,
    pub kind: String,
    pub external_id: String,
    pub url: Option<String>,
    pub revision: Option<String>,
    pub title: Option<String>,
    pub state: Option<String>,
    pub observed_at: DateTime<Utc>,
}

pub async fn for_solutions(pool: &PgPool, ids: &[Uuid]) -> DbResult<Vec<SourceRow>> {
    Ok(sqlx::query_as(
        "SELECT ss.solution_id, ss.relation, a.kind, a.external_id, a.url, a.revision, a.title, a.state, a.observed_at
           FROM solution_sources ss JOIN source_artifacts a ON a.id = ss.source_artifact_id
          WHERE ss.solution_id = ANY($1)
          ORDER BY a.observed_at DESC",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?)
}
