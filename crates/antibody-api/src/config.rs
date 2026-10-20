use crate::net::Cidr;
use std::net::SocketAddr;
use std::time::Duration;

/// API server configuration (environment variables, see `deploy/env.example`).
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    /// Externally visible base URL, e.g. `https://antibody.example.com`.
    pub public_url: String,
    /// Master key for deriving webhook secrets. Never logged.
    pub secret_key: Vec<u8>,
    pub cookie_secure: bool,
    pub session_ttl: Duration,
    pub max_session_bytes: usize,
    pub max_webhook_bytes: usize,
    pub max_json_bytes: usize,
    pub rate_limit_per_minute: u32,
    pub login_rate_limit_per_minute: u32,
    pub trust_proxy: bool,
    /// Networks allowed to use plain HTTP. Everything else needs HTTPS
    /// (via a TLS-terminating reverse proxy that sets X-Forwarded-Proto).
    pub http_trusted_networks: Vec<Cidr>,
    pub mcp_allowed_hosts: Vec<String>,
    pub search_timeout: Duration,
    pub embed_timeout: Duration,
    pub db_max_connections: u32,
    pub run_migrations: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("configuration error: {0}")]
pub struct ConfigError(pub String);

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse<T: std::str::FromStr>(name: &str, default: T) -> Result<T, ConfigError> {
    match var(name) {
        None => Ok(default),
        Some(v) => v
            .parse()
            .map_err(|_| ConfigError(format!("{name} has an invalid value"))),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url =
            var("DATABASE_URL").ok_or_else(|| ConfigError("DATABASE_URL is required".into()))?;
        let public_url =
            var("ANTIBODY_PUBLIC_URL").unwrap_or_else(|| "http://localhost:8080".into());
        let parsed = url::Url::parse(&public_url)
            .map_err(|_| ConfigError("ANTIBODY_PUBLIC_URL must be a URL".into()))?;
        let secret = var("ANTIBODY_SECRET_KEY").ok_or_else(|| {
            ConfigError("ANTIBODY_SECRET_KEY is required (32+ random characters)".into())
        })?;
        if secret.len() < 32 {
            return Err(ConfigError(
                "ANTIBODY_SECRET_KEY must be at least 32 characters".into(),
            ));
        }
        let mut hosts = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ];
        if let Some(h) = parsed.host_str() {
            hosts.push(h.to_string());
            if let Some(p) = parsed.port() {
                hosts.push(format!("{h}:{p}"));
            }
        }
        if let Some(extra) = var("ANTIBODY_ALLOWED_HOSTS") {
            hosts.extend(
                extra
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );
        }
        let networks = var("ANTIBODY_HTTP_TRUSTED_NETWORKS")
            .unwrap_or_else(|| "127.0.0.0/8,::1/128".into())
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<Cidr>().map_err(|_| {
                    ConfigError(format!(
                        "invalid network {s} in ANTIBODY_HTTP_TRUSTED_NETWORKS"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            bind: parse(
                "ANTIBODY_BIND",
                "0.0.0.0:8080".parse().expect("valid default"),
            )?,
            database_url,
            cookie_secure: parse("ANTIBODY_COOKIE_SECURE", parsed.scheme() == "https")?,
            public_url: public_url.trim_end_matches('/').to_string(),
            secret_key: secret.into_bytes(),
            session_ttl: Duration::from_secs(parse("ANTIBODY_SESSION_TTL_HOURS", 12u64)? * 3600),
            max_session_bytes: parse("ANTIBODY_MAX_SESSION_BYTES", 2 * 1024 * 1024)?,
            max_webhook_bytes: parse("ANTIBODY_MAX_WEBHOOK_BYTES", 5 * 1024 * 1024)?,
            max_json_bytes: parse("ANTIBODY_MAX_JSON_BYTES", 256 * 1024)?,
            rate_limit_per_minute: parse("ANTIBODY_RATE_LIMIT_PER_MINUTE", 600)?,
            login_rate_limit_per_minute: parse("ANTIBODY_LOGIN_RATE_LIMIT_PER_MINUTE", 10)?,
            trust_proxy: parse("ANTIBODY_TRUST_PROXY", false)?,
            http_trusted_networks: networks,
            mcp_allowed_hosts: hosts,
            search_timeout: Duration::from_millis(parse("ANTIBODY_SEARCH_TIMEOUT_MS", 1200)?),
            embed_timeout: Duration::from_millis(parse("ANTIBODY_EMBED_TIMEOUT_MS", 400)?),
            db_max_connections: parse("ANTIBODY_DB_MAX_CONNECTIONS", 20)?,
            run_migrations: parse("ANTIBODY_MIGRATE", true)?,
        })
    }

    /// Settings for tests and local tooling.
    pub fn for_tests(database_url: &str) -> Self {
        Self {
            bind: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url.to_string(),
            public_url: "http://localhost:8080".into(),
            secret_key: b"test-secret-key-test-secret-key-0123456789".to_vec(),
            cookie_secure: false,
            session_ttl: Duration::from_secs(3600),
            max_session_bytes: 512 * 1024,
            max_webhook_bytes: 1024 * 1024,
            max_json_bytes: 64 * 1024,
            rate_limit_per_minute: 100_000,
            login_rate_limit_per_minute: 1000,
            trust_proxy: false,
            http_trusted_networks: vec!["127.0.0.0/8".parse().unwrap(), "::1/128".parse().unwrap()],
            mcp_allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            search_timeout: Duration::from_millis(2000),
            embed_timeout: Duration::from_millis(500),
            db_max_connections: 10,
            run_migrations: true,
        }
    }

    pub fn public_origin(&self) -> String {
        url::Url::parse(&self.public_url)
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|_| self.public_url.clone())
    }

    pub fn cookie_name(&self) -> &'static str {
        // The __Host- prefix forces Secure, Path=/ and no Domain attribute.
        if self.cookie_secure {
            "__Host-antibody_session"
        } else {
            "antibody_session"
        }
    }
}
