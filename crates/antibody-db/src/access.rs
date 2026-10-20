//! Principals, authentication contexts and repository access scopes.
//!
//! Access is computed *before* any search, ranking or source expansion, and
//! every downstream query filters by `AccessScope::readable_ids()`.

use crate::{DbResult, parse_enum};
use antibody_core::model::{Permission, PrincipalKind, RepositoryProvider, Role, Scope};
use sqlx::{PgExecutor, PgPool};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Principal {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub kind: PrincipalKind,
    pub role: Role,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    Token { token_id: Uuid },
    BrowserSession { user_id: Uuid, csrf_token: String },
}

/// The authenticated caller.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub principal: Principal,
    pub scopes: Vec<Scope>,
    /// Optional per-token repository allow-list (intersected with grants).
    pub token_repository_ids: Option<Vec<Uuid>>,
    pub method: AuthMethod,
}

impl AuthContext {
    pub fn org_id(&self) -> Uuid {
        self.principal.organization_id
    }
    pub fn is_admin(&self) -> bool {
        self.principal.role == Role::Admin && self.scopes.contains(&Scope::Admin)
    }
    pub fn has_scope(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope) || (scope != Scope::Admin && self.is_admin())
    }
    pub fn actor_label(&self) -> String {
        format!("{}:{}", self.principal.kind, self.principal.display_name)
    }
}

#[derive(Debug, Clone)]
pub struct RepoAccess {
    pub id: Uuid,
    pub full_name: String,
    pub provider: RepositoryProvider,
    pub permission: Permission,
}

/// Repositories the caller may touch. Built once per request.
#[derive(Debug, Clone)]
pub struct AccessScope {
    pub organization_id: Uuid,
    pub principal_id: Option<Uuid>,
    pub repos: BTreeMap<Uuid, RepoAccess>,
}

impl AccessScope {
    pub fn readable_ids(&self) -> Vec<Uuid> {
        self.repos.keys().copied().collect()
    }

    pub fn can_read(&self, repo_id: Uuid) -> bool {
        self.repos.contains_key(&repo_id)
    }

    pub fn can_write(&self, repo_id: Uuid) -> bool {
        self.repos
            .get(&repo_id)
            .is_some_and(|r| r.permission == Permission::Write)
    }

    /// Resolve a repository by full name (case-insensitive) within scope.
    pub fn find_by_name(&self, full_name: &str) -> Option<&RepoAccess> {
        self.repos
            .values()
            .find(|r| r.full_name.eq_ignore_ascii_case(full_name))
    }

    /// A scope covering a fixed set of repositories (system callers such as
    /// CI lookups that are not principals).
    pub fn system(organization_id: Uuid, repos: Vec<RepoAccess>) -> Self {
        Self {
            organization_id,
            principal_id: None,
            repos: repos.into_iter().map(|r| (r.id, r)).collect(),
        }
    }
}

#[derive(sqlx::FromRow)]
struct RepoRow {
    id: Uuid,
    full_name: String,
    provider: String,
    permission: String,
}

/// Compute the access scope for an authenticated caller.
pub async fn access_scope(pool: &PgPool, auth: &AuthContext) -> DbResult<AccessScope> {
    let rows: Vec<RepoRow> = if auth.principal.role == Role::Admin {
        sqlx::query_as(
            "SELECT id, full_name, provider, 'write' AS permission
               FROM repositories
              WHERE organization_id = $1 AND status = 'active'",
        )
        .bind(auth.org_id())
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT r.id, r.full_name, r.provider, g.permission
               FROM repository_grants g
               JOIN repositories r ON r.id = g.repository_id
              WHERE g.principal_id = $1
                AND r.organization_id = $2
                AND g.organization_id = $2
                AND r.status = 'active'",
        )
        .bind(auth.principal.id)
        .bind(auth.org_id())
        .fetch_all(pool)
        .await?
    };

    let mut repos = BTreeMap::new();
    for row in rows {
        if let Some(allow) = &auth.token_repository_ids
            && !allow.contains(&row.id)
        {
            continue;
        }
        repos.insert(
            row.id,
            RepoAccess {
                id: row.id,
                full_name: row.full_name,
                provider: parse_enum(&row.provider)?,
                permission: parse_enum(&row.permission)?,
            },
        );
    }
    Ok(AccessScope {
        organization_id: auth.org_id(),
        principal_id: Some(auth.principal.id),
        repos,
    })
}

#[derive(sqlx::FromRow)]
pub(crate) struct PrincipalRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub kind: String,
    pub role: String,
    pub display_name: String,
}

impl PrincipalRow {
    pub(crate) fn into_principal(self) -> DbResult<Principal> {
        Ok(Principal {
            id: self.id,
            organization_id: self.organization_id,
            kind: parse_enum(&self.kind)?,
            role: parse_enum(&self.role)?,
            display_name: self.display_name,
        })
    }
}

pub async fn create_principal<'e>(
    exec: impl PgExecutor<'e>,
    org_id: Uuid,
    kind: PrincipalKind,
    role: Role,
    display_name: &str,
) -> DbResult<Principal> {
    let id = crate::new_id();
    sqlx::query(
        "INSERT INTO principals (id, organization_id, kind, role, display_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(org_id)
    .bind(kind.as_str())
    .bind(role.as_str())
    .bind(display_name)
    .execute(exec)
    .await?;
    Ok(Principal {
        id,
        organization_id: org_id,
        kind,
        role,
        display_name: display_name.to_string(),
    })
}

pub async fn get_principal(pool: &PgPool, org_id: Uuid, id: Uuid) -> DbResult<Option<Principal>> {
    let row: Option<PrincipalRow> = sqlx::query_as(
        "SELECT id, organization_id, kind, role, display_name FROM principals
          WHERE id = $1 AND organization_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?;
    row.map(PrincipalRow::into_principal).transpose()
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct PrincipalListRow {
    pub id: Uuid,
    pub kind: String,
    pub role: String,
    pub display_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub username: Option<String>,
}

pub async fn list_principals(pool: &PgPool, org_id: Uuid) -> DbResult<Vec<PrincipalListRow>> {
    Ok(sqlx::query_as(
        "SELECT p.id, p.kind, p.role, p.display_name, p.created_at, p.revoked_at, u.username
           FROM principals p LEFT JOIN users u ON u.principal_id = p.id
          WHERE p.organization_id = $1
          ORDER BY p.created_at",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?)
}

pub async fn grant_repository(
    pool: &PgPool,
    org_id: Uuid,
    principal_id: Uuid,
    repository_id: Uuid,
    permission: Permission,
) -> DbResult<()> {
    // The joins guarantee both rows belong to the same organization.
    let res = sqlx::query(
        "INSERT INTO repository_grants (principal_id, repository_id, organization_id, permission)
         SELECT p.id, r.id, $1, $4
           FROM principals p, repositories r
          WHERE p.id = $2 AND r.id = $3 AND p.organization_id = $1 AND r.organization_id = $1
         ON CONFLICT (principal_id, repository_id) DO UPDATE SET permission = EXCLUDED.permission",
    )
    .bind(org_id)
    .bind(principal_id)
    .bind(repository_id)
    .bind(permission.as_str())
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(antibody_core::error::DomainError::not_found("principal or repository").into());
    }
    Ok(())
}

pub async fn revoke_grant(
    pool: &PgPool,
    org_id: Uuid,
    principal_id: Uuid,
    repository_id: Uuid,
) -> DbResult<bool> {
    let res = sqlx::query(
        "DELETE FROM repository_grants WHERE organization_id = $1 AND principal_id = $2 AND repository_id = $3",
    )
    .bind(org_id)
    .bind(principal_id)
    .bind(repository_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Scope for CI lookups, which run without a principal: the failing
/// repository only, or every active repository when the organization opted
/// into cross-repository CI search.
pub async fn ci_scope(
    pool: &PgPool,
    org_id: Uuid,
    repo_id: Uuid,
    cross_repo: bool,
) -> DbResult<AccessScope> {
    let rows: Vec<RepoRow> = sqlx::query_as(
        "SELECT id, full_name, provider, 'read' AS permission FROM repositories
          WHERE organization_id = $1 AND status = 'active' AND ($3 OR id = $2)",
    )
    .bind(org_id)
    .bind(repo_id)
    .bind(cross_repo)
    .fetch_all(pool)
    .await?;
    let mut repos = Vec::with_capacity(rows.len());
    for r in rows {
        repos.push(RepoAccess {
            id: r.id,
            full_name: r.full_name,
            provider: parse_enum(&r.provider)?,
            permission: parse_enum(&r.permission)?,
        });
    }
    Ok(AccessScope::system(org_id, repos))
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct GrantRow {
    pub principal_id: Uuid,
    pub principal_name: String,
    pub repository_id: Uuid,
    pub repository: String,
    pub permission: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn list_grants(pool: &PgPool, org_id: Uuid) -> DbResult<Vec<GrantRow>> {
    Ok(sqlx::query_as(
        "SELECT g.principal_id, p.display_name AS principal_name, g.repository_id, r.full_name AS repository,
                g.permission, g.created_at
           FROM repository_grants g
           JOIN principals p ON p.id = g.principal_id
           JOIN repositories r ON r.id = g.repository_id
          WHERE g.organization_id = $1
          ORDER BY p.display_name, r.full_name",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?)
}
