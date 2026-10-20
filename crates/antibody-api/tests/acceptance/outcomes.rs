//! A15: outcome tracking.

use super::verified_fix;
use crate::common::*;
use antibody_core::model::{Permission, Scope};
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// A15: outcome tracking never fabricates savings

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a15_metrics_count_only_reported_and_confirmed_outcomes(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let api = app.create_repo("acme/api").await;
    let id = verified_fix(&app).await;
    let (_, b) = app
        .member(
            "b",
            &[(api, Permission::Write)],
            &[Scope::ContextRead, Scope::OutcomesWrite],
        )
        .await;
    let (_, c) = app
        .member(
            "c",
            &[(api, Permission::Write)],
            &[Scope::ContextRead, Scope::OutcomesWrite],
        )
        .await;

    let s = app.search(&b, &json!({ "task": "x", "error": "error[E0308]: mismatched types", "repository": "acme/api" })).await;
    let rid = s["retrieval_id"].as_str().unwrap().to_string();
    let ov = TestApp::json(
        app.get("/v1/overview", &app.admin_token).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        (
            ov["stats"]["outcomes_used"].as_i64(),
            ov["stats"]["confirmed_reuse"].as_i64()
        ),
        (Some(0), Some(0)),
        "search hits are not reuse"
    );
    assert!(ov["stats"].get("estimated_savings").is_none());

    // Outcomes must reference a suggestion actually shown to the same caller.
    let other: Uuid = Uuid::now_v7();
    let r = app
        .post(
            "/v1/outcomes",
            &b,
            &json!({ "retrieval_id": rid, "solution_id": other, "outcome": "used" }),
        )
        .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = app
        .post(
            "/v1/outcomes",
            &c,
            &json!({ "retrieval_id": rid, "solution_id": id, "outcome": "used" }),
        )
        .await;
    assert_eq!(
        r.status(),
        StatusCode::NOT_FOUND,
        "cannot report on someone else's retrieval"
    );
    let r = app
        .post(
            "/v1/outcomes",
            &b,
            &json!({ "solution_id": id, "outcome": "used" }),
        )
        .await;
    assert_eq!(
        r.status(),
        StatusCode::BAD_REQUEST,
        "retrieval_id is required"
    );

    let fix_rev = "c0ffee0000000000000000000000000000000000";
    let body = json!({ "retrieval_id": rid, "solution_id": id, "outcome": "used", "check": { "name": "cargo test", "revision": &fix_rev[..12], "result": "pass" } });
    let first = TestApp::json(app.post("/v1/outcomes", &b, &body).await, StatusCode::OK).await;
    assert_eq!(first["verification"], "claimed");
    let dup = TestApp::json(app.post("/v1/outcomes", &b, &body).await, StatusCode::OK).await;
    assert_eq!(dup["duplicate"], true);
    let ov = TestApp::json(
        app.get("/v1/overview", &app.admin_token).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        (
            ov["stats"]["outcomes_used"].as_i64(),
            ov["stats"]["confirmed_reuse"].as_i64()
        ),
        (Some(1), Some(0)),
        "claims are not confirmed reuse"
    );

    // A signed CI success for the claimed check and revision confirms reuse.
    app.github_event(
        "check_run",
        "conf-1",
        &check_run("acme/api", 500, "cargo test", "success", fix_rev, "ok"),
    )
    .await;
    app.drain().await;
    let ov = TestApp::json(
        app.get("/v1/overview", &app.admin_token).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(ov["stats"]["confirmed_reuse"], 1);
    let tokens: i64 = ov["stats"]["retrieval_context_tokens_7d"].as_i64().unwrap();
    assert!(tokens > 0, "retrieval overhead is reported");
}
