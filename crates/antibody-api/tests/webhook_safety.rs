//! Signed webhook trust boundaries and repository identity regressions.
mod common;

use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

fn secret() -> String {
    format!("ghp_{}", "SyntheticTestOnly123".repeat(2))
}

fn repository_event(action: &str, name: &str, timestamp: &str) -> Value {
    json!({
        "action": action,
        "repository": { "id": 42, "full_name": name, "html_url": format!("https://github.com/{name}"),
            "default_branch": "trunk", "updated_at": timestamp }
    })
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn signed_body_replay_cannot_change_delivery_or_regress_sources(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let repo = app.create_repo("acme/api").await;
    let mut opened = pr_merged("acme/api", 7, "fix/topic", "abcdef1", "abcdef2");
    opened["action"] = json!("opened");
    opened["pull_request"]["merged"] = json!(false);
    opened["pull_request"]["merge_commit_sha"] = Value::Null;
    opened["pull_request"]["updated_at"] = json!("2026-10-01T00:00:00Z");
    assert_eq!(
        app.github_event("pull_request", "pr-old", &opened)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let mut merged = pr_merged("acme/api", 7, "fix/topic", "abcdef1", "abcdef2");
    merged["pull_request"]["updated_at"] = json!("2026-10-02T00:00:00Z");
    assert_eq!(
        app.github_event("pull_request", "pr-new", &merged)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    // Delivery diagnostics can expire, but the authenticated payload cannot be replayed.
    sqlx::query("DELETE FROM webhook_deliveries")
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        app.github_event("pull_request", "forged-delivery", &opened)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let forged = TestApp::json(
        app.github_event("repository", "forged-event-type", &opened)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(forged["accepted"], false);
    // A separately signed older observation cannot overwrite current state either.
    opened["pull_request"]["title"] = json!("stale title");
    assert_eq!(
        app.github_event("pull_request", "older-new-body", &opened)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let state: (String, String) = sqlx::query_as("SELECT state, title FROM source_artifacts WHERE repository_id = $1 AND kind = 'pull_request'")
        .bind(repo).fetch_one(&app.pool).await.unwrap();
    assert_eq!(state, ("merged".into(), "Fix nullable id".into()));
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn github_content_is_redacted_before_queue_and_artifact_storage(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let token = secret();
    let mut commit = push(
        "acme/api",
        "abcdef1",
        json!([{
            "id": "abcdef1", "message": format!("Fix {token}"),
            "url": format!("https://github.com/acme/api/commit/abcdef1?access_token={token}"),
            "modified": [format!("src/{token}.rs")]
        }]),
    );
    commit["ref"] = json!(format!("refs/heads/{token}"));
    let mut pr = pr_merged("acme/api", 3, &token, "abcdef1", "abcdef2");
    pr["pull_request"]["title"] = json!(format!("Fix {token}"));
    pr["pull_request"]["html_url"] = json!(format!("https://example.test/{token}"));
    let mut check = check_run("acme/api", 12, &token, "failure", "abcdef3", &token);
    check["check_run"]["html_url"] = json!(format!("https://example.test/{token}"));
    check["check_run"]["check_suite"]["head_branch"] = json!(token);
    let deployment = json!({
        "repository": { "full_name": "acme/api" }, "deployment": { "sha": "abcdef1" },
        "deployment_status": { "id": 21, "state": "success", "environment": token,
            "target_url": format!("https://example.test/{token}") }
    });
    for (name, id, event) in [
        ("push", "redact-push", commit),
        ("pull_request", "redact-pr", pr),
        ("check_run", "redact-check", check),
        ("deployment_status", "redact-deploy", deployment),
    ] {
        assert_eq!(
            app.github_event(name, id, &event).await.status(),
            StatusCode::ACCEPTED
        );
    }
    let payloads: Vec<String> = sqlx::query_scalar("SELECT payload::text FROM jobs")
        .fetch_all(&app.pool)
        .await
        .unwrap();
    assert_eq!(payloads.len(), 4);
    for payload in payloads {
        assert!(
            !payload.contains(&token),
            "secret persisted in queued event"
        );
        assert!(payload.contains("REDACTED"));
    }
    app.drain().await;
    let stored: Vec<String> = sqlx::query_scalar("SELECT row_to_json(a)::text FROM source_artifacts a UNION ALL SELECT row_to_json(c)::text FROM ci_events c")
        .fetch_all(&app.pool).await.unwrap();
    assert!(!stored.is_empty());
    assert!(stored.iter().all(|s| !s.contains(&token)));
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn generic_ci_redacts_all_fields_and_rejects_changed_delivery_replay(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let token = secret();
    let event = json!({
        "repository": "acme/api", "revision": token, "check_name": format!("build {token}"),
        "signature": format!("TypeError: credential {token}"), "excerpt": format!("failed {token}"),
        "url": format!("https://example.test/{token}"), "component_path": format!("src/{token}")
    });
    let response = TestApp::json(app.ci_event("ci-secret", &event, 0).await, StatusCode::OK).await;
    assert!(!response.to_string().contains(&token));
    assert_eq!(
        app.ci_event("ci-replay", &event, 0).await.status(),
        StatusCode::CONFLICT
    );
    let stored: Vec<String> = sqlx::query_scalar("SELECT row_to_json(c)::text FROM ci_events c UNION ALL SELECT row_to_json(r)::text FROM retrievals r")
        .fetch_all(&app.pool).await.unwrap();
    assert!(stored.len() >= 2);
    assert!(stored.iter().all(|s| !s.contains(&token)));
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn repository_identity_survives_lifecycle_without_overriding_admin_disconnect(
    pool: sqlx::PgPool,
) {
    let app = TestApp::spawn(pool).await;
    let repo = app.create_repo("acme/api").await;
    // The first signed event binds the numeric provider identity.
    let mut initial = push(
        "acme/api",
        "abcdef1",
        json!([{ "id": "abcdef1", "message": "Fix build" }]),
    );
    initial["repository"]["id"] = json!(42);
    assert_eq!(
        app.github_event("push", "identity-bind", &initial)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    for (n, action, name, time) in [
        (1, "renamed", "acme/renamed", "2026-10-01T00:00:00Z"),
        (2, "transferred", "other/renamed", "2026-10-02T00:00:00Z"),
        (3, "archived", "other/renamed", "2026-10-03T00:00:00Z"),
        (4, "unarchived", "other/renamed", "2026-10-04T00:00:00Z"),
    ] {
        let event = repository_event(action, name, time);
        assert_eq!(
            app.github_event("repository", &format!("life-{n}"), &event)
                .await
                .status(),
            StatusCode::ACCEPTED
        );
        app.drain().await;
        let row = antibody_db::repos::get(&app.pool, app.org_id, repo)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.provider_repo_id.as_deref(), Some("42"));
        assert_eq!(row.full_name, name);
        assert_eq!(
            row.url.as_deref(),
            Some(format!("https://github.com/{name}").as_str())
        );
        assert_eq!(row.default_branch, "trunk");
        assert_eq!(
            row.status,
            if action == "archived" {
                "disconnected"
            } else {
                "active"
            }
        );
        let source_state: String = sqlx::query_scalar(
            "SELECT state FROM source_artifacts WHERE repository_id = $1 AND kind = 'commit'",
        )
        .bind(repo)
        .fetch_one(&app.pool)
        .await
        .unwrap();
        assert_eq!(
            source_state,
            if action == "archived" {
                "unavailable"
            } else {
                "merged"
            }
        );
    }
    // A newer delivery carrying older provider state cannot roll the name back.
    let stale = repository_event("renamed", "acme/stale", "2026-10-01T12:00:00Z");
    assert_eq!(
        app.github_event("repository", "stale-life", &stale)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    assert_eq!(
        antibody_db::repos::get(&app.pool, app.org_id, repo)
            .await
            .unwrap()
            .unwrap()
            .full_name,
        "other/renamed"
    );
    assert_eq!(
        app.http
            .patch(app.url(&format!("/v1/repositories/{repo}")))
            .bearer_auth(&app.admin_token)
            .json(&json!({"status":"disconnected"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let unarchive = repository_event("unarchived", "other/renamed", "2026-10-05T00:00:00Z");
    assert_eq!(
        app.github_event("repository", "admin-unarchive", &unarchive)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let mut ordinary = push("other/renamed", "abcdef2", json!([]));
    ordinary["repository"]["id"] = json!(42);
    let response = TestApp::json(
        app.github_event("push", "admin-push", &ordinary).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(response["accepted"], false);
    assert_eq!(
        antibody_db::repos::get(&app.pool, app.org_id, repo)
            .await
            .unwrap()
            .unwrap()
            .status,
        "disconnected"
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn first_rename_binds_identity_from_signed_previous_name(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let repo = app.create_repo("acme/api").await;
    let mut event = repository_event("renamed", "acme/new", "2026-10-01T00:00:00Z");
    event["changes"] = json!({ "name": { "from": "api" } });
    assert_eq!(
        app.github_event("repository", "first-rename", &event)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let row = antibody_db::repos::get(&app.pool, app.org_id, repo)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.provider_repo_id.as_deref(), Some("42"));
    assert_eq!(row.full_name, "acme/new");
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn ignored_event_does_not_consume_payload_before_registration(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let event = push("acme/api", "abcdef1", json!([]));
    let ignored = TestApp::json(
        app.github_event("push", "before-register", &event).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(ignored["accepted"], false);
    app.create_repo("acme/api").await;
    let unsupported = TestApp::json(
        app.github_event("star", "wrong-event-header", &event).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(unsupported["accepted"], false);
    let ping = TestApp::json(
        app.github_event("ping", "wrong-ping-header", &event).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(ping["detail"], "pong");
    assert_eq!(
        app.github_event("push", "after-register", &event)
            .await
            .status(),
        StatusCode::ACCEPTED
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn unsigned_event_header_cannot_consume_another_event_payload(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let mut pr = pr_merged("acme/api", 1, "fix/build", "abcdef1", "abcdef2");
    // `edited` is valid for a repository event too; the signed PR object must
    // prevent that misclassification even though action alone looks valid.
    pr["action"] = json!("edited");
    let check = check_run(
        "acme/api",
        3,
        "build",
        "failure",
        "abcdef3",
        "TypeError: invalid build",
    );
    let member = json!({ "action": "edited", "member": {"login":"teammate"}, "repository": {"full_name":"acme/api"} });
    for (n, actual, payload) in [
        (1, "pull_request", pr),
        (2, "check_run", check),
        (3, "member", member),
    ] {
        for wrong in ["repository", "member", "organization"] {
            if wrong == actual {
                continue;
            }
            let ignored = TestApp::json(
                app.github_event(wrong, &format!("wrong-{n}-{wrong}"), &payload)
                    .await,
                StatusCode::OK,
            )
            .await;
            assert_eq!(ignored["accepted"], false);
        }
        assert_eq!(
            app.github_event(actual, &format!("actual-{n}"), &payload)
                .await
                .status(),
            StatusCode::ACCEPTED
        );
    }
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn lifecycle_jobs_survive_delivery_diagnostic_expiry(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let repo = app.create_repo("acme/api").await;
    for (n, name) in [(1, "acme/current"), (2, "acme/legacy")] {
        let mut event = repository_event("renamed", name, &format!("2026-10-0{n}T00:00:00Z"));
        if n == 1 {
            event["changes"] = json!({ "name": { "from": "api" } });
        }
        assert_eq!(
            app.github_event("repository", &format!("expire-{n}"), &event)
                .await
                .status(),
            StatusCode::ACCEPTED
        );
        sqlx::query("DELETE FROM webhook_deliveries")
            .execute(&app.pool)
            .await
            .unwrap();
        if n == 2 {
            // Jobs accepted before the receipt field was introduced recover the
            // durable original creation time, never the current retry time.
            sqlx::query("UPDATE jobs SET payload = payload - 'received_at' WHERE state = 'queued'")
                .execute(&app.pool)
                .await
                .unwrap();
        }
        app.drain().await;
        assert_eq!(
            antibody_db::repos::get(&app.pool, app.org_id, repo)
                .await
                .unwrap()
                .unwrap()
                .full_name,
            name
        );
    }
}
