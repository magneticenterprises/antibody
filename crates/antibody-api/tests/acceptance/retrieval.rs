//! A07-A09: retrieval of verified fixes, compatibility and isolation (REST and MCP).

use super::{REV, verified_fix};
use crate::common::*;
use antibody_core::model::{Permission, Scope};
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// A07: a known exact error finds the prior verified fix (cross-repo)

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a07_exact_error_finds_verified_fix_across_repositories(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let api = app.create_repo("acme/api").await;
    let web = app.create_repo("acme/web").await;
    let id = verified_fix(&app).await;

    // Engineer B works in another repository and sees the same error with
    // different paths and line numbers.
    let (_, b) = app
        .member(
            "engineer-b",
            &[(api, Permission::Read), (web, Permission::Write)],
            &[Scope::ContextRead, Scope::OutcomesWrite],
        )
        .await;
    let resp = app.search(&b, &json!({
        "task": "Build broke after bumping sqlx",
        "repository": "acme/web",
        "error": "error[E0308]: mismatched types\n --> /Users/bob/web/src/handlers/user.rs:7:3"
    })).await;
    let s = &resp["suggestions"];
    assert_eq!(s.as_array().unwrap().len(), 1, "{resp}");
    assert_eq!(s[0]["solution_id"], id.to_string());
    assert_eq!(s[0]["verification"]["state"], "verified");
    assert_eq!(s[0]["cross_repository"], true);
    assert!(
        s[0]["match_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "exact error signature")
    );
    assert!(!s[0]["source_links"].as_array().unwrap().is_empty());
    assert!(
        resp["notice"]
            .as_str()
            .unwrap()
            .contains("not instructions")
    );
    assert!(resp["latency_ms"].as_u64().unwrap() < 1500);

    // At most three suggestions, even when more are requested.
    let r = app
        .search(
            &b,
            &json!({ "task": "x", "error": "error[E0308]: mismatched types", "limit": 50 }),
        )
        .await;
    assert!(r["suggestions"].as_array().unwrap().len() <= 3);
}

// ---------------------------------------------------------------------------
// A08: similar but incompatible fixes are rejected or qualified

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a08_incompatible_fix_is_suppressed_or_qualified(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let mut body = session_body("acme/api", None, None);
    body["hints"] =
        json!({ "dependencies": [ { "name": "sqlx", "version": "0.8.2", "ecosystem": "cargo" } ] });
    let (_, sols) = app
        .ingest_and_extract(&app.admin_token, "key-compat-01", &body)
        .await;
    let d = app.solution(sols[0]).await;
    assert_eq!(
        d["applicability"]["dependencies"][0]["version_req"],
        "^0.8.2"
    );

    let query = |deps: serde_json::Value, include: bool| {
        json!({
            "task": "build fails", "repository": "acme/api", "error": "error[E0308]: mismatched types",
            "dependencies": deps, "include_incompatible": include
        })
    };
    let old = app
        .search(
            &app.admin_token,
            &query(json!([{ "name": "sqlx", "version": "0.6.3" }]), false),
        )
        .await;
    assert_eq!(old["suggestions"].as_array().unwrap().len(), 0, "{old}");
    assert_eq!(old["suppressed_incompatible"], 1);

    let qualified = app
        .search(
            &app.admin_token,
            &query(json!([{ "name": "sqlx", "version": "0.6.3" }]), true),
        )
        .await;
    let a = &qualified["suggestions"][0]["applicability"];
    assert_eq!(a["status"], "incompatible");
    assert!(
        a["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("you have 0.6.3"))
    );

    let ok = app
        .search(
            &app.admin_token,
            &query(json!([{ "name": "sqlx", "version": "0.8.6" }]), false),
        )
        .await;
    assert_eq!(
        ok["suggestions"][0]["applicability"]["status"],
        "compatible"
    );

    let unknown = app.search(&app.admin_token, &query(json!([]), false)).await;
    assert_eq!(
        unknown["suggestions"][0]["applicability"]["status"],
        "unknown"
    );
}

// ---------------------------------------------------------------------------
// A09: authorized cross-repo retrieval works; unauthorized leaks nothing

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a09_isolation_between_repositories_and_organizations(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool.clone()).await;
    let api = app.create_repo("acme/api").await;
    let _secret = app.create_repo("acme/secret").await;
    let mut body = session_body("acme/secret", Some(REV), None);
    body["messages"][2]["content"] =
        json!("panicked at 'vault unsealed key mismatch: KEYRING_ROTATION_FAILED'");
    body["messages"][3]["content"] = json!(
        "Rotated the keyring by re-running the unseal ceremony with the backup shard; the service starts now."
    );
    let (secret_session, sols) = app
        .ingest_and_extract(&app.admin_token, "key-secret-01", &body)
        .await;
    let secret_id = sols[0];

    let (_, member) = app
        .member(
            "m",
            &[(api, Permission::Write)],
            &[
                Scope::ContextRead,
                Scope::SessionsWrite,
                Scope::OutcomesWrite,
            ],
        )
        .await;
    let q = json!({ "task": "vault keyring rotation failed", "error": "panicked at 'vault unsealed key mismatch: KEYRING_ROTATION_FAILED'" });

    // The admin sees it; the member sees nothing, not even counts.
    assert_eq!(
        app.search(&app.admin_token, &q).await["suggestions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let m = app.search(&member, &q).await;
    assert_eq!(m["suggestions"].as_array().unwrap().len(), 0);
    assert_eq!(m["suppressed_incompatible"], 0);
    let named = app.search(&member, &json!({ "task": "x", "repository": "acme/secret", "error": "KEYRING_ROTATION_FAILED" })).await;
    let missing = app
        .search(
            &member,
            &json!({ "task": "x", "repository": "acme/nope", "error": "KEYRING_ROTATION_FAILED" }),
        )
        .await;
    assert_eq!(
        named["degraded_reasons"], missing["degraded_reasons"],
        "inaccessible and nonexistent repos look identical"
    );
    assert_eq!(named["suggestions"].as_array().unwrap().len(), 0);

    // Direct reads are 404, lists exclude it, overview counts exclude it.
    assert_eq!(
        app.get(&format!("/v1/solutions/{secret_id}"), &member)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("/v1/sessions/{secret_session}"), &member)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let list = TestApp::json(
        app.get("/v1/solutions?q=keyring", &member).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(list["total"], 0);
    let ov = TestApp::json(app.get("/v1/overview", &member).await, StatusCode::OK).await;
    assert_eq!(ov["stats"]["solutions_total"], 0);
    let repos = TestApp::json(app.get("/v1/repositories", &member).await, StatusCode::OK).await;
    assert_eq!(repos["items"].as_array().unwrap().len(), 1);

    // Granting read access makes authorized cross-repo retrieval work.
    let secret_repo: Uuid =
        sqlx::query_scalar("SELECT id FROM repositories WHERE full_name = 'acme/secret'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    let mid: Uuid = sqlx::query_scalar("SELECT id FROM principals WHERE display_name = 'm'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    antibody_db::access::grant_repository(
        &app.pool,
        app.org_id,
        mid,
        secret_repo,
        Permission::Read,
    )
    .await
    .unwrap();
    let m2 = app.search(&member, &json!({ "task": "vault", "repository": "acme/api", "error": "panicked at 'vault unsealed key mismatch: KEYRING_ROTATION_FAILED'" })).await;
    assert_eq!(m2["suggestions"][0]["cross_repository"], true);

    // A token allow-list narrows access even for an admin principal.
    let narrow = antibody_db::tokens::issue_token(
        &app.pool,
        antibody_db::tokens::NewToken {
            org_id: app.org_id,
            principal_id: app.admin_principal,
            name: "narrow",
            scopes: &[Scope::ContextRead],
            repository_ids: Some(&[api]),
            expires_at: None,
            created_by: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        app.search(&narrow.token, &q).await["suggestions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // Another organization sees nothing.
    let other = antibody_api::bootstrap::bootstrap(
        &pool,
        "globex",
        "Globex",
        "globex-admin",
        Some(ADMIN_PASSWORD.into()),
    )
    .await
    .unwrap();
    let g = app.search(&other.admin_token, &q).await;
    assert_eq!(g["suggestions"].as_array().unwrap().len(), 0);
    assert_eq!(
        app.get(&format!("/v1/solutions/{secret_id}"), &other.admin_token)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let gl = TestApp::json(
        app.get("/v1/solutions", &other.admin_token).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(gl["total"], 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a09_mcp_shares_retrieval_and_permissions(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let api = app.create_repo("acme/api").await;
    let id = verified_fix(&app).await;
    let (_, member) = app
        .member(
            "m",
            &[(api, Permission::Read)],
            &[Scope::ContextRead, Scope::OutcomesWrite],
        )
        .await;
    let (_, outsider) = app.member("o", &[], &[Scope::ContextRead]).await;

    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}});
    let anon = app
        .http
        .post(app.url("/mcp"))
        .header("accept", "application/json, text/event-stream")
        .json(&init)
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
    let r = mcp(&app, &member, init).await;
    assert_eq!(r["result"]["serverInfo"]["name"], "antibody", "{r}");

    let tools = mcp(
        &app,
        &member,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    )
    .await;
    let mut names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["find_relevant_context", "get_solution", "report_outcome"]
    );

    let call = |name: &str, args: serde_json::Value| json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":args}});
    let args = json!({ "task": "build fails", "error": "error[E0308]: mismatched types", "repository": "acme/api" });
    let found = mcp(&app, &member, call("find_relevant_context", args.clone())).await;
    let sc = &found["result"]["structuredContent"];
    assert_eq!(
        sc["suggestions"][0]["solution_id"],
        id.to_string(),
        "{found}"
    );
    assert!(
        found["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("untrusted reference data")
    );
    let retrieval_id = sc["retrieval_id"].as_str().unwrap().to_string();

    let hidden = mcp(&app, &outsider, call("find_relevant_context", args)).await;
    assert_eq!(
        hidden["result"]["structuredContent"]["suggestions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let get_hidden = mcp(
        &app,
        &outsider,
        call("get_solution", json!({ "solution_id": id })),
    )
    .await;
    assert_eq!(get_hidden["result"]["isError"], true);

    let outcome = mcp(
        &app,
        &member,
        call(
            "report_outcome",
            json!({ "retrieval_id": retrieval_id, "solution_id": id, "outcome": "used" }),
        ),
    )
    .await;
    assert_eq!(
        outcome["result"]["structuredContent"]["verification"], "none",
        "{outcome}"
    );
    let channels: Vec<String> =
        sqlx::query_scalar("SELECT channel FROM retrievals ORDER BY created_at")
            .fetch_all(&app.pool)
            .await
            .unwrap();
    assert!(channels.iter().any(|c| c == "mcp"));
}

async fn mcp(app: &TestApp, token: &str, body: serde_json::Value) -> serde_json::Value {
    let r = app
        .http
        .post(app.url("/mcp"))
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status();
    let text = r.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{text}");
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("non-JSON MCP response: {text}"))
}
