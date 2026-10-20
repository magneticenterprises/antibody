//! CI observation ordering and immutable fix provenance across PR updates.

mod common;

use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

const FIX: &str = "1111111111111111111111111111111111111111";
const REPLACEMENT: &str = "2222222222222222222222222222222222222222";
const MERGE: &str = "3333333333333333333333333333333333333333";

async fn seed(app: &TestApp) -> Uuid {
    app.create_repo("acme/api").await;
    app.ingest_and_extract(
        &app.admin_token,
        "ordering-session",
        &session_body("acme/api", Some(FIX), Some("fix/nullable")),
    )
    .await
    .1[0]
}

async fn send(app: &TestApp, event: &str, delivery: &str, payload: Value) {
    assert_eq!(
        app.github_event(event, delivery, &payload).await.status(),
        StatusCode::ACCEPTED,
    );
    app.drain().await;
}

fn check(id: i64, result: &str, revision: &str, completed_at: &str) -> Value {
    let mut payload = check_run(
        "acme/api",
        id,
        "integration-tests",
        result,
        revision,
        "check",
    );
    payload["check_run"]["completed_at"] = json!(completed_at);
    payload["check_run"]["app"] = json!({ "id": 99 });
    payload["check_run"]["check_suite"]["id"] = json!(100);
    payload
}

fn workflow(id: i64, result: &str, updated_at: &str) -> Value {
    json!({
        "action": "completed",
        "workflow_run": {
            "id": id, "workflow_id": 10, "run_attempt": 1,
            "name": "integration-tests", "path": ".github/workflows/integration.yml",
            "conclusion": result, "head_sha": FIX, "head_branch": "fix/nullable",
            "updated_at": updated_at, "display_title": "error[E0308]: mismatched types",
        },
        "repository": { "full_name": "acme/api", "default_branch": "main" },
    })
}

fn pull_request(head: &str, merge: Option<&str>, updated_at: &str) -> Value {
    json!({
        "action": if merge.is_some() { "closed" } else { "synchronize" },
        "number": 1,
        "pull_request": {
            "merged": merge.is_some(), "merge_commit_sha": merge,
            "title": "Fix nullable id", "updated_at": updated_at,
            "html_url": "https://github.com/acme/api/pull/1",
            "head": { "sha": head, "ref": "fix/nullable" },
        },
        "repository": { "full_name": "acme/api", "default_branch": "main" },
    })
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn corrected_conclusion_at_same_attempt_and_time_preserves_failure(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let id = seed(&app).await;
    for (result, delivery) in [
        ("success", "initial-result"),
        ("failure", "corrected-result"),
    ] {
        let payload = check_run("acme/api", 88, "integration-tests", result, FIX, "check");
        send(&app, "check_run", delivery, payload).await;
    }
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        true,
        "a corrected conclusion with no timestamp must not overwrite or lose failure evidence",
    );
    let history: Vec<String> = sqlx::query_scalar(
        "SELECT result FROM evidence WHERE solution_id = $1 AND kind = 'ci_check' ORDER BY result",
    )
    .bind(id)
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(history, vec!["fail", "pass"]);
    let summary = antibody_db::evidence::trusted_summary(&app.pool, &[id])
        .await
        .unwrap();
    assert_eq!(summary[0].1, 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn fractional_completion_time_orders_later_failure_after_whole_second_pass(
    pool: sqlx::PgPool,
) {
    let app = TestApp::spawn(pool).await;
    let id = seed(&app).await;
    send(
        &app,
        "check_run",
        "whole-second-pass",
        check(1, "success", FIX, "2026-10-20T12:00:00Z"),
    )
    .await;
    assert_eq!(app.solution(id).await["verification"]["state"], "verified");
    send(
        &app,
        "check_run",
        "fractional-second-failure",
        check(1, "failure", FIX, "2026-10-20T12:00:00.100Z"),
    )
    .await;
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        true
    );
    let summary = antibody_db::evidence::trusted_summary(&app.pool, &[id])
        .await
        .unwrap();
    assert_eq!(summary[0].1, 0, "the earlier pass is no longer current");
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn delayed_success_from_an_earlier_run_id_cannot_erase_newer_failure(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let id = seed(&app).await;
    let search = app
        .search(
            &app.admin_token,
            &json!({
                "task": "fix nullable id", "repository": "acme/api",
                "error": "error[E0308]: mismatched types",
            }),
        )
        .await;
    let result = TestApp::json(
        app.post(
            "/v1/outcomes",
            &app.admin_token,
            &json!({
                "retrieval_id": search["retrieval_id"], "solution_id": id, "outcome": "used",
                "check": { "name": "integration-tests", "revision": FIX, "result": "pass" },
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let outcome: Uuid = result["id"].as_str().unwrap().parse().unwrap();
    async fn verification(app: &TestApp, id: Uuid) -> String {
        sqlx::query_scalar("SELECT verification FROM outcomes WHERE id = $1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap()
    }

    send(
        &app,
        "workflow_run",
        "run-60",
        workflow(60, "success", "2026-10-20T12:00:00Z"),
    )
    .await;
    assert_eq!(verification(&app, outcome).await, "ci_verified");
    send(
        &app,
        "workflow_run",
        "run-61",
        workflow(61, "failure", "2026-10-20T12:02:00Z"),
    )
    .await;
    assert_eq!(verification(&app, outcome).await, "claimed");
    send(
        &app,
        "workflow_run",
        "delayed-run-59",
        workflow(59, "success", "2026-10-20T11:59:00Z"),
    )
    .await;
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        true
    );
    assert_eq!(verification(&app, outcome).await, "claimed");
    let summary = antibody_db::evidence::trusted_summary(&app.pool, &[id])
        .await
        .unwrap();
    assert_eq!(summary[0].1, 0);
    send(
        &app,
        "workflow_run",
        "run-62",
        workflow(62, "success", "2026-10-20T12:03:00Z"),
    )
    .await;
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        false
    );
    assert_eq!(verification(&app, outcome).await, "ci_verified");
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn force_pushed_pr_and_its_merge_do_not_verify_the_previous_head_fix(pool: sqlx::PgPool) {
    assert_force_pushed_pr_cannot_verify(pool, true).await;
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn force_pushed_pr_merge_alone_does_not_verify_the_previous_head_fix(pool: sqlx::PgPool) {
    assert_force_pushed_pr_cannot_verify(pool, false).await;
}

async fn assert_force_pushed_pr_cannot_verify(pool: sqlx::PgPool, check_head: bool) {
    let app = TestApp::spawn(pool).await;
    let id = seed(&app).await;
    send(
        &app,
        "pull_request",
        "pr-original",
        pull_request(FIX, None, "2026-10-20T12:00:00Z"),
    )
    .await;
    send(
        &app,
        "pull_request",
        "pr-force-pushed",
        pull_request(REPLACEMENT, None, "2026-10-20T12:01:00Z"),
    )
    .await;
    if check_head {
        send(
            &app,
            "check_run",
            "replacement-head-pass",
            check(20, "success", REPLACEMENT, "2026-10-20T12:02:00Z"),
        )
        .await;
        assert_eq!(app.solution(id).await["verification"]["state"], "candidate");
    }
    send(
        &app,
        "pull_request",
        "replacement-merged",
        pull_request(REPLACEMENT, Some(MERGE), "2026-10-20T12:03:00Z"),
    )
    .await;
    send(
        &app,
        "check_run",
        "replacement-merge-pass",
        check(21, "success", MERGE, "2026-10-20T12:04:00Z"),
    )
    .await;
    let detail = app.solution(id).await;
    assert_eq!(detail["verification"]["state"], "candidate", "{detail}");
    assert_ne!(detail["verification"]["merge_state"], "merged");
    send(
        &app,
        "deployment_status",
        "replacement-merge-deployed",
        json!({
            "deployment_status": { "id": 100, "state": "success", "environment": "production" },
            "deployment": { "sha": MERGE },
            "repository": { "full_name": "acme/api", "default_branch": "main" },
        }),
    )
    .await;
    assert_ne!(
        app.solution(id).await["verification"]["deployment_state"],
        "deployed"
    );
    let trusted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM evidence WHERE solution_id = $1 AND trusted")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(trusted, 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn unknown_completion_times_do_not_let_delayed_success_clear_a_failure(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let id = seed(&app).await;
    for (run_id, conclusion) in [(60, "success"), (61, "failure"), (59, "success")] {
        let mut payload = workflow(run_id, conclusion, "2026-10-20T12:00:00Z");
        payload["workflow_run"]
            .as_object_mut()
            .unwrap()
            .remove("updated_at");
        send(
            &app,
            "workflow_run",
            &format!("unknown-time-{run_id}"),
            payload,
        )
        .await;
    }
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        true
    );
    let summary = antibody_db::evidence::trusted_summary(&app.pool, &[id])
        .await
        .unwrap();
    assert_eq!(summary[0].1, 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn merged_pr_with_the_recorded_fix_head_still_accepts_merge_revision_check(
    pool: sqlx::PgPool,
) {
    let app = TestApp::spawn(pool).await;
    let id = seed(&app).await;
    send(
        &app,
        "pull_request",
        "valid-pr-open",
        pull_request(FIX, None, "2026-10-20T12:00:00Z"),
    )
    .await;
    send(
        &app,
        "pull_request",
        "valid-pr-merged",
        pull_request(FIX, Some(MERGE), "2026-10-20T12:01:00Z"),
    )
    .await;
    send(
        &app,
        "check_run",
        "valid-merge-pass",
        check(30, "success", MERGE, "2026-10-20T12:02:00Z"),
    )
    .await;
    let detail = app.solution(id).await;
    assert_eq!(detail["verification"]["state"], "verified", "{detail}");
    assert_eq!(detail["verification"]["merge_state"], "merged");
}
