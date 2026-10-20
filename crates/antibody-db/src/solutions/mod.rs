//! Solutions: deduplicated, versioned knowledge with a verification state
//! machine (see `antibody_core::state`).

mod lookup;
mod transitions;
mod upsert;

pub use lookup::*;
pub use transitions::*;
pub use upsert::*;

use crate::{DbResult, parse_enum};
use antibody_core::applicability::Applicability;
use antibody_core::model::{DeploymentState, MergeState, SolutionState};
use antibody_core::normalize;
use antibody_core::state::SolutionStatus;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SolutionRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub repository_id: Uuid,
    pub repository: String,
    pub component_id: Option<Uuid>,
    pub component: Option<String>,
    pub lineage_key: String,
    pub fingerprint: String,
    pub version: i32,
    pub title: String,
    pub symptom: String,
    pub root_cause: Option<String>,
    pub remedy: String,
    pub error_signatures: Vec<String>,
    pub signature_hashes: Vec<String>,
    pub symbols: Vec<String>,
    pub paths: Vec<String>,
    pub applicability: sqlx::types::Json<Applicability>,
    pub state: String,
    pub needs_revalidation: bool,
    pub revalidation_reason: Option<String>,
    pub merge_state: String,
    pub deployment_state: String,
    pub fix_revision: Option<String>,
    pub fix_branch: Option<String>,
    pub owner_principal_id: Option<Uuid>,
    pub superseded_by: Option<Uuid>,
    pub confidence: f32,
    pub extractor: String,
    pub corroboration_count: i32,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SolutionRow {
    pub fn status(&self) -> DbResult<SolutionStatus> {
        Ok(SolutionStatus {
            state: parse_enum(&self.state)?,
            merge_state: parse_enum(&self.merge_state)?,
            deployment_state: parse_enum(&self.deployment_state)?,
            needs_revalidation: self.needs_revalidation,
            revalidation_reason: self.revalidation_reason.clone(),
        })
    }
    pub fn state_enum(&self) -> SolutionState {
        self.state.parse().unwrap_or(SolutionState::Candidate)
    }
    pub fn merge_enum(&self) -> MergeState {
        self.merge_state.parse().unwrap_or(MergeState::Unknown)
    }
    pub fn deployment_enum(&self) -> DeploymentState {
        self.deployment_state
            .parse()
            .unwrap_or(DeploymentState::Unknown)
    }
}

pub const SELECT: &str = "SELECT s.id, s.organization_id, s.repository_id, r.full_name AS repository, s.component_id,
        c.name AS component, s.lineage_key, s.fingerprint, s.version, s.title, s.symptom, s.root_cause, s.remedy,
        s.error_signatures, s.signature_hashes, s.symbols, s.paths, s.applicability, s.state, s.needs_revalidation,
        s.revalidation_reason, s.merge_state, s.deployment_state, s.fix_revision, s.fix_branch, s.owner_principal_id,
        s.superseded_by, s.confidence, s.extractor, s.corroboration_count, s.last_verified_at, s.created_at, s.updated_at
   FROM solutions s
   JOIN repositories r ON r.id = s.repository_id
   LEFT JOIN components c ON c.id = s.component_id";

/// Fetch a solution only if its repository is readable by the caller.
pub async fn get_in_scope(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    id: Uuid,
) -> DbResult<Option<SolutionRow>> {
    let sql = format!(
        "{SELECT} WHERE s.id = $1 AND s.organization_id = $2 AND s.repository_id = ANY($3)"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(org_id)
        .bind(repo_ids)
        .fetch_optional(pool)
        .await?)
}

pub async fn get_system(conn: &mut PgConnection, id: Uuid) -> DbResult<Option<SolutionRow>> {
    let sql = format!("{SELECT} WHERE s.id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

pub async fn get_many_in_scope(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    ids: &[Uuid],
) -> DbResult<Vec<SolutionRow>> {
    let sql = format!(
        "{SELECT} WHERE s.id = ANY($1) AND s.organization_id = $2 AND s.repository_id = ANY($3)"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .bind(org_id)
        .bind(repo_ids)
        .fetch_all(pool)
        .await?)
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter<'a> {
    pub query: Option<&'a str>,
    pub state: Option<SolutionState>,
    pub repository_id: Option<Uuid>,
    pub needs_revalidation: Option<bool>,
    pub limit: i64,
    pub offset: i64,
}

pub async fn list_in_scope(
    pool: &PgPool,
    org_id: Uuid,
    repo_ids: &[Uuid],
    f: &ListFilter<'_>,
) -> DbResult<(Vec<SolutionRow>, i64)> {
    let terms = f
        .query
        .map(|q| normalize::or_tsquery(&normalize::query_terms(q, 16)))
        .filter(|q| !q.is_empty());
    let where_clause = "WHERE s.organization_id = $1 AND s.repository_id = ANY($2)
           AND ($3::text IS NULL OR s.state = $3)
           AND ($4::uuid IS NULL OR s.repository_id = $4)
           AND ($5::bool IS NULL OR s.needs_revalidation = $5)
           AND ($6::text IS NULL OR s.search_tsv @@ to_tsquery('english', $6) OR s.search_tsv @@ to_tsquery('simple', $6))";
    let sql = format!("{SELECT} {where_clause} ORDER BY s.updated_at DESC LIMIT $7 OFFSET $8");
    let rows: Vec<SolutionRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(repo_ids)
        .bind(f.state.map(|s| s.as_str()))
        .bind(f.repository_id)
        .bind(f.needs_revalidation)
        .bind(&terms)
        .bind(f.limit)
        .bind(f.offset)
        .fetch_all(pool)
        .await?;
    let count_sql = format!("SELECT count(*) FROM solutions s {where_clause}");
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count_sql))
        .bind(org_id)
        .bind(repo_ids)
        .bind(f.state.map(|s| s.as_str()))
        .bind(f.repository_id)
        .bind(f.needs_revalidation)
        .bind(&terms)
        .fetch_one(pool)
        .await?;
    Ok((rows, total))
}
