use crate::config::Config;
use crate::net::RateLimiters;
use antibody_db::PgPool;
use antibody_providers::Providers;
use antibody_search::{SearchConfig, SearchEngine};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub providers: Providers,
    pub search: SearchEngine,
    pub limiters: Arc<RateLimiters>,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config, providers: Providers) -> Self {
        let search = SearchEngine::new(
            pool.clone(),
            providers.clone(),
            SearchConfig {
                query_timeout: config.search_timeout,
                embed_timeout: config.embed_timeout,
                public_url: Some(config.public_url.clone()),
                ..Default::default()
            },
        );
        let limiters = Arc::new(RateLimiters::new(
            config.rate_limit_per_minute,
            config.login_rate_limit_per_minute,
        ));
        Self {
            pool,
            config: Arc::new(config),
            providers,
            search,
            limiters,
        }
    }

    /// Webhook secret for an organization, derived from the master key.
    pub fn github_webhook_secret(&self, org_id: uuid::Uuid, version: i32) -> String {
        antibody_integrations::signing::derive_secret(
            &self.config.secret_key,
            &format!("github-webhook:{org_id}:v{version}"),
        )
    }

    pub fn ci_webhook_secret(&self, org_id: uuid::Uuid, version: i32) -> String {
        antibody_integrations::signing::derive_secret(
            &self.config.secret_key,
            &format!("ci-webhook:{org_id}:v{version}"),
        )
    }
}
