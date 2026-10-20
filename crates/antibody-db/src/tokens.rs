//! API tokens, browser users and browser sessions. Only SHA-256 hashes of
//! tokens and session ids are stored; plaintext is returned exactly once.

use crate::access::{AuthContext, AuthMethod, PrincipalRow};
use crate::{DbResult, new_id, parse_enum};
use antibody_core::model::{Role, Scope};
use chrono::{DateTime, Duration, Utc};
use rand::RngExt;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgExecutor, PgPool};
use uuid::Uuid;

pub const TOKEN_PREFIX: &str = "abt_";

/// Generate a high-entropy random string (base62).
pub fn random_string(len: usize) -> String {
    rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

pub fn hash_secret(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

pub fn generate_token() -> String {
    format!("{TOKEN_PREFIX}{}", random_string(43))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct IssuedToken {
    pub id: Uuid,
    /// Plaintext token, shown once.
    pub token: String,
    pub prefix: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct TokenRow {
    pub id: Uuid,
    pub principal_id: Uuid,
    pub principal_name: String,
    pub name: String,
    pub token_prefix: String,
    pub scopes: Vec<String>,
    pub repository_ids: Option<Vec<Uuid>>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

pub struct NewToken<'a> {
    pub org_id: Uuid,
    pub principal_id: Uuid,
    pub name: &'a str,
    pub scopes: &'a [Scope],
    pub repository_ids: Option<&'a [Uuid]>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
}

pub async fn issue_token(pool: &PgPool, t: NewToken<'_>) -> DbResult<IssuedToken> {
    let mut conn = pool.acquire().await?;
    issue_token_on(&mut conn, t).await
}

/// Issue a token using the caller's connection, including its transaction.
pub async fn issue_token_on(conn: &mut PgConnection, t: NewToken<'_>) -> DbResult<IssuedToken> {
    // A token can never exceed its principal: only admins get the admin scope.
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM principals WHERE id = $1 AND organization_id = $2 AND revoked_at IS NULL",
    )
    .bind(t.principal_id)
    .bind(t.org_id)
    .fetch_optional(&mut *conn)
    .await?;
    let role: Role = match role {
        Some(r) => parse_enum(&r)?,
        None => return Err(antibody_core::error::DomainError::not_found("principal").into()),
    };
    if t.scopes.contains(&Scope::Admin) && role != Role::Admin {
        return Err(antibody_core::error::DomainError::forbidden(
            "admin scope requires an admin principal",
        )
        .into());
    }
    if t.scopes.is_empty() {
        return Err(
            antibody_core::error::DomainError::invalid("at least one scope is required").into(),
        );
    }
    if let Some(ids) = t.repository_ids {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM repositories WHERE organization_id = $1 AND id = ANY($2)",
        )
        .bind(t.org_id)
        .bind(ids)
        .fetch_one(&mut *conn)
        .await?;
        if count as usize != ids.len() {
            return Err(antibody_core::error::DomainError::invalid(
                "unknown repository in allow-list",
            )
            .into());
        }
    }

    let token = generate_token();
    let prefix: String = token.chars().take(12).collect();
    let id = new_id();
    let scopes: Vec<String> = t.scopes.iter().map(|s| s.as_str().to_string()).collect();
    sqlx::query(
        "INSERT INTO api_tokens (id, organization_id, principal_id, name, token_prefix, token_hash, scopes, repository_ids, expires_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(t.org_id)
    .bind(t.principal_id)
    .bind(t.name)
    .bind(&prefix)
    .bind(hash_secret(&token))
    .bind(&scopes)
    .bind(t.repository_ids.map(|r| r.to_vec()))
    .bind(t.expires_at)
    .bind(t.created_by)
    .execute(&mut *conn)
    .await?;
    Ok(IssuedToken { id, token, prefix })
}

#[derive(sqlx::FromRow)]
struct TokenAuthRow {
    token_id: Uuid,
    scopes: Vec<String>,
    repository_ids: Option<Vec<Uuid>>,
    #[sqlx(flatten)]
    principal: PrincipalRow,
}

/// Resolve a bearer token to an auth context. Revoked/expired tokens and
/// revoked principals fail closed.
pub async fn authenticate_token(pool: &PgPool, token: &str) -> DbResult<Option<AuthContext>> {
    if !token.starts_with(TOKEN_PREFIX) || token.len() > 128 {
        return Ok(None);
    }
    let row: Option<TokenAuthRow> = sqlx::query_as(
        "UPDATE api_tokens t SET last_used_at = now()
           FROM principals p
          WHERE t.token_hash = $1
            AND p.id = t.principal_id
            AND p.organization_id = t.organization_id
            AND t.revoked_at IS NULL
            AND p.revoked_at IS NULL
            AND (t.expires_at IS NULL OR t.expires_at > now())
        RETURNING t.id AS token_id, t.scopes, t.repository_ids,
                  p.id, p.organization_id, p.kind, p.role, p.display_name",
    )
    .bind(hash_secret(token))
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let principal = row.principal.into_principal()?;
    let mut scopes = Vec::with_capacity(row.scopes.len());
    for s in &row.scopes {
        let scope: Scope = parse_enum(s)?;
        if scope == Scope::Admin && principal.role != Role::Admin {
            continue;
        }
        scopes.push(scope);
    }
    Ok(Some(AuthContext {
        principal,
        scopes,
        token_repository_ids: row.repository_ids,
        method: AuthMethod::Token {
            token_id: row.token_id,
        },
    }))
}

pub async fn list_tokens(pool: &PgPool, org_id: Uuid) -> DbResult<Vec<TokenRow>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.principal_id, p.display_name AS principal_name, t.name, t.token_prefix, t.scopes,
                t.repository_ids, t.created_at, t.expires_at, t.last_used_at, t.revoked_at
           FROM api_tokens t JOIN principals p ON p.id = t.principal_id
          WHERE t.organization_id = $1
          ORDER BY t.created_at DESC",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?)
}

pub async fn revoke_token(pool: &PgPool, org_id: Uuid, token_id: Uuid) -> DbResult<bool> {
    let res = sqlx::query(
        "UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND organization_id = $2 AND revoked_at IS NULL",
    )
    .bind(token_id)
    .bind(org_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Browser users and sessions

#[derive(sqlx::FromRow)]
pub struct UserCredentials {
    pub user_id: Uuid,
    pub password_hash: String,
    pub principal_id: Uuid,
    pub organization_id: Uuid,
}

pub async fn create_user<'e>(
    exec: impl PgExecutor<'e>,
    org_id: Uuid,
    principal_id: Uuid,
    username: &str,
    password_hash: &str,
) -> DbResult<Uuid> {
    let id = new_id();
    sqlx::query(
        "INSERT INTO users (id, organization_id, principal_id, username, password_hash) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(org_id)
    .bind(principal_id)
    .bind(username)
    .bind(password_hash)
    .execute(exec)
    .await?;
    Ok(id)
}

pub async fn find_user_credentials(
    pool: &PgPool,
    username: &str,
) -> DbResult<Option<UserCredentials>> {
    Ok(sqlx::query_as(
        "SELECT u.id AS user_id, u.password_hash, u.principal_id, u.organization_id
           FROM users u JOIN principals p ON p.id = u.principal_id
          WHERE u.username = $1 AND p.revoked_at IS NULL",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?)
}

pub async fn set_user_password(pool: &PgPool, user_id: Uuid, password_hash: &str) -> DbResult<()> {
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(user_id)
        .bind(password_hash)
        .execute(pool)
        .await?;
    Ok(())
}

pub struct NewWebSession {
    /// Plaintext session id for the cookie (shown once).
    pub session_id: String,
    pub csrf_token: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn create_web_session(
    pool: &PgPool,
    creds: &UserCredentials,
    ttl: Duration,
) -> DbResult<NewWebSession> {
    let session_id = random_string(48);
    let csrf_token = random_string(32);
    let expires_at = Utc::now() + ttl;
    sqlx::query(
        "INSERT INTO web_sessions (id_hash, organization_id, user_id, principal_id, csrf_token, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(hash_secret(&session_id))
    .bind(creds.organization_id)
    .bind(creds.user_id)
    .bind(creds.principal_id)
    .bind(&csrf_token)
    .bind(expires_at)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(creds.user_id)
        .execute(pool)
        .await?;
    Ok(NewWebSession {
        session_id,
        csrf_token,
        expires_at,
    })
}

#[derive(sqlx::FromRow)]
struct WebSessionRow {
    user_id: Uuid,
    csrf_token: String,
    #[sqlx(flatten)]
    principal: PrincipalRow,
}

/// Resolve a browser session cookie. Browser sessions carry the scopes of
/// the principal's role.
pub async fn authenticate_web_session(
    pool: &PgPool,
    session_id: &str,
) -> DbResult<Option<AuthContext>> {
    if session_id.len() > 128 {
        return Ok(None);
    }
    let row: Option<WebSessionRow> = sqlx::query_as(
        "UPDATE web_sessions s SET last_seen_at = now()
           FROM principals p
          WHERE s.id_hash = $1
            AND p.id = s.principal_id
            AND p.organization_id = s.organization_id
            AND p.revoked_at IS NULL
            AND s.expires_at > now()
        RETURNING s.user_id, s.csrf_token,
                  p.id, p.organization_id, p.kind, p.role, p.display_name",
    )
    .bind(hash_secret(session_id))
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let principal = row.principal.into_principal()?;
    let scopes = match principal.role {
        Role::Admin => vec![Scope::Admin, Scope::ContextRead, Scope::OutcomesWrite],
        Role::Member => vec![Scope::ContextRead, Scope::OutcomesWrite],
    };
    Ok(Some(AuthContext {
        principal,
        scopes,
        token_repository_ids: None,
        method: AuthMethod::BrowserSession {
            user_id: row.user_id,
            csrf_token: row.csrf_token,
        },
    }))
}

pub async fn delete_web_session(pool: &PgPool, session_id: &str) -> DbResult<()> {
    sqlx::query("DELETE FROM web_sessions WHERE id_hash = $1")
        .bind(hash_secret(session_id))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn username_for(pool: &PgPool, user_id: Uuid) -> DbResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?,
    )
}
