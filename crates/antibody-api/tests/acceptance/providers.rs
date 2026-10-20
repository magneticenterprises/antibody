//! A14: model provider absence, failure and approval.

use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// A14: a missing or failing model provider does not break ingestion

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a14_no_provider_mode_ingests_and_retrieves(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let (_, sols) = app
        .ingest_and_extract(
            &app.admin_token,
            "key-noprov-01",
            &session_body("acme/api", None, None),
        )
        .await;
    assert_eq!(sols.len(), 1);
    let d = app.solution(sols[0]).await;
    assert_eq!(d["extractor"], "heuristic-v1");
    // Full-text only (no error text): still finds it, flagged as not semantic.
    let r = app.search(&app.admin_token, &json!({ "task": "sqlx nullable columns inferred as Option after upgrade", "repository": "acme/api" })).await;
    assert_eq!(
        r["suggestions"][0]["solution_id"],
        sols[0].to_string(),
        "{r}"
    );
    assert_eq!(r["degraded"], false);
    assert!(
        r["degraded_reasons"][0]
            .as_str()
            .unwrap()
            .contains("no embedding provider")
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a14_failing_providers_degrade_gracefully(pool: sqlx::PgPool) {
    use antibody_providers::DataLocation;
    use antibody_providers::openai::{OpenAiChat, OpenAiCompatibleConfig, OpenAiEmbedding};
    let dead = |model: &str| OpenAiCompatibleConfig {
        base_url: "http://127.0.0.1:9/v1".into(),
        model: model.into(),
        api_key: None,
        location: DataLocation::Local,
        timeout: std::time::Duration::from_millis(300),
        json_mode: true,
        openai_reasoning: false,
    };
    let providers = antibody_providers::Providers {
        embedding: Some(std::sync::Arc::new(OpenAiEmbedding::new(dead("e"), None))),
        llm: Some(std::sync::Arc::new(OpenAiChat::new(dead("l")))),
    };
    let app = TestApp::spawn_with(pool, providers, |_| {}).await;
    app.create_repo("acme/api").await;
    let v = TestApp::json(
        app.ingest(
            &app.admin_token,
            "key-fail-0001",
            &session_body("acme/api", None, None),
        )
        .await,
        StatusCode::ACCEPTED,
    )
    .await;
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // Ingestion succeeded even though the model is down; extraction retries.
    app.drain().await;
    let st = TestApp::json(
        app.get(&format!("/v1/sessions/{id}"), &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(st["ingestion_status"], "accepted");
    assert_eq!(st["extraction_status"], "pending");
    let (state, future): (String, bool) =
        sqlx::query_as("SELECT state, run_at > now() FROM jobs WHERE session_id = $1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!((state.as_str(), future), ("queued", true));

    // On the final attempt the heuristic extractor is used instead.
    sqlx::query(
        "UPDATE jobs SET run_at = now(), attempts = max_attempts - 2 WHERE session_id = $1",
    )
    .bind(id)
    .execute(&app.pool)
    .await
    .unwrap();
    app.drain().await;
    let st = TestApp::json(
        app.get(&format!("/v1/sessions/{id}"), &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(st["extraction_status"], "succeeded");
    assert_eq!(st["extractor"], "heuristic-v1 (llm fallback)");

    // Search still answers from exact/full-text and reports the degradation.
    let r = app.search(&app.admin_token, &json!({ "task": "x", "error": "error[E0308]: mismatched types", "repository": "acme/api" })).await;
    assert_eq!(r["suggestions"].as_array().unwrap().len(), 1);
    assert_eq!(r["degraded"], true);
    assert!(
        r["degraded_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x.as_str().unwrap().contains("embedding provider"))
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a14_external_providers_need_admin_approval(pool: sqlx::PgPool) {
    use antibody_providers::DataLocation;
    use antibody_providers::openai::{OpenAiChat, OpenAiCompatibleConfig};
    // An "external" LLM that would fail loudly if called.
    let providers = antibody_providers::Providers {
        embedding: None,
        llm: Some(std::sync::Arc::new(OpenAiChat::new(
            OpenAiCompatibleConfig {
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "external".into(),
                api_key: None,
                location: DataLocation::External,
                timeout: std::time::Duration::from_millis(200),
                json_mode: true,
                openai_reasoning: false,
            },
        ))),
    };
    let app = TestApp::spawn_with(pool, providers, |_| {}).await;
    app.create_repo("acme/api").await;
    let (id, _) = app
        .ingest_and_extract(
            &app.admin_token,
            "key-ext-00001",
            &session_body("acme/api", None, None),
        )
        .await;
    let st = TestApp::json(
        app.get(&format!("/v1/sessions/{id}"), &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        st["extractor"], "heuristic-v1",
        "unapproved external provider is never called"
    );
}
