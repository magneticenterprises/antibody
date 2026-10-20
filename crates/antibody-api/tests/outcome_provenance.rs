#[path = "common/mod.rs"]
mod common;

use antibody_core::model::{Permission, Scope};
use common::*;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

const REVISION: &str = "1234567890abcdef1234567890abcdef12345678";

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn cross_repository_outcome_requires_ci_in_consumer_repository(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let source = app.create_repo("acme/source").await;
    let consumer = app.create_repo("acme/consumer").await;
    let (_, solutions) = app
        .ingest_and_extract(
            &app.admin_token,
            "outcome-fixture",
            &session_body("acme/source", None, None),
        )
        .await;
    let (_, token) = app
        .member(
            "consumer-agent",
            &[(source, Permission::Read), (consumer, Permission::Write)],
            &[Scope::ContextRead, Scope::OutcomesWrite],
        )
        .await;
    let found = app
        .search(
            &token,
            &json!({
                "task": "error[E0308]: mismatched types", "repository": "acme/consumer",
            }),
        )
        .await;
    let reported = TestApp::json(
        app.post(
            "/v1/outcomes",
            &token,
            &json!({
                "retrieval_id": found["retrieval_id"],
                "solution_id": solutions[0],
                "outcome": "used",
                "check": { "name": "cargo test", "revision": REVISION, "result": "pass" },
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let outcome: Uuid = reported["id"].as_str().unwrap().parse().unwrap();
    let repository: Option<Uuid> =
        sqlx::query_scalar("SELECT repository_id FROM outcomes WHERE id = $1")
            .bind(outcome)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(repository, Some(consumer));

    for (repo, check, expected, delivery) in [
        ("acme/source", "cargo test", "claimed", "source-ci"),
        (
            "acme/consumer",
            "unrelated check",
            "claimed",
            "wrong-check-ci",
        ),
        ("acme/consumer", "cargo test", "ci_verified", "consumer-ci"),
    ] {
        assert_eq!(
            app.github_event(
                "check_run",
                delivery,
                &check_run(
                    repo,
                    1,
                    check,
                    "success",
                    REVISION,
                    "synthetic successful check"
                ),
            )
            .await
            .status(),
            StatusCode::ACCEPTED
        );
        app.drain().await;
        let verification: String =
            sqlx::query_scalar("SELECT verification FROM outcomes WHERE id = $1")
                .bind(outcome)
                .fetch_one(&app.pool)
                .await
                .unwrap();
        assert_eq!(verification, expected, "{repo}: {check}");
    }
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn no_consumer_repository_allows_feedback_but_cannot_claim_ci_verification(
    pool: sqlx::PgPool,
) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/source").await;
    let (_, solutions) = app
        .ingest_and_extract(
            &app.admin_token,
            "unscoped-outcome-fixture",
            &session_body("acme/source", None, None),
        )
        .await;
    let found = app
        .search(
            &app.admin_token,
            &json!({"task": "error[E0308]: mismatched types"}),
        )
        .await;
    let mut report = json!({
        "retrieval_id": found["retrieval_id"],
        "solution_id": solutions[0],
        "outcome": "used",
        "check": { "name": "cargo test", "revision": REVISION, "result": "pass" },
    });
    assert_eq!(
        app.post("/v1/outcomes", &app.admin_token, &report)
            .await
            .status(),
        StatusCode::BAD_REQUEST,
    );
    report.as_object_mut().unwrap().remove("check");
    let recorded = TestApp::json(
        app.post("/v1/outcomes", &app.admin_token, &report).await,
        StatusCode::OK,
    )
    .await;
    let id: Uuid = recorded["id"].as_str().unwrap().parse().unwrap();
    let repository: Option<Uuid> =
        sqlx::query_scalar("SELECT repository_id FROM outcomes WHERE id = $1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(repository, None);
    assert_eq!(recorded["verification"], "none");
    let overview = TestApp::json(
        app.get("/v1/overview", &app.admin_token).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(overview["stats"]["outcomes_used"], 1);
    assert_eq!(overview["stats"]["confirmed_reuse"], 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn losing_consumer_access_prevents_reporting_against_old_retrieval(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let source = app.create_repo("acme/source").await;
    let consumer = app.create_repo("acme/consumer").await;
    let (_, solutions) = app
        .ingest_and_extract(
            &app.admin_token,
            "revoked-consumer-fixture",
            &session_body("acme/source", None, None),
        )
        .await;
    let (principal, token) = app
        .member(
            "consumer-agent",
            &[(source, Permission::Read), (consumer, Permission::Read)],
            &[Scope::ContextRead, Scope::OutcomesWrite],
        )
        .await;
    let found = app
        .search(
            &token,
            &json!({
                "task": "error[E0308]: mismatched types", "repository": "acme/consumer",
            }),
        )
        .await;
    antibody_db::access::revoke_grant(&app.pool, app.org_id, principal, consumer)
        .await
        .unwrap();
    assert_eq!(
        app.post(
            "/v1/outcomes",
            &token,
            &json!({
                "retrieval_id": found["retrieval_id"],
                "solution_id": solutions[0],
                "outcome": "used",
            })
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
