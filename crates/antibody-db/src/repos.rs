use crate::{DbResult, new_id};
use antibody_core::model::RepositoryProvider;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepositoryRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub provider: String,
    pub provider_repo_id: Option<String>,
    pub full_name: String,
    pub url: Option<String>,
    pub default_branch: String,
    pub status: String,
    pub last_event_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub session_count: i64,
    pub solution_count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ComponentRow {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub name: String,
    pub path_glob: String,
    pub package_name: Option<String>,
    pub ecosystem: Option<String>,
}

pub fn valid_full_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('/').collect();
    name.len() <= 200
        && parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        })
}

pub struct NewRepository<'a> {
    pub org_id: Uuid,
    pub provider: RepositoryProvider,
    pub full_name: &'a str,
    pub url: Option<&'a str>,
    pub provider_repo_id: Option<&'a str>,
    pub default_branch: Option<&'a str>,
}

/// Create or reactivate a repository. Returns (id, created).
pub async fn upsert(pool: &PgPool, r: NewRepository<'_>) -> DbResult<(Uuid, bool)> {
    if !valid_full_name(r.full_name) {
        return Err(antibody_core::error::DomainError::invalid(
            "repository full_name must look like owner/name",
        )
        .into());
    }
    let row: (Uuid, bool) = sqlx::query_as(
        "INSERT INTO repositories (id, organization_id, provider, full_name, url, provider_repo_id, default_branch)
         VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, 'main'))
         ON CONFLICT (organization_id, provider, full_name) DO UPDATE
            SET url = COALESCE(EXCLUDED.url, repositories.url),
                provider_repo_id = COALESCE(EXCLUDED.provider_repo_id, repositories.provider_repo_id),
                default_branch = COALESCE($7, repositories.default_branch),
                status = 'active', disconnected_reason = NULL
         RETURNING id, (xmax = 0) AS created",
    )
    .bind(new_id())
    .bind(r.org_id)
    .bind(r.provider.as_str())
    .bind(r.full_name)
    .bind(r.url)
    .bind(r.provider_repo_id)
    .bind(r.default_branch)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

const SELECT: &str = "SELECT r.id, r.organization_id, r.provider, r.provider_repo_id, r.full_name, r.url, r.default_branch,
        r.status, r.last_event_at, r.created_at,
        (SELECT count(*) FROM sessions s WHERE s.repository_id = r.id) AS session_count,
        (SELECT count(*) FROM solutions x WHERE x.repository_id = r.id) AS solution_count
   FROM repositories r";

/// List repositories restricted to the caller's readable ids.
pub async fn list_in_scope(
    pool: &PgPool,
    org_id: Uuid,
    ids: &[Uuid],
) -> DbResult<Vec<RepositoryRow>> {
    let sql =
        format!("{SELECT} WHERE r.organization_id = $1 AND r.id = ANY($2) ORDER BY r.full_name");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(ids)
        .fetch_all(pool)
        .await?)
}

/// Admin-only listing including disconnected repositories.
pub async fn list_all(pool: &PgPool, org_id: Uuid) -> DbResult<Vec<RepositoryRow>> {
    let sql = format!("{SELECT} WHERE r.organization_id = $1 ORDER BY r.full_name");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .fetch_all(pool)
        .await?)
}

pub async fn get(pool: &PgPool, org_id: Uuid, id: Uuid) -> DbResult<Option<RepositoryRow>> {
    let sql = format!("{SELECT} WHERE r.organization_id = $1 AND r.id = $2");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Lookup used by webhooks (system context, already signature-verified).
pub async fn find_by_name(
    pool: &PgPool,
    org_id: Uuid,
    provider: RepositoryProvider,
    full_name: &str,
) -> DbResult<Option<RepositoryRow>> {
    let sql = format!(
        "{SELECT} WHERE r.organization_id = $1 AND r.provider = $2 AND lower(r.full_name) = lower($3) AND r.status = 'active'"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(provider.as_str())
        .bind(full_name)
        .fetch_optional(pool)
        .await?)
}

/// Resolve a signed provider event by immutable identity before trying its name.
/// Lifecycle events can reach disconnected rows; other events cannot ingest them.
/// A name match can bind an unbound registration, including the provider's old
/// name on the first rename/transfer delivery. A conflicting identity never binds.
pub async fn find_for_webhook(
    pool: &PgPool,
    org_id: Uuid,
    provider: RepositoryProvider,
    provider_id: Option<&str>,
    full_name: &str,
    previous_full_name: Option<&str>,
    lifecycle: bool,
) -> DbResult<Option<RepositoryRow>> {
    let sql = format!(
        "{SELECT} WHERE r.organization_id = $1 AND r.provider = $2
         AND ($6 OR r.status = 'active')
         AND (($3::text IS NOT NULL AND r.provider_repo_id = $3)
              OR ((r.provider_repo_id IS NULL OR $3::text IS NULL)
                  AND (lower(r.full_name) = lower($4) OR lower(r.full_name) = lower($5))))
         ORDER BY (r.provider_repo_id = $3) DESC NULLS LAST LIMIT 1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(provider.as_str())
        .bind(provider_id)
        .bind(full_name)
        .bind(previous_full_name)
        .bind(lifecycle)
        .fetch_optional(pool)
        .await?)
}

/// Bind provider identity only while accepting a new authenticated delivery.
pub async fn bind_provider_id(
    conn: &mut sqlx::PgConnection,
    org_id: Uuid,
    repo_id: Uuid,
    provider_id: Option<&str>,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE repositories SET provider_repo_id = $3
         WHERE id = $1 AND organization_id = $2 AND provider_repo_id IS NULL
           AND $3::text IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM repositories r WHERE r.organization_id = $2
                           AND r.provider = 'github' AND r.provider_repo_id = $3)",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(provider_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Apply ordered provider lifecycle state without undoing an administrator's
/// disconnection. The caller holds this transaction through related source writes.
#[allow(clippy::too_many_arguments)]
pub async fn apply_provider_lifecycle(
    conn: &mut sqlx::PgConnection,
    org_id: Uuid,
    repo_id: Uuid,
    action: &str,
    full_name: &str,
    url: Option<&str>,
    default_branch: Option<&str>,
    updated_at: Option<DateTime<Utc>>,
    received_at: DateTime<Utc>,
) -> DbResult<bool> {
    if !valid_full_name(full_name) {
        return Err(antibody_core::error::DomainError::invalid("invalid repository name").into());
    }
    let result = sqlx::query(
        "UPDATE repositories SET full_name = $4, url = COALESCE($5, url),
           default_branch = COALESCE($6, default_branch),
           status = CASE
             WHEN disconnected_reason = 'admin' THEN status
             WHEN $3 IN ('archived', 'deleted') THEN 'disconnected'
             WHEN $3 = 'unarchived' AND disconnected_reason = 'archived' THEN 'active'
             ELSE status END,
           disconnected_reason = CASE
             WHEN disconnected_reason = 'admin' THEN 'admin'
             WHEN $3 IN ('archived', 'deleted') THEN $3
             WHEN $3 = 'unarchived' AND disconnected_reason = 'archived' THEN NULL
             ELSE disconnected_reason END,
           provider_updated_at = COALESCE($7, provider_updated_at), lifecycle_received_at = $8
         WHERE id = $1 AND organization_id = $2
           AND ((provider_updated_at IS NULL AND (lifecycle_received_at IS NULL OR lifecycle_received_at < $8))
                OR provider_updated_at < $7
                OR (provider_updated_at = $7 AND lifecycle_received_at < $8))",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(action)
    .bind(full_name)
    .bind(url.map(|v| antibody_core::redact::redact(v).text))
    .bind(default_branch.map(|v| antibody_core::redact::redact(v).text))
    .bind(updated_at)
    .bind(received_at)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn find_any_provider(
    pool: &PgPool,
    org_id: Uuid,
    full_name: &str,
) -> DbResult<Option<RepositoryRow>> {
    let sql = format!(
        "{SELECT} WHERE r.organization_id = $1 AND lower(r.full_name) = lower($2) AND r.status = 'active' ORDER BY r.created_at LIMIT 1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(full_name)
        .fetch_optional(pool)
        .await?)
}

pub async fn touch_event(pool: &PgPool, repo_id: Uuid) -> DbResult<()> {
    sqlx::query("UPDATE repositories SET last_event_at = now() WHERE id = $1")
        .bind(repo_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_status(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    status: &str,
) -> DbResult<bool> {
    let res =
        sqlx::query("UPDATE repositories SET status = $3, disconnected_reason = CASE WHEN $3 = 'disconnected' THEN 'admin' ELSE NULL END WHERE id = $1 AND organization_id = $2")
            .bind(repo_id)
            .bind(org_id)
            .bind(status)
            .execute(pool)
            .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn components_for(pool: &PgPool, repo_ids: &[Uuid]) -> DbResult<Vec<ComponentRow>> {
    Ok(sqlx::query_as(
        "SELECT id, repository_id, name, path_glob, package_name, ecosystem FROM components
          WHERE repository_id = ANY($1) ORDER BY name",
    )
    .bind(repo_ids)
    .fetch_all(pool)
    .await?)
}

pub async fn add_component(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    name: &str,
    path_glob: &str,
    package_name: Option<&str>,
    ecosystem: Option<&str>,
) -> DbResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO components (id, organization_id, repository_id, name, path_glob, package_name, ecosystem)
         SELECT $1, $2, r.id, $4, $5, $6, $7 FROM repositories r WHERE r.id = $3 AND r.organization_id = $2
         ON CONFLICT (repository_id, name) DO UPDATE SET path_glob = EXCLUDED.path_glob,
             package_name = EXCLUDED.package_name, ecosystem = EXCLUDED.ecosystem
         RETURNING id",
    )
    .bind(new_id())
    .bind(org_id)
    .bind(repo_id)
    .bind(name)
    .bind(path_glob)
    .bind(package_name)
    .bind(ecosystem)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| antibody_core::error::DomainError::not_found("repository"))?;
    Ok(id)
}

/// Find the component whose glob matches a repository-relative path.
pub async fn match_component(
    pool: &PgPool,
    repo_id: Uuid,
    path: &str,
) -> DbResult<Option<ComponentRow>> {
    let comps = components_for(pool, &[repo_id]).await?;
    let norm = antibody_core::normalize::normalize_path(path);
    Ok(comps.into_iter().find(|c| {
        let g = c.path_glob.trim_start_matches("./").to_lowercase();
        let prefix = g.trim_end_matches("**").trim_end_matches('/');
        (!prefix.is_empty() && norm.starts_with(prefix)) || norm == g
    }))
}
