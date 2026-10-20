use antibody_search::{SearchConfig, SearchEngine};
use antibody_worker::{WorkerConfig, WorkerContext};
use clap::Parser;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Antibody background worker: extraction, correlation, embeddings, retention.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[arg(long, env = "ANTIBODY_WORKER_CONCURRENCY", default_value_t = 4)]
    concurrency: usize,
    #[arg(long, env = "ANTIBODY_WORKER_LEASE_SECS", default_value_t = 120)]
    lease_secs: u64,
    #[arg(long, env = "ANTIBODY_PUBLIC_URL")]
    public_url: Option<String>,
    /// Run migrations before starting (the API also does this).
    #[arg(long, env = "ANTIBODY_MIGRATE", default_value_t = false)]
    migrate: bool,
    /// Process all currently due jobs and exit.
    #[arg(long, default_value_t = false)]
    once: bool,
    #[arg(long, env = "ANTIBODY_LOG_FORMAT", default_value = "json")]
    log_format: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,sqlx=warn".into());
    if args.log_format == "json" {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let pool = antibody_db::connect(
        &args.database_url,
        (args.concurrency as u32 + 4).max(8),
        60_000,
    )
    .await?;
    if args.migrate {
        antibody_db::migrate(&pool).await?;
    }
    let providers = antibody_providers::config::from_env()?;
    tracing::info!(providers = %providers.describe(), "providers configured");
    let search = SearchEngine::new(
        pool.clone(),
        providers.clone(),
        SearchConfig {
            public_url: args.public_url.clone(),
            ..Default::default()
        },
    );
    let ctx = WorkerContext {
        pool,
        providers,
        search,
    };
    let cfg = WorkerConfig {
        concurrency: args.concurrency.clamp(1, 64),
        lease: Duration::from_secs(args.lease_secs.clamp(10, 3600)),
        ..Default::default()
    };

    if args.once {
        let n = antibody_worker::drain(&ctx, &cfg).await?;
        tracing::info!(jobs = n, "drained");
        return Ok(());
    }

    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("signal handler");
            tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
        }
        #[cfg(not(unix))]
        let _ = ctrl_c.await;
        signal.cancel();
    });
    antibody_worker::run(ctx, cfg, shutdown).await
}
