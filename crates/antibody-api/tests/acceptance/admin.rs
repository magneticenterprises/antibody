//! Workflow improvements, health routes and admin/dashboard contract regressions.

use super::verified_fix;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Workflow improvements: lifecycle gates

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn workflow_promotion_requires_evaluation_and_gates(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let wid = Uuid::now_v7();
    sqlx::query("INSERT INTO workflow_improvements (id, organization_id, target_scope, title, proposed_change, fingerprint) VALUES ($1, $2, 'repository', 'Run migrations before tests', 'Add `make db-migrate` to the test setup step.', 'fp1')")
        .bind(wid).bind(app.org_id).execute(&app.pool).await.unwrap();
    let path = format!("/v1/admin/workflow-improvements/{wid}/transition");
    let t = |body: serde_json::Value| {
        app.http
            .post(app.url(&path))
            .bearer_auth(&app.admin_token)
            .json(&body)
            .send()
    };
    assert_eq!(
        t(json!({ "to": "promoted" })).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        t(json!({ "to": "evaluated" })).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        t(json!({ "to": "evaluated", "evaluation": { "runs": 5, "passed": 5 } }))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        t(json!({ "to": "available" })).await.unwrap().status(),
        StatusCode::OK
    );
    let r = TestApp::json(
        t(json!({ "to": "promoted" })).await.unwrap(),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("negative cases")
    );
    let ok = TestApp::json(t(json!({ "to": "promoted", "evaluation_reproducible": true, "has_negative_cases": true, "preserves_checks": true })).await.unwrap(), StatusCode::OK).await;
    assert_eq!(ok["state"], "promoted");
    assert_eq!(ok["approved_by"], app.admin_principal.to_string());
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn health_and_unknown_routes(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let ready = TestApp::json(
        app.http
            .get(app.url("/v1/health/ready"))
            .send()
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(ready["status"], "ready");
    let live = app
        .http
        .get(app.url("/v1/health/live"))
        .send()
        .await
        .unwrap();
    assert!(live.headers().get("x-request-id").is_some());
    let nf = TestApp::json(
        app.http.get(app.url("/v1/nope")).send().await.unwrap(),
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(nf["error"]["code"], "not_found");
    assert!(nf["error"]["request_id"].is_string());
}

// ---------------------------------------------------------------------------
// Admin/dashboard contract regressions

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn admin_contract_tokens_grants_preview_and_workflow(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let api = app.create_repo("acme/api").await;
    // Create a service principal and a token through the API using the
    // documented scope strings.
    let p = TestApp::json(
        app.post(
            "/v1/admin/principals",
            &app.admin_token,
            &json!({ "kind": "service", "display_name": "ci-bot", "role": "member" }),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    let pid = p["id"].as_str().unwrap().to_string();
    let t = TestApp::json(app.post("/v1/admin/tokens", &app.admin_token, &json!({ "principal_id": pid, "name": "bot", "scopes": ["context:read", "sessions:write"] })).await, StatusCode::CREATED).await;
    let token = t["token"].as_str().unwrap().to_string();
    let r = app
        .post(
            "/v1/admin/tokens",
            &app.admin_token,
            &json!({ "principal_id": pid, "name": "bad", "scopes": ["context_read"] }),
        )
        .await;
    assert_eq!(
        r.status(),
        StatusCode::BAD_REQUEST,
        "only the documented scope strings are accepted"
    );
    let r = app
        .post(
            "/v1/admin/tokens",
            &app.admin_token,
            &json!({ "principal_id": pid, "name": "esc", "scopes": ["admin"] }),
        )
        .await;
    assert_eq!(
        r.status(),
        StatusCode::FORBIDDEN,
        "member principals cannot receive the admin scope"
    );
    let list = TestApp::json(
        app.get("/v1/admin/tokens", &app.admin_token).await,
        StatusCode::OK,
    )
    .await;
    let bot = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "bot")
        .unwrap();
    assert_eq!(bot["scopes"], json!(["context:read", "sessions:write"]));
    assert!(
        list.to_string().find(&token).is_none(),
        "plaintext tokens are never listed"
    );

    // Grants can be listed.
    let r = app
        .http
        .put(app.url("/v1/admin/grants"))
        .bearer_auth(&app.admin_token)
        .json(&json!({ "principal_id": pid, "repository_id": api, "permission": "read" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let grants = TestApp::json(
        app.get("/v1/admin/grants", &app.admin_token).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(grants[0]["repository"], "acme/api");
    assert_eq!(grants[0]["principal_name"], "ci-bot");
    assert_eq!(grants[0]["permission"], "read");
    assert_eq!(
        app.get("/v1/admin/grants", &token).await.status(),
        StatusCode::FORBIDDEN
    );

    // Preview searches are not recorded as retrievals.
    verified_fix(&app).await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM retrievals")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let preview = TestApp::json(
        app.post(
            "/v1/context/search?preview=true",
            &app.admin_token,
            &json!({ "task": "x", "error": "error[E0308]: mismatched types" }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(preview["suggestions"].as_array().unwrap().len(), 1);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM retrievals")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let r = app.post("/v1/outcomes", &app.admin_token, &json!({ "retrieval_id": preview["retrieval_id"], "solution_id": preview["suggestions"][0]["solution_id"], "outcome": "used" })).await;
    assert_eq!(
        r.status(),
        StatusCode::NOT_FOUND,
        "previews cannot receive outcomes"
    );

    // Workflow improvements can be proposed manually and start as candidates.
    let w = TestApp::json(app.post("/v1/admin/workflow-improvements", &app.admin_token, &json!({ "title": "Migrate before tests", "proposed_change": "Run `make db-migrate` before `cargo test` in CI.", "target_scope": "repository", "repository_id": api })).await, StatusCode::CREATED).await;
    assert_eq!(w["state"], "candidate");
    let dup = app.post("/v1/admin/workflow-improvements", &app.admin_token, &json!({ "title": "Again", "proposed_change": "Run `make db-migrate` before `cargo test` in CI.", "target_scope": "repository", "repository_id": api })).await;
    assert_eq!(dup.status(), StatusCode::CONFLICT);
}
