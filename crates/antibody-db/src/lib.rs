//! SQLx data access for Antibody. All queries are parameterized; dynamic SQL
//! is never built from user input. Every query is scoped by organization.

pub mod access;
pub mod audit;
pub mod ci;
pub mod evidence;
pub mod jobs;
pub mod orgs;
pub mod outcomes;
pub mod repos;
pub mod retention;
pub mod retrievals;
pub mod sessions;
pub mod solutions;
pub mod sources;
pub mod stats;
pub mod tokens;
pub mod webhooks;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use std::time::Duration;

pub use sqlx::PgPool;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Domain(#[from] antibody_core::error::DomainError),
    #[error("{0}")]
    Other(String),
}

pub type DbResult<T> = Result<T, DbError>;

/// Open a connection pool. `statement_timeout` bounds every query so a slow
/// search cannot hold connections indefinitely.
pub async fn connect(
    url: &str,
    max_connections: u32,
    statement_timeout_ms: u64,
) -> DbResult<PgPool> {
    let opts = PgConnectOptions::from_str(url)?
        .application_name("antibody")
        .options([("statement_timeout", statement_timeout_ms.to_string())]);
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(opts)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> DbResult<()> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(|e| DbError::Other(format!("migration failed: {e}")))
}

/// Parse a string column into a domain enum, mapping failures to DbError.
pub(crate) fn parse_enum<T: FromStr<Err = antibody_core::error::DomainError>>(
    s: &str,
) -> DbResult<T> {
    Ok(T::from_str(s)?)
}

pub(crate) fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}
