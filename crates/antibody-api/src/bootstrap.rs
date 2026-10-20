//! First-run setup: create an organization, an administrator who can sign in
//! to the dashboard, and an admin API token. Secrets are returned once.

use crate::auth;
use antibody_core::model::{PrincipalKind, Role, Scope};
use antibody_db::tokens::{self, NewToken};
use antibody_db::{PgPool, access, orgs};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Bootstrapped {
    pub organization_id: uuid::Uuid,
    pub organization_slug: String,
    pub admin_username: String,
    /// Present only when the password was generated.
    pub admin_password: Option<String>,
    pub admin_token: String,
}

pub const ALL_SCOPES: &[Scope] = &[
    Scope::Admin,
    Scope::SessionsWrite,
    Scope::ContextRead,
    Scope::OutcomesWrite,
    Scope::CiWrite,
];

pub async fn bootstrap(
    pool: &PgPool,
    slug: &str,
    name: &str,
    username: &str,
    password: Option<String>,
) -> anyhow::Result<Bootstrapped> {
    let generated = password.is_none();
    let password = password.unwrap_or_else(|| tokens::random_string(24));
    auth::validate_username(username).map_err(|e| anyhow::anyhow!(e.message))?;
    auth::validate_new_password(&password).map_err(|e| anyhow::anyhow!(e.message))?;
    if slug.is_empty()
        || slug.len() > 63
        || !slug.as_bytes()[0].is_ascii_alphanumeric()
        || !slug
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        anyhow::bail!(
            "organization slug must be 1-63 lowercase letters, digits or hyphens, starting with a letter or digit"
        );
    }
    if name.trim().is_empty() {
        anyhow::bail!("organization name is required");
    }
    if orgs::get_by_slug(pool, slug).await?.is_some() {
        anyhow::bail!("organization {slug} already exists");
    }
    let password_to_hash = password.clone();
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&password_to_hash))
        .await?
        .map_err(|e| anyhow::anyhow!(e.message))?;
    let mut tx = pool.begin().await?;
    let org = orgs::create(&mut *tx, slug, name).await?;
    let principal =
        access::create_principal(&mut *tx, org.id, PrincipalKind::User, Role::Admin, username)
            .await?;
    tokens::create_user(&mut *tx, org.id, principal.id, username, &hash).await?;
    let token = tokens::issue_token_on(
        &mut tx,
        NewToken {
            org_id: org.id,
            principal_id: principal.id,
            name: "bootstrap admin token",
            scopes: ALL_SCOPES,
            repository_ids: None,
            expires_at: None,
            created_by: Some(principal.id),
        },
    )
    .await?;
    antibody_db::audit::record(
        &mut *tx,
        Some(org.id),
        &antibody_db::audit::Actor::system("bootstrap"),
        "organization.bootstrapped",
        "organization",
        Some(org.id.to_string()),
        serde_json::json!({ "admin": username }),
    )
    .await?;
    tx.commit().await?;
    Ok(Bootstrapped {
        organization_id: org.id,
        organization_slug: org.slug,
        admin_username: username.to_string(),
        admin_password: generated.then_some(password),
        admin_token: token.token,
    })
}
