//! Regression coverage for trusted fix provenance and recoverable correlation.

mod common;

use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

const REV: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";
const OTHER: &str = "2222222222222222222222222222222222222222";

fn workflow(id: i64, attempt: i64, conclusion: &str) -> Value {
    json!({
        "action": "completed",
        "workflow_run": {
            "id": id, "run_attempt": attempt, "name": "integration-tests",
            "conclusion": conclusion, "head_sha": REV, "head_branch": "fix/nullable",
            "updated_at": format!("2026-10-20T12:{attempt:02}:00Z"),
            "display_title": "error[E0308]: mismatched types",
        },
        "repository": {"full_name": "acme/api", "default_branch": "main"},
    })
}

async fn send_workflow(app: &TestApp, id: i64, attempt: i64, result: &str) {
    assert_eq!(
        app.github_event(
            "workflow_run",
            &format!("workflow-{id}-{attempt}"),
            &workflow(id, attempt, result),
        )
        .await
        .status(),
        StatusCode::ACCEPTED,
    );
    app.drain().await;
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn dirty_base_revision_and_branch_never_verify_uncommitted_fix(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    // A successful check and a merged PR already exist before extraction.
    app.github_event(
        "check_run",
        "dirty-before",
        &check_run("acme/api", 1, "test", "success", REV, "ok"),
    )
    .await;
    app.github_event(
        "pull_request",
        "dirty-pr-before",
        &pr_merged("acme/api", 1, "fix/nullable", REV, OTHER),
    )
    .await;
    app.drain().await;
    let mut body = session_body("acme/api", Some(REV), Some("fix/nullable"));
    body["worktree_dirty"] = json!(true);
    let (_, solutions) = app
        .ingest_and_extract(&app.admin_token, "dirty-session", &body)
        .await;
    let id = solutions[0];
    assert_eq!(app.solution(id).await["verification"]["state"], "candidate");
    let provenance: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT fix_revision, fix_branch FROM solutions WHERE id = $1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(provenance, (None, None));

    // The same base and branch also cannot attach evidence delivered later.
    app.github_event(
        "pull_request",
        "dirty-pr-after",
        &pr_merged("acme/api", 2, "fix/nullable", REV, OTHER),
    )
    .await;
    app.github_event(
        "check_run",
        "dirty-after",
        &check_run("acme/api", 2, "test", "success", REV, "ok"),
    )
    .await;
    app.github_event(
        "check_run",
        "dirty-merge",
        &check_run("acme/api", 3, "test", "success", OTHER, "ok"),
    )
    .await;
    app.drain().await;
    let detail = app.solution(id).await;
    assert_eq!(detail["verification"]["state"], "candidate", "{detail}");
    let trusted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM evidence WHERE solution_id = $1 AND trusted")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(trusted, 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn reused_branch_does_not_attach_unrelated_pr_evidence(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let (_, ids) = app
        .ingest_and_extract(
            &app.admin_token,
            "branch-session",
            &session_body("acme/api", Some(REV), Some("fix/reused")),
        )
        .await;
    app.github_event(
        "pull_request",
        "unrelated-pr",
        &pr_merged(
            "acme/api",
            20,
            "fix/reused",
            OTHER,
            "3333333333333333333333333333333333333333",
        ),
    )
    .await;
    app.github_event(
        "check_run",
        "unrelated-check",
        &check_run("acme/api", 20, "test", "success", OTHER, "ok"),
    )
    .await;
    app.drain().await;
    assert_eq!(
        app.solution(ids[0]).await["verification"]["state"],
        "candidate"
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn workflow_attempts_keep_history_and_only_latest_result_controls_state(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let (_, ids) = app
        .ingest_and_extract(
            &app.admin_token,
            "attempt-session",
            &session_body("acme/api", Some(REV), None),
        )
        .await;
    let id = ids[0];
    send_workflow(&app, 44, 1, "success").await;
    assert_eq!(app.solution(id).await["verification"]["state"], "verified");
    send_workflow(&app, 44, 3, "failure").await;
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        true
    );
    // Late success for attempt 2 must not erase attempt 3's failure.
    send_workflow(&app, 44, 2, "success").await;
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        true
    );
    let history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM evidence WHERE solution_id = $1 AND kind = 'ci_check'",
    )
    .bind(id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(history, 3);
    let summary = antibody_db::evidence::trusted_summary(&app.pool, &[id])
        .await
        .unwrap();
    assert_eq!(
        summary[0].1, 0,
        "superseded successes are not current passing checks"
    );
    send_workflow(&app, 44, 4, "success").await;
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        false
    );
    let summary = antibody_db::evidence::trusted_summary(&app.pool, &[id])
        .await
        .unwrap();
    assert_eq!(summary[0].1, 1);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn historical_failed_attempt_wins_until_a_newer_rerun_passes(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    send_workflow(&app, 45, 3, "failure").await;
    send_workflow(&app, 45, 2, "success").await;
    let (_, ids) = app
        .ingest_and_extract(
            &app.admin_token,
            "historical-attempt-session",
            &session_body("acme/api", Some(REV), None),
        )
        .await;
    assert_eq!(
        app.solution(ids[0]).await["verification"]["state"],
        "candidate",
        "late historical success cannot verify after a newer failure",
    );
    send_workflow(&app, 45, 4, "success").await;
    assert_eq!(
        app.solution(ids[0]).await["verification"]["state"],
        "verified"
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn completed_extraction_retries_correlation_without_deleted_payload(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    app.github_event(
        "check_run",
        "preexisting-check",
        &check_run("acme/api", 9, "test", "success", REV, "ok"),
    )
    .await;
    app.drain().await;
    // Fail only trusted evidence writes: extraction and its untrusted claim
    // commit successfully, then correlation fails in its own transaction.
    sqlx::raw_sql(
        "CREATE FUNCTION reject_trusted_evidence() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN IF NEW.trusted THEN RAISE EXCEPTION 'injected correlation failure'; END IF;
         RETURN NEW; END $$;
         CREATE TRIGGER reject_trusted BEFORE INSERT ON evidence
         FOR EACH ROW EXECUTE FUNCTION reject_trusted_evidence();",
    )
    .execute(&app.pool)
    .await
    .unwrap();
    let body = session_body("acme/api", Some(REV), None);
    let ingested = TestApp::json(
        app.ingest(&app.admin_token, "retry-correlation", &body)
            .await,
        StatusCode::ACCEPTED,
    )
    .await;
    let session: Uuid = ingested["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE session_id = $1")
        .bind(session)
        .execute(&app.pool)
        .await
        .unwrap();
    app.drain().await;
    let status: String = sqlx::query_scalar("SELECT extraction_status FROM sessions WHERE id = $1")
        .bind(session)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(status, "succeeded");
    let payload_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_payloads WHERE session_id = $1")
            .bind(session)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(payload_count, 0);
    let job: Uuid = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE session_id = $1 AND kind = 'extract_session' AND state = 'dead'",
    )
    .bind(session)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    sqlx::query("DROP TRIGGER reject_trusted ON evidence")
        .execute(&app.pool)
        .await
        .unwrap();
    assert!(
        antibody_db::jobs::retry_dead(&app.pool, app.org_id, job)
            .await
            .unwrap()
    );
    app.drain().await;
    let solution: Uuid =
        sqlx::query_scalar("SELECT solution_id FROM solution_sessions WHERE session_id = $1")
            .bind(session)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(
        app.solution(solution).await["verification"]["state"],
        "verified"
    );
    let extracts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'session.extracted' AND resource_id = $1",
    )
    .bind(session.to_string())
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        extracts, 1,
        "retry resumes correlation without extracting twice"
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn outcomes_require_claimed_check_and_revision_and_revoke_failed_reruns(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let (_, ids) = app
        .ingest_and_extract(
            &app.admin_token,
            "outcome-session",
            &session_body("acme/api", Some(REV), None),
        )
        .await;
    send_workflow(&app, 11, 1, "success").await;
    let search = app.search(&app.admin_token, &json!({
        "task": "fix nullable id", "repository": "acme/api", "error": "error[E0308]: mismatched types",
    })).await;
    let result = TestApp::json(
        app.post(
            "/v1/outcomes",
            &app.admin_token,
            &json!({
                "retrieval_id": search["retrieval_id"], "solution_id": ids[0], "outcome": "used",
                "check": { "name": "integration-tests", "revision": &REV[..12], "result": "pass" },
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let outcome: Uuid = result["id"].as_str().unwrap().parse().unwrap();
    for (check_id, name, revision) in [(50, "lint", REV), (51, "integration-tests", OTHER)] {
        app.github_event(
            "check_run",
            &format!("outcome-{check_id}"),
            &check_run("acme/api", check_id, name, "success", revision, "ok"),
        )
        .await;
    }
    app.drain().await;
    async fn verification(app: &TestApp, id: Uuid) -> String {
        sqlx::query_scalar("SELECT verification FROM outcomes WHERE id = $1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap()
    }
    assert_eq!(verification(&app, outcome).await, "claimed");
    send_workflow(&app, 60, 1, "success").await;
    assert_eq!(verification(&app, outcome).await, "ci_verified");
    send_workflow(&app, 60, 3, "failure").await;
    assert_eq!(verification(&app, outcome).await, "claimed");
    send_workflow(&app, 60, 2, "success").await;
    assert_eq!(verification(&app, outcome).await, "claimed");
    send_workflow(&app, 60, 4, "success").await;
    assert_eq!(verification(&app, outcome).await, "ci_verified");
}
