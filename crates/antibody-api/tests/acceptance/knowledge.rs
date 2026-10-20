//! A04/A05: extraction, redaction, and verification lifecycle.

use super::REV;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;

// ---------------------------------------------------------------------------
// A04: extracted knowledge retains source references; redaction before storage

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a04_extraction_keeps_sources_and_redacts(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let mut body = session_body("acme/api", Some(REV), Some("fix/nullable"));
    let fake_token = format!("ghp_{}", "Ab12".repeat(9));
    body["messages"][0]["content"] = json!(format!(
        "cargo build fails; my GITHUB_TOKEN={fake_token} is set"
    ));
    let (session_id, sols) = app
        .ingest_and_extract(&app.admin_token, "key-redact-01", &body)
        .await;
    assert_eq!(sols.len(), 1);

    let detail = app.solution(sols[0]).await;
    assert!(
        detail["error_signatures"][0]
            .as_str()
            .unwrap()
            .starts_with("error[e0308]: mismatched types")
    );
    assert!(detail["remedy"].as_str().unwrap().contains("non-null cast"));
    assert!(
        detail["root_cause"]
            .as_str()
            .unwrap()
            .contains("nullable columns")
    );
    assert_eq!(detail["fix_revision"], REV);
    assert_eq!(detail["session_ids"][0], session_id.to_string());
    assert!(
        detail["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["kind"] == "session"),
        "{detail}"
    );
    assert_eq!(detail["verification"]["state"], "candidate");

    let status = TestApp::json(
        app.get(&format!("/v1/sessions/{session_id}"), &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    assert!(status["redaction_count"].as_i64().unwrap() >= 1);
    assert_eq!(status["capture_mode"], "structured_only");
    assert_eq!(
        status["raw_payload_available"], false,
        "structured-only payload is discarded after extraction"
    );

    // No stored text anywhere contains the secret.
    let dump: String = sqlx::query_scalar(
        "SELECT COALESCE(string_agg(t, ' '), '') FROM (
            SELECT payload::text AS t FROM session_payloads
            UNION ALL SELECT title || symptom || remedy || coalesce(root_cause,'') FROM solutions
            UNION ALL SELECT metadata::text FROM audit_events) x",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(!dump.contains(&fake_token));
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a04_sessions_without_a_solved_problem_create_no_knowledge(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let body = json!({
        "agent_type": "codex", "repository": "acme/api",
        "messages": [ { "role": "user", "content": "rename the variable" }, { "role": "assistant", "content": "Renamed it." } ]
    });
    let (id, sols) = app
        .ingest_and_extract(&app.admin_token, "key-noop-0001", &body)
        .await;
    assert!(sols.is_empty());
    let status = TestApp::json(
        app.get(&format!("/v1/sessions/{id}"), &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["extraction_status"], "skipped");
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a04_duplicates_corroborate_and_distinct_applicability_versions(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let (_, a) = app
        .ingest_and_extract(
            &app.admin_token,
            "key-dup-00001",
            &session_body("acme/api", None, None),
        )
        .await;
    let (_, b) = app
        .ingest_and_extract(
            &app.admin_token,
            "key-dup-00002",
            &session_body("acme/api", None, None),
        )
        .await;
    assert_eq!(
        a, b,
        "the same fix from another session corroborates instead of duplicating"
    );
    let d = app.solution(a[0]).await;
    assert_eq!(d["verification"]["state"], "corroborated");
    assert_eq!(d["corroboration_count"], 2);

    // Same problem under a different dependency range becomes a new version.
    let mut other = session_body("acme/api", None, None);
    other["hints"] = json!({ "dependencies": [ { "name": "sqlx", "version": "0.8.2" } ] });
    let (_, c) = app
        .ingest_and_extract(&app.admin_token, "key-dup-00003", &other)
        .await;
    assert_ne!(c, a);
    let dc = app.solution(c[0]).await;
    assert_eq!(dc["version"], 2);
    assert_eq!(dc["other_versions"][0]["id"], a[0].to_string());
}

// ---------------------------------------------------------------------------
// A05: merge is distinct from verification and deployment

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a05_merge_verification_and_deployment_are_separate(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let (_, sols) = app
        .ingest_and_extract(
            &app.admin_token,
            "key-merge-001",
            &session_body("acme/api", Some(REV), Some("fix/nullable")),
        )
        .await;
    let id = sols[0];
    let merge_sha = "9999999999999999999999999999999999999999";

    // Merging the PR corroborates but does not verify or deploy.
    assert_eq!(
        app.github_event(
            "pull_request",
            "d-1",
            &pr_merged("acme/api", 7, "fix/nullable", REV, merge_sha)
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let d = app.solution(id).await;
    assert_eq!(d["verification"]["state"], "corroborated");
    assert_eq!(d["verification"]["merge_state"], "merged");
    assert_eq!(d["verification"]["deployment_state"], "unknown");
    assert!(
        d["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["kind"] == "pull_request")
    );

    // An agent's claim that tests passed is recorded but never verifies.
    let search = app.search(&app.admin_token, &json!({ "task": "fix", "error": "error[E0308]: mismatched types", "repository": "acme/api" })).await;
    let rid = search["retrieval_id"].as_str().unwrap();
    let r = app
        .post(
            "/v1/outcomes",
            &app.admin_token,
            &json!({
                "retrieval_id": rid, "solution_id": id, "outcome": "used",
                "check": { "name": "cargo test", "revision": merge_sha, "result": "pass" }
            }),
        )
        .await;
    assert_eq!(
        TestApp::json(r, StatusCode::OK).await["verification"],
        "claimed"
    );
    assert_eq!(
        app.solution(id).await["verification"]["state"],
        "corroborated"
    );

    // Deployment is tracked separately and is not verification either.
    let dep = json!({
        "deployment_status": { "id": 5, "state": "success", "environment": "production" },
        "deployment": { "sha": merge_sha },
        "repository": { "full_name": "acme/api", "default_branch": "main" }
    });
    assert_eq!(
        app.github_event("deployment_status", "d-2", &dep)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let d = app.solution(id).await;
    assert_eq!(d["verification"]["deployment_state"], "deployed");
    assert_eq!(d["verification"]["state"], "corroborated");

    // Only a signed passing check on a revision containing the fix verifies.
    assert_eq!(
        app.github_event(
            "check_run",
            "d-3",
            &check_run("acme/api", 11, "test", "success", merge_sha, "all green")
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let d = app.solution(id).await;
    assert_eq!(d["verification"]["state"], "verified");
    assert!(
        d["verification"]["summary"]
            .as_str()
            .unwrap()
            .contains("trusted CI check `test`")
    );
    let trusted: Vec<_> = d["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["trusted"] == true && e["kind"] == "ci_check")
        .collect();
    assert_eq!(trusted.len(), 1);
    let claims: Vec<_> = d["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "agent_claim")
        .collect();
    assert!(claims.iter().all(|e| e["trusted"] == false));
}
