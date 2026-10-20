use crate::state::AppState;
use antibody_core::api::{HealthCheck, HealthStatus};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use std::time::Duration;

fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[utoipa::path(get, path = "/v1/health/live", operation_id = "health_live", tag = "health", responses((status = 200, body = HealthStatus)))]
pub async fn live() -> Json<HealthStatus> {
    Json(HealthStatus {
        status: "ok".into(),
        checks: vec![],
        version: version(),
    })
}

#[utoipa::path(
    get, path = "/v1/health/ready", operation_id = "health_ready", tag = "health",
    responses((status = 200, body = HealthStatus), (status = 503, body = HealthStatus))
)]
pub async fn ready(State(state): State<AppState>) -> (StatusCode, Json<HealthStatus>) {
    let db = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.pool),
    )
    .await;
    let db_ok = matches!(db, Ok(Ok(1)));
    let vector_ok = if db_ok {
        matches!(
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')"
            )
            .fetch_one(&state.pool)
            .await,
            Ok(true)
        )
    } else {
        false
    };
    let migrations_ok = if db_ok {
        let applied: Result<i64, _> =
            sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
                .fetch_one(&state.pool)
                .await;
        applied
            .map(|n| n as usize >= antibody_db::MIGRATOR.iter().count())
            .unwrap_or(false)
    } else {
        false
    };
    let checks = vec![
        HealthCheck {
            name: "database".into(),
            ok: db_ok,
            detail: None,
        },
        HealthCheck {
            name: "pgvector".into(),
            ok: vector_ok,
            detail: None,
        },
        HealthCheck {
            name: "migrations".into(),
            ok: migrations_ok,
            detail: None,
        },
        HealthCheck {
            name: "embedding_provider".into(),
            ok: true,
            detail: Some(if state.providers.embedding.is_some() {
                "configured".into()
            } else {
                "not configured (exact and full-text search only)".into()
            }),
        },
    ];
    let ok = db_ok && vector_ok && migrations_ok;
    (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(HealthStatus {
            status: if ok {
                "ready".into()
            } else {
                "unavailable".into()
            },
            checks,
            version: version(),
        }),
    )
}
