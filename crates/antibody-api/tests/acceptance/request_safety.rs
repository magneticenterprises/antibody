//! A03: invalid, unauthorized and oversized requests fail safely.

use super::REV;
use crate::common::*;
use antibody_core::model::{Permission, Scope};
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// A03: invalid / unauthorized / oversized payloads fail safely

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a03_negative_ingestion_cases(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let api = app.create_repo("acme/api").await;
    let secret_repo = app.create_repo("acme/secret").await;
    let _ = secret_repo;
    let body = session_body("acme/api", Some(REV), None);
    let code = |v: serde_json::Value| v["error"]["code"].as_str().unwrap_or_default().to_string();

    // No credentials / bad credentials.
    let r = app
        .http
        .post(app.url("/v1/sessions"))
        .header("Idempotency-Key", "k-12345678")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        code(TestApp::json(r, StatusCode::UNAUTHORIZED).await),
        "unauthenticated"
    );
    let r = app
        .ingest(
            "abt_not_a_real_token_000000000000000000000000",
            "k-12345678",
            &body,
        )
        .await;
    assert_eq!(
        code(TestApp::json(r, StatusCode::UNAUTHORIZED).await),
        "unauthenticated"
    );

    // Missing idempotency key.
    let r = app.post("/v1/sessions", &app.admin_token, &body).await;
    assert_eq!(
        code(TestApp::json(r, StatusCode::BAD_REQUEST).await),
        "idempotency_key_required"
    );

    // Wrong content type.
    let r = app
        .http
        .post(app.url("/v1/sessions"))
        .bearer_auth(&app.admin_token)
        .header("Idempotency-Key", "k-12345678")
        .header("content-type", "text/plain")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(
        code(TestApp::json(r, StatusCode::UNSUPPORTED_MEDIA_TYPE).await),
        "unsupported_media_type"
    );

    // Malformed JSON and unknown fields.
    let r = app
        .http
        .post(app.url("/v1/sessions"))
        .bearer_auth(&app.admin_token)
        .header("Idempotency-Key", "k-12345678")
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(
        code(TestApp::json(r, StatusCode::BAD_REQUEST).await),
        "invalid_request"
    );
    let mut unknown = body.clone();
    unknown["shell_command"] = json!("rm -rf /");
    assert_eq!(
        code(
            TestApp::json(
                app.ingest(&app.admin_token, "k-12345679", &unknown).await,
                StatusCode::BAD_REQUEST
            )
            .await
        ),
        "invalid_request"
    );

    // Unsafe content: NUL bytes, non-hex revision.
    let mut nul = body.clone();
    nul["messages"][0]["content"] = json!("bad\u{0000}byte");
    assert_eq!(
        code(
            TestApp::json(
                app.ingest(&app.admin_token, "k-12345680", &nul).await,
                StatusCode::BAD_REQUEST
            )
            .await
        ),
        "invalid_request"
    );
    let mut rev = body.clone();
    rev["revision"] = json!("main; rm -rf");
    assert_eq!(
        code(
            TestApp::json(
                app.ingest(&app.admin_token, "k-12345681", &rev).await,
                StatusCode::BAD_REQUEST
            )
            .await
        ),
        "invalid_request"
    );

    // Oversized (test config limit is 512 KiB).
    let mut big = body.clone();
    big["messages"][0]["content"] = json!("x".repeat(600 * 1024));
    let r = app.ingest(&app.admin_token, "k-12345682", &big).await;
    assert_eq!(
        code(TestApp::json(r, StatusCode::PAYLOAD_TOO_LARGE).await),
        "payload_too_large"
    );

    // Unauthorized repository and nonexistent repository are indistinguishable.
    let (_, member) = app
        .member(
            "m",
            &[(api, Permission::Read)],
            &[Scope::SessionsWrite, Scope::ContextRead],
        )
        .await;
    let denied = TestApp::json(
        app.ingest(
            &member,
            "k-12345683",
            &session_body("acme/secret", None, None),
        )
        .await,
        StatusCode::FORBIDDEN,
    )
    .await;
    let missing = TestApp::json(
        app.ingest(
            &member,
            "k-12345684",
            &session_body("acme/nope", None, None),
        )
        .await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(denied["error"]["code"], "repository_access_denied");
    assert_eq!(denied["error"]["message"], missing["error"]["message"]);
    // Read permission is not enough to ingest.
    assert_eq!(
        code(
            TestApp::json(
                app.ingest(&member, "k-12345685", &body).await,
                StatusCode::FORBIDDEN
            )
            .await
        ),
        "repository_access_denied"
    );

    // Missing scope.
    let (_, reader) = app
        .member("r", &[(api, Permission::Write)], &[Scope::ContextRead])
        .await;
    assert_eq!(
        code(
            TestApp::json(
                app.ingest(&reader, "k-12345686", &body).await,
                StatusCode::FORBIDDEN
            )
            .await
        ),
        "forbidden"
    );

    // Nothing was stored by any rejected request.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);

    // Revoked tokens stop working immediately.
    let tid: Uuid = sqlx::query_scalar("SELECT id FROM api_tokens WHERE name = 'r'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        app.http
            .delete(app.url(&format!("/v1/admin/tokens/{tid}")))
            .bearer_auth(&app.admin_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get("/v1/repositories", &reader).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a03_browser_session_requires_csrf_and_same_origin(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let bad = app
        .http
        .post(app.url("/v1/auth/login"))
        .json(&json!({ "username": "admin", "password": "wrong-password-123" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    let login = app
        .http
        .post(app.url("/v1/auth/login"))
        .json(&json!({ "username": "admin", "password": ADMIN_PASSWORD }))
        .send()
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"),
        "{cookie}"
    );
    let info = TestApp::json(login, StatusCode::OK).await;
    let csrf = info["csrf_token"].as_str().unwrap().to_string();

    // Safe requests work with the cookie alone.
    assert_eq!(
        app.http
            .get(app.url("/v1/overview"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let body = json!({ "provider": "github", "full_name": "acme/web" });
    // Mutations need the CSRF token...
    let r = app
        .http
        .post(app.url("/v1/repositories"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        TestApp::json(r, StatusCode::FORBIDDEN).await["error"]["code"],
        "csrf_failed"
    );
    // ...and a same-origin Origin header when present.
    let r = app
        .http
        .post(app.url("/v1/repositories"))
        .header("x-csrf-token", &csrf)
        .header("origin", "https://evil.example")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        TestApp::json(r, StatusCode::FORBIDDEN).await["error"]["code"],
        "csrf_failed"
    );
    let r = app
        .http
        .post(app.url("/v1/repositories"))
        .header("x-csrf-token", &csrf)
        .header("origin", "http://localhost:8080")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);

    // Logout invalidates the session.
    let r = app
        .http
        .post(app.url("/v1/auth/logout"))
        .header("x-csrf-token", &csrf)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.http
            .get(app.url("/v1/overview"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a03_members_cannot_use_admin_endpoints(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let api = app.create_repo("acme/api").await;
    let (_, member) = app
        .member(
            "m",
            &[(api, Permission::Write)],
            &[Scope::ContextRead, Scope::SessionsWrite],
        )
        .await;
    for path in [
        "/v1/admin/tokens",
        "/v1/admin/settings",
        "/v1/admin/audit",
        "/v1/admin/export",
        "/v1/admin/jobs",
    ] {
        let r = app.get(path, &member).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path}");
    }
    let r = app
        .post(
            "/v1/admin/tokens",
            &member,
            &json!({ "principal_id": Uuid::nil(), "name": "x", "scopes": ["admin"] }),
        )
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}
