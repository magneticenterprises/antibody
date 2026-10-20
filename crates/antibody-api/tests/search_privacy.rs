#[path = "common/mod.rs"]
mod common;

use antibody_core::model::{Permission, Scope};
use antibody_providers::openai::{OpenAiCompatibleConfig, OpenAiEmbedding};
use antibody_providers::{DataLocation, Providers};
use axum::{Json, Router, routing::post};
use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn shared_search_redacts_before_embedding_and_hides_provider_error_text(pool: sqlx::PgPool) {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = captured.clone();
    let mock = Router::new().route(
        "/embeddings",
        post(move |Json(body): Json<Value>| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push(body);
                (
                    StatusCode::BAD_REQUEST,
                    "synthetic private provider diagnostics",
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let providers = Providers {
        embedding: Some(Arc::new(OpenAiEmbedding::new(
            OpenAiCompatibleConfig {
                base_url: format!("http://{address}"),
                api_key: None,
                model: "recording-test-fixture".into(),
                location: DataLocation::Local,
                timeout: Duration::from_secs(1),
                json_mode: true,
                openai_reasoning: false,
            },
            Some(3),
        ))),
        llm: None,
    };
    let app = TestApp::spawn_with(pool, providers, |_| {}).await;
    app.create_repo("acme/api").await;
    let result = app
        .search(
            &app.admin_token,
            &json!({
                "task": "failed request {\"api_key\":\"synthetic query credential\"}",
                "error": "error: DB_PASSWORD='synthetic error credential'",
                "repository": "acme/api"
            }),
        )
        .await;
    assert_eq!(result["degraded"], true);
    assert!(
        !result
            .to_string()
            .contains("synthetic private provider diagnostics")
    );
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let input = requests[0]["input"][0].as_str().unwrap();
    assert!(input.contains("[REDACTED:secret_assignment]"), "{input}");
    assert!(!input.contains("synthetic query credential"), "{input}");
    assert!(!input.contains("synthetic error credential"), "{input}");
    server.abort();
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn activity_and_overview_hide_unscoped_and_inaccessible_source_retrievals(
    pool: sqlx::PgPool,
) {
    let app = TestApp::spawn(pool).await;
    let consumer = app.create_repo("acme/consumer").await;
    let source = app.create_repo("acme/source").await;
    app.ingest_and_extract(
        &app.admin_token,
        "privacy-fixture",
        &session_body("acme/source", None, None),
    )
    .await;
    let (_, outsider) = app.member("outsider", &[], &[Scope::ContextRead]).await;
    let (_, consumer_only) = app
        .member(
            "consumer-only",
            &[(consumer, Permission::Read)],
            &[Scope::ContextRead],
        )
        .await;
    let (_, both) = app
        .member(
            "both",
            &[(consumer, Permission::Read), (source, Permission::Read)],
            &[Scope::ContextRead],
        )
        .await;
    let unscoped = app
        .search(
            &app.admin_token,
            &json!({"task": "error[E0308]: mismatched types"}),
        )
        .await;
    assert_eq!(unscoped["suggestions"].as_array().unwrap().len(), 1);
    let scoped = app
        .search(
            &app.admin_token,
            &json!({"task": "error[E0308]: mismatched types", "repository": "acme/consumer"}),
        )
        .await;
    assert_eq!(scoped["suggestions"].as_array().unwrap().len(), 1);
    let narrow = antibody_db::tokens::issue_token(
        &app.pool,
        antibody_db::tokens::NewToken {
            org_id: app.org_id,
            principal_id: app.admin_principal,
            name: "narrow-admin",
            scopes: &[Scope::ContextRead, Scope::Admin],
            repository_ids: Some(&[consumer]),
            expires_at: None,
            created_by: None,
        },
    )
    .await
    .unwrap();

    for token in [&outsider, &consumer_only, &narrow.token] {
        let activity = TestApp::json(
            app.get("/v1/activity/retrievals", token).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(activity, json!([]));
        let overview = TestApp::json(app.get("/v1/overview", token).await, StatusCode::OK).await;
        assert_eq!(overview["stats"]["retrievals_7d"], 0);
        assert_eq!(overview["stats"]["retrievals_with_results_7d"], 0);
        assert_eq!(overview["stats"]["retrieval_context_tokens_7d"], 0);
        assert!(overview["stats"]["retrieval_p95_latency_ms_7d"].is_null());
    }
    for (token, expected) in [(&both, 1), (&app.admin_token, 2)] {
        let activity = TestApp::json(
            app.get("/v1/activity/retrievals", token).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(activity.as_array().unwrap().len(), expected);
        let overview = TestApp::json(app.get("/v1/overview", token).await, StatusCode::OK).await;
        assert_eq!(overview["stats"]["retrievals_7d"], expected);
    }
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn same_repo_component_mismatch_is_suppressed_unless_requested(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/monorepo").await;
    let (_, solutions) = app
        .ingest_and_extract(
            &app.admin_token,
            "component-fixture",
            &session_body("acme/monorepo", None, None),
        )
        .await;
    sqlx::query("UPDATE solutions SET applicability = $2 WHERE id = $1")
        .bind(solutions[0])
        .bind(json!({"components": ["services/api/**"]}))
        .execute(&app.pool)
        .await
        .unwrap();
    let mut query = json!({
        "task": "error[E0308]: mismatched types",
        "repository": "acme/monorepo",
        "component_path": "services/api2",
    });
    let hidden = app.search(&app.admin_token, &query).await;
    assert_eq!(hidden["suggestions"], json!([]));
    assert_eq!(hidden["suppressed_incompatible"], 1);
    query["include_incompatible"] = json!(true);
    let visible = app.search(&app.admin_token, &query).await;
    assert_eq!(
        visible["suggestions"][0]["applicability"]["status"],
        "incompatible"
    );
}
