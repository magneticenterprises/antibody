//! A06 (server side): native HTTP agent hooks.

use super::verified_fix;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;

// ---------------------------------------------------------------------------
// A06 (server side): the native HTTP task-start hook returns context once per session

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a06_user_prompt_submit_hook_injects_context_once(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let id = verified_fix(&app).await;
    let hook = |sid: &str, prompt: &str| {
        app.http.post(app.url("/v1/hooks/user-prompt-submit")).bearer_auth(&app.admin_token).header("X-Antibody-Repository", "acme/api")
            .json(&json!({ "session_id": sid, "hook_event_name": "UserPromptSubmit", "cwd": "/tmp", "transcript_path": "/tmp/x.jsonl",
                "prompt": prompt, "permission_mode": "default", "some_future_field": true }))
            .send()
    };
    let out: serde_json::Value = hook(
        "s-1",
        "cargo build fails with error[E0308]: mismatched types in user.rs",
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        out["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    assert!(
        ctx.contains(&id.to_string()) && ctx.contains("untrusted"),
        "{ctx}"
    );
    assert!(ctx.len() < 10_000);
    let again: serde_json::Value = hook(
        "s-1",
        "now also fix error[E0308]: mismatched types elsewhere",
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(
        again,
        json!({}),
        "only the first prompt of a session is searched"
    );
    let short: serde_json::Value = hook("s-2", "yes").await.unwrap().json().await.unwrap();
    assert_eq!(short, json!({}));
    let unauth = app
        .http
        .post(app.url("/v1/hooks/user-prompt-submit"))
        .json(&json!({ "prompt": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
    let hook_retrievals: i64 =
        sqlx::query_scalar("SELECT count(*) FROM retrievals WHERE channel = 'hook'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(hook_retrievals, 1);
}
