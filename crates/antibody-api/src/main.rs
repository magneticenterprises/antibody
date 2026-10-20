use antibody_api::{AppState, Config, bootstrap, openapi, router};
use antibody_core::model::{PrincipalKind, Role, Scope};
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::str::FromStr;

/// Antibody API and MCP server.
#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(
        long,
        env = "ANTIBODY_LOG_FORMAT",
        default_value = "json",
        global = true
    )]
    log_format: String,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the HTTP API and MCP server (default).
    Serve,
    /// Apply database migrations and exit.
    Migrate,
    /// Create the first organization, administrator and admin API token.
    Bootstrap {
        #[arg(long)]
        org_slug: String,
        #[arg(long)]
        org_name: String,
        #[arg(long, default_value = "admin")]
        admin_username: String,
        /// Read the admin password from this environment variable instead of generating one.
        #[arg(long)]
        admin_password_env: Option<String>,
    },
    /// Create a service principal and an API token for it.
    CreateToken {
        #[arg(long)]
        org_slug: String,
        #[arg(long)]
        name: String,
        /// Comma-separated scopes: sessions:write,context:read,outcomes:write,ci:write,admin
        #[arg(long, default_value = "sessions:write,context:read,outcomes:write")]
        scopes: String,
        #[arg(long, default_value = "admin")]
        role: String,
    },
    /// Print the OpenAPI document.
    Openapi,
    /// Print the MCP tool definitions (names, descriptions, JSON schemas).
    McpSchema,
}

fn init_tracing(format: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,sqlx=warn,tower_http=info".into());
    if format == "json" {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Openapi => {
            println!("{}", openapi::json());
            Ok(())
        }
        Command::McpSchema => {
            println!("{}", openapi::mcp_tools_json());
            Ok(())
        }
        Command::Migrate => {
            init_tracing(&cli.log_format);
            let cfg = Config::from_env()?;
            let pool = antibody_db::connect(&cfg.database_url, 2, 600_000).await?;
            antibody_db::migrate(&pool).await?;
            tracing::info!("migrations applied");
            Ok(())
        }
        Command::Bootstrap {
            org_slug,
            org_name,
            admin_username,
            admin_password_env,
        } => {
            let url = std::env::var("DATABASE_URL")?;
            let pool = antibody_db::connect(&url, 2, 60_000).await?;
            antibody_db::migrate(&pool).await?;
            let password = match admin_password_env {
                Some(var) => {
                    Some(std::env::var(&var).map_err(|_| anyhow::anyhow!("{var} is not set"))?)
                }
                None => None,
            };
            let out = bootstrap::bootstrap(&pool, &org_slug, &org_name, &admin_username, password)
                .await?;
            println!("{}", serde_json::to_string_pretty(&out)?);
            eprintln!("Store the admin token and password now; they are not shown again.");
            Ok(())
        }
        Command::CreateToken {
            org_slug,
            name,
            scopes,
            role,
        } => {
            let url = std::env::var("DATABASE_URL")?;
            let pool = antibody_db::connect(&url, 2, 60_000).await?;
            let org = antibody_db::orgs::get_by_slug(&pool, &org_slug)
                .await?
                .ok_or_else(|| anyhow::anyhow!("unknown organization"))?;
            let scopes: Vec<Scope> = scopes
                .split(',')
                .map(|s| Scope::from_str(s.trim()))
                .collect::<Result<_, _>>()?;
            let role = Role::from_str(&role)?;
            let p = antibody_db::access::create_principal(
                &pool,
                org.id,
                PrincipalKind::Service,
                role,
                &name,
            )
            .await?;
            let t = antibody_db::tokens::issue_token(
                &pool,
                antibody_db::tokens::NewToken {
                    org_id: org.id,
                    principal_id: p.id,
                    name: &name,
                    scopes: &scopes,
                    repository_ids: None,
                    expires_at: None,
                    created_by: None,
                },
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({ "principal_id": p.id, "token": t.token })
                )?
            );
            Ok(())
        }
        Command::Serve => serve(&cli.log_format).await,
    }
}

async fn serve(log_format: &str) -> anyhow::Result<()> {
    init_tracing(log_format);
    let cfg = Config::from_env()?;
    let pool = antibody_db::connect(&cfg.database_url, cfg.db_max_connections, 30_000).await?;
    if cfg.run_migrations {
        antibody_db::migrate(&pool).await?;
    }
    auto_bootstrap(&pool).await?;
    let providers = antibody_providers::config::from_env()?;
    tracing::info!(providers = %providers.describe(), public_url = %cfg.public_url, "starting antibody-api");
    let bind = cfg.bind;
    let state = AppState::new(pool, cfg, providers);
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// Local/example convenience: bootstrap from env when the database is empty.
async fn auto_bootstrap(pool: &antibody_db::PgPool) -> anyhow::Result<()> {
    let (Ok(slug), Ok(password)) = (
        std::env::var("ANTIBODY_BOOTSTRAP_ORG_SLUG"),
        std::env::var("ANTIBODY_BOOTSTRAP_ADMIN_PASSWORD"),
    ) else {
        return Ok(());
    };
    if !antibody_db::orgs::list(pool).await?.is_empty() {
        return Ok(());
    }
    let name = std::env::var("ANTIBODY_BOOTSTRAP_ORG_NAME").unwrap_or_else(|_| slug.clone());
    let user =
        std::env::var("ANTIBODY_BOOTSTRAP_ADMIN_USERNAME").unwrap_or_else(|_| "admin".into());
    bootstrap::bootstrap(pool, &slug, &name, &user, Some(password)).await?;
    tracing::warn!(org = %slug, admin = %user, "bootstrapped from environment; create API tokens from the dashboard (the bootstrap token is not shown)");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler");
        tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}
