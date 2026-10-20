//! A10/A11 and Flow D: CI failures, Git invalidation and propagation.

use super::{REV, verified_fix};
use crate::common::*;
use antibody_core::model::{Permission, Scope};
use reqwest::StatusCode;
use serde_json::json;

// ---------------------------------------------------------------------------
// A10: CI failure finds the prior diagnosis (signed webhooks)

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a10_signed_ci_failure_finds_prior_diagnosis(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let id = verified_fix(&app).await;

    let event = json!({
        "repository": "acme/api", "revision": "abcdef1234567", "check_name": "build",
        "excerpt": "Compiling api\nerror[E0308]: mismatched types\n  --> src/handlers/user.rs:99:1\nerror: could not compile `api`"
    });
    let r = TestApp::json(app.ci_event("ci-1", &event, 0).await, StatusCode::OK).await;
    assert_eq!(r["blocking"], false);
    assert_eq!(r["duplicate"], false);
    assert_eq!(r["suggestions"][0]["solution_id"], id.to_string(), "{r}");

    // Replayed authenticated bytes are rejected even with a different delivery id.
    assert_eq!(
        TestApp::json(app.ci_event("ci-1", &event, 0).await, StatusCode::CONFLICT).await["error"]["code"],
        "replayed_delivery"
    );
    assert_eq!(
        app.ci_event("ci-2", &event, 0).await.status(),
        StatusCode::CONFLICT
    );
    // Independently reported failures with the same semantic signature deduplicate.
    let mut repeated = event.clone();
    repeated["excerpt"] = json!("error[E0308]: mismatched types\n  --> src/handlers/user.rs:100:1");
    assert_eq!(
        TestApp::json(
            app.ci_event("ci-2-new-report", &repeated, 0).await,
            StatusCode::OK
        )
        .await["duplicate"],
        true
    );
    // Bad signatures and stale timestamps fail.
    assert_eq!(
        app.ci_event("ci-3", &event, -3600).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let r = app
        .http
        .post(app.url("/v1/webhooks/ci"))
        .header("X-Antibody-Org", "acme")
        .header("X-Antibody-Delivery", "ci-4")
        .header(
            "X-Antibody-Timestamp",
            chrono::Utc::now().timestamp().to_string(),
        )
        .header("X-Antibody-Signature", "sha256=00")
        .header("content-type", "application/json")
        .body(event.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(
        TestApp::json(r, StatusCode::UNAUTHORIZED).await["error"]["code"],
        "invalid_signature"
    );

    // GitHub check_run failures run the same lookup asynchronously.
    let gh = check_run(
        "acme/api",
        77,
        "build",
        "failure",
        "fedcba9876543",
        "error[E0308]: mismatched types\n --> src/x.rs:1:1",
    );
    assert_eq!(
        app.github_event("check_run", "gh-1", &gh).await.status(),
        StatusCode::ACCEPTED
    );
    let bad = app
        .http
        .post(app.url("/v1/webhooks/github?org=acme"))
        .header("X-GitHub-Event", "check_run")
        .header("X-GitHub-Delivery", "gh-2")
        .header("X-Hub-Signature-256", "sha256=deadbeef")
        .header("content-type", "application/json")
        .body(gh.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        app.github_event("check_run", "gh-1", &gh).await.status(),
        StatusCode::CONFLICT,
        "replayed GitHub delivery"
    );
    app.drain().await;
    let events = TestApp::json(
        app.get("/v1/activity/ci-events", &app.admin_token).await,
        StatusCode::OK,
    )
    .await;
    let gh_event = events
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["source"] == "github_check_run")
        .unwrap();
    assert_eq!(gh_event["status"], "searched");
    assert_eq!(gh_event["suggested_solution_ids"][0], id.to_string());
    // A failing check on an unrelated revision does not touch the verified fix.
    assert_eq!(
        app.solution(id).await["verification"]["needs_revalidation"],
        false
    );
}

// ---------------------------------------------------------------------------
// A11: reverts and stale dependencies invalidate applicability

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a11_revert_and_dependency_changes_flag_revalidation(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let id = verified_fix(&app).await;

    // Revert of the fix commit.
    let revert = push(
        "acme/api",
        "aaaa111",
        json!([{
            "id": "aaaa111111111111111111111111111111111111",
            "message": format!("Revert \"Fix nullable id\"\n\nThis reverts commit {REV}."),
            "added": [], "modified": ["src/handlers/user.rs"], "removed": []
        }]),
    );
    assert_eq!(
        app.github_event("push", "p-1", &revert).await.status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let d = app.solution(id).await;
    assert_eq!(d["verification"]["needs_revalidation"], true);
    assert_eq!(d["verification"]["merge_state"], "reverted");
    assert!(
        d["verification"]["revalidation_reason"]
            .as_str()
            .unwrap()
            .contains("reverted")
    );
    let s = app.search(&app.admin_token, &json!({ "task": "x", "repository": "acme/api", "error": "error[E0308]: mismatched types" })).await;
    assert_eq!(s["suggestions"][0]["applicability"]["status"], "stale");
    assert!(
        s["suggestions"][0]["verification"]["summary"]
            .as_str()
            .unwrap()
            .contains("reverted")
    );

    // A dependency bump on the default branch flags fixes that depend on versions.
    let mut dep_body = session_body("acme/api", None, None);
    dep_body["messages"][2]["content"] = json!(
        "thread 'main' panicked at tokio-runtime: Cannot start a runtime from within a runtime"
    );
    dep_body["messages"][3]["content"] = json!(
        "tokio 1.38 forbids nested runtimes; switched to Handle::current().block_on in src/main.rs."
    );
    dep_body["hints"] = json!({ "dependencies": [ { "name": "tokio", "version": "1.38.0" } ] });
    let (_, dep_sols) = app
        .ingest_and_extract(&app.admin_token, "key-deps-0001", &dep_body)
        .await;
    let mut plain_body = session_body("acme/api", None, None);
    plain_body["messages"][2]["content"] =
        json!("TypeError: Cannot read properties of undefined (reading 'name')");
    plain_body["messages"][3]["content"] = json!(
        "Guarded the optional chaining in web/profile.ts so undefined users render a placeholder."
    );
    let (_, plain_sols) = app
        .ingest_and_extract(&app.admin_token, "key-deps-0002", &plain_body)
        .await;

    let bump = push(
        "acme/api",
        "bbbb222",
        json!([{
            "id": "bbbb222222222222222222222222222222222222", "message": "Bump tokio to 1.45",
            "added": [], "modified": ["Cargo.lock", "Cargo.toml"], "removed": []
        }]),
    );
    assert_eq!(
        app.github_event("push", "p-2", &bump).await.status(),
        StatusCode::ACCEPTED
    );
    app.drain().await;
    let dep = app.solution(dep_sols[0]).await;
    assert_eq!(dep["verification"]["needs_revalidation"], true, "{dep}");
    assert!(
        dep["verification"]["revalidation_reason"]
            .as_str()
            .unwrap()
            .contains("dependencies changed")
    );
    assert_eq!(
        app.solution(plain_sols[0]).await["verification"]["needs_revalidation"],
        false,
        "fixes without dependency constraints are untouched"
    );
    let audit = TestApp::json(
        app.get(
            "/v1/admin/audit?action=solution.invalidated",
            &app.admin_token,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(audit.as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Flow D: propagation candidates (informational, scope-filtered)

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn flow_d_propagation_lists_siblings_within_scope(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let api = app.create_repo("acme/api").await;
    let web = app.create_repo("acme/web").await;
    let id = verified_fix(&app).await;
    // The same error fails CI in another repository.
    let event = json!({ "repository": "acme/web", "revision": "abc1234def", "check_name": "build", "excerpt": "error[E0308]: mismatched types\n --> src/a.rs:1:1" });
    assert_eq!(
        app.ci_event("prop-1", &event, 0).await.status(),
        StatusCode::OK
    );

    let p = TestApp::json(
        app.get(&format!("/v1/solutions/{id}/propagation"), &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    let c = &p["candidates"][0];
    assert_eq!(c["repository"], "acme/web", "{p}");
    assert_eq!(c["matching_ci_failures"], 1);
    assert!(p["notice"].as_str().unwrap().contains("never applies"));

    // A caller without access to acme/web never sees it.
    let (_, member) = app
        .member("m", &[(api, Permission::Read)], &[Scope::ContextRead])
        .await;
    let p = TestApp::json(
        app.get(&format!("/v1/solutions/{id}/propagation"), &member)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(p["candidates"].as_array().unwrap().len(), 0);
    let _ = web;
}
