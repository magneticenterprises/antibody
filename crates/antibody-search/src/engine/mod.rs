use antibody_core::error::{DomainError, ErrorCode};
use antibody_providers::Providers;
use sqlx::PgPool;
use std::time::Duration;

mod detail;
mod search;
mod stages;

#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Budget for the indexed (database) stages.
    pub query_timeout: Duration,
    /// Budget for embedding the query; on timeout the vector stage is skipped.
    pub embed_timeout: Duration,
    pub candidate_limit: i64,
    /// Public dashboard URL used to build detail links.
    pub public_url: Option<String>,
    /// Minimum cosine similarity for a vector-only candidate.
    pub min_vector_similarity: f32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            query_timeout: Duration::from_millis(1200),
            embed_timeout: Duration::from_millis(400),
            candidate_limit: 40,
            public_url: None,
            min_vector_similarity: 0.45,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("database error: {0}")]
    Db(#[from] antibody_db::DbError),
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

impl SearchError {
    pub fn code(&self) -> ErrorCode {
        match self {
            SearchError::Domain(d) => d.code,
            SearchError::Db(antibody_db::DbError::Domain(d)) => d.code,
            _ => ErrorCode::Internal,
        }
    }
}

#[derive(Clone)]
pub struct SearchEngine {
    pool: PgPool,
    providers: Providers,
    cfg: SearchConfig,
}

#[derive(Debug, Default, Clone)]
struct Signals {
    signature_match: bool,
    symbol_overlap: i32,
    path_overlap: i32,
    fts_rank: f32,
    vector_similarity: f32,
}

const MAX_TASK_CHARS: usize = 8_000;
const MAX_ERROR_CHARS: usize = 20_000;

impl SearchEngine {
    pub fn new(pool: PgPool, providers: Providers, cfg: SearchConfig) -> Self {
        Self {
            pool,
            providers,
            cfg,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}
