use crate::{DbResult, new_id};
use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct Organization {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub raw_capture_enabled: bool,
    pub raw_retention_days: i32,
    pub external_providers_approved: bool,
    pub external_providers_approved_by: Option<Uuid>,
    pub external_providers_approved_at: Option<DateTime<Utc>>,
    pub ci_cross_repository_search: bool,
    pub webhook_secret_version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLUMNS: &str =
    "id, slug, name, raw_capture_enabled, raw_retention_days, external_providers_approved,
    external_providers_approved_by, external_providers_approved_at, ci_cross_repository_search,
    webhook_secret_version, created_at, updated_at";

pub async fn create<'e>(
    exec: impl PgExecutor<'e>,
    slug: &str,
    name: &str,
) -> DbResult<Organization> {
    let id = new_id();
    let sql = format!(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) RETURNING {COLUMNS}"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(slug)
        .bind(name)
        .fetch_one(exec)
        .await?)
}

pub async fn get(pool: &PgPool, id: Uuid) -> DbResult<Option<Organization>> {
    let sql = format!("SELECT {COLUMNS} FROM organizations WHERE id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn get_by_slug(pool: &PgPool, slug: &str) -> DbResult<Option<Organization>> {
    let sql = format!("SELECT {COLUMNS} FROM organizations WHERE slug = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(slug)
        .fetch_optional(pool)
        .await?)
}

pub async fn list(pool: &PgPool) -> DbResult<Vec<Organization>> {
    let sql = format!("SELECT {COLUMNS} FROM organizations ORDER BY created_at");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?)
}

#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    pub raw_capture_enabled: Option<bool>,
    pub raw_retention_days: Option<i32>,
    pub external_providers_approved: Option<bool>,
    pub ci_cross_repository_search: Option<bool>,
}

pub async fn update_settings(
    pool: &PgPool,
    org_id: Uuid,
    actor: Uuid,
    patch: &SettingsPatch,
) -> DbResult<Organization> {
    if let Some(days) = patch.raw_retention_days
        && !(0..=3650).contains(&days)
    {
        return Err(antibody_core::error::DomainError::invalid(
            "raw_retention_days must be between 0 and 3650",
        )
        .into());
    }
    let sql = format!(
        "UPDATE organizations SET
            raw_capture_enabled = COALESCE($2, raw_capture_enabled),
            raw_retention_days = COALESCE($3, raw_retention_days),
            external_providers_approved = COALESCE($4, external_providers_approved),
            external_providers_approved_by = CASE WHEN $4 IS NULL THEN external_providers_approved_by
                                                  WHEN $4 THEN $6 ELSE NULL END,
            external_providers_approved_at = CASE WHEN $4 IS NULL THEN external_providers_approved_at
                                                  WHEN $4 THEN now() ELSE NULL END,
            ci_cross_repository_search = COALESCE($5, ci_cross_repository_search),
            updated_at = now()
          WHERE id = $1
        RETURNING {COLUMNS}"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(patch.raw_capture_enabled)
        .bind(patch.raw_retention_days)
        .bind(patch.external_providers_approved)
        .bind(patch.ci_cross_repository_search)
        .bind(actor)
        .fetch_one(pool)
        .await?)
}

pub async fn rotate_webhook_secret(pool: &PgPool, org_id: Uuid) -> DbResult<i32> {
    Ok(sqlx::query_scalar(
        "UPDATE organizations SET webhook_secret_version = webhook_secret_version + 1, updated_at = now()
          WHERE id = $1 RETURNING webhook_secret_version",
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?)
}
