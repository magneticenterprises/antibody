//! A13: retention and deletion.

use super::verified_fix;
use crate::common::*;
use antibody_core::model::{Permission, Scope};
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// A13: retention and deletion remove raw and derived data

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a13_retention_and_deletion(pool: sqlx::PgPool) {
    let app = TestApp::spawn_with(
        pool,
        antibody_providers::Providers {
            embedding: Some(std::sync::Arc::new(
                antibody_providers::hash::HashEmbedding::new(64),
            )),
            llm: None,
        },
        |_| {},
    )
    .await;
    app.create_repo("acme/api").await;
    // Raw capture must be explicitly enabled.
    let r = app
        .http
        .patch(app.url("/v1/admin/settings"))
        .bearer_auth(&app.admin_token)
        .json(&json!({ "raw_capture_enabled": true, "raw_retention_days": 30 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let (raw_session, raw_sols) = app
        .ingest_and_extract(
            &app.admin_token,
            "key-ret-00001",
            &session_body("acme/api", None, None),
        )
        .await;
    let status = TestApp::json(
        app.get(&format!("/v1/sessions/{raw_session}"), &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["capture_mode"], "raw");
    assert_eq!(status["raw_payload_available"], true);
    assert!(status["retention_expires_at"].is_string());
    // Transcript access is a separate, audited, admin-only action.
    let t = TestApp::json(
        app.get(
            &format!("/v1/sessions/{raw_session}/transcript"),
            &app.admin_token,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert!(t["messages"].as_array().unwrap().len() == 4);
    let audit = TestApp::json(
        app.get(
            "/v1/admin/audit?action=session.transcript_viewed",
            &app.admin_token,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(audit.as_array().unwrap().len(), 1);

    // Retention expiry purges the raw payload.
    sqlx::query("UPDATE session_payloads SET purge_after = now() - interval '1 minute'")
        .execute(&app.pool)
        .await
        .unwrap();
    let sweep = TestApp::json(
        app.post("/v1/admin/retention/sweep", &app.admin_token, &json!({}))
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(sweep["payloads_purged"], 1);
    assert_eq!(
        app.get(
            &format!("/v1/sessions/{raw_session}/transcript"),
            &app.admin_token
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    // Shortening retention applies to existing raw payloads too.
    let (s2, _) = app
        .ingest_and_extract(
            &app.admin_token,
            "key-ret-00002",
            &session_body("acme/api", None, None),
        )
        .await;
    sqlx::query(
        "UPDATE session_payloads SET created_at = now() - interval '3 days' WHERE session_id = $1",
    )
    .bind(s2)
    .execute(&app.pool)
    .await
    .unwrap();
    app.http
        .patch(app.url("/v1/admin/settings"))
        .bearer_auth(&app.admin_token)
        .json(&json!({ "raw_retention_days": 1 }))
        .send()
        .await
        .unwrap();
    let sweep = TestApp::json(
        app.post("/v1/admin/retention/sweep", &app.admin_token, &json!({}))
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(sweep["payloads_purged"], 1);

    // s2 corroborated the raw session's solution: deleting the raw session
    // detaches but keeps it; deleting both removes it with embeddings/evidence.
    let emb: i64 = sqlx::query_scalar("SELECT count(*) FROM embeddings")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(emb, 1, "hash provider embedded the solution");
    let del = TestApp::json(
        app.http
            .delete(app.url(&format!("/v1/sessions/{raw_session}")))
            .bearer_auth(&app.admin_token)
            .send()
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        (
            del["solutions_deleted"].as_u64(),
            del["solutions_detached"].as_u64()
        ),
        (Some(0), Some(1))
    );
    let del2 = TestApp::json(
        app.http
            .delete(app.url(&format!("/v1/sessions/{s2}")))
            .bearer_auth(&app.admin_token)
            .send()
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(del2["solutions_deleted"], 1);
    for table in [
        "solutions",
        "embeddings",
        "evidence",
        "solution_sources",
        "session_payloads",
        "sessions",
    ] {
        let n: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                .fetch_one(&app.pool)
                .await
                .unwrap();
        assert_eq!(n, 0, "{table} should be empty after deletion");
    }
    assert_eq!(
        app.get(&format!("/v1/solutions/{}", raw_sols[0]), &app.admin_token)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // Deleting a session with a pending job cancels the job.
    let v = TestApp::json(
        app.ingest(
            &app.admin_token,
            "key-ret-00003",
            &session_body("acme/api", None, None),
        )
        .await,
        StatusCode::ACCEPTED,
    )
    .await;
    let pending: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    app.http
        .delete(app.url(&format!("/v1/sessions/{pending}")))
        .bearer_auth(&app.admin_token)
        .send()
        .await
        .unwrap();
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE state IN ('queued','running') AND kind = 'extract_session'").fetch_one(&app.pool).await.unwrap();
    assert_eq!(live, 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a13_admin_can_hard_delete_a_solution(pool: sqlx::PgPool) {
    let app = TestApp::spawn_with(
        pool,
        antibody_providers::Providers {
            embedding: Some(std::sync::Arc::new(
                antibody_providers::hash::HashEmbedding::new(32),
            )),
            llm: None,
        },
        |_| {},
    )
    .await;
    let api = app.create_repo("acme/api").await;
    let id = verified_fix(&app).await;
    let (_, member) = app
        .member("m", &[(api, Permission::Write)], &[Scope::ContextRead])
        .await;
    let url = app.url(&format!("/v1/solutions/{id}"));
    assert_eq!(
        app.http
            .delete(&url)
            .bearer_auth(&member)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.http
            .delete(&url)
            .bearer_auth(&app.admin_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.http
            .delete(&url)
            .bearer_auth(&app.admin_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    for table in [
        "solutions",
        "embeddings",
        "evidence",
        "solution_sources",
        "solution_sessions",
    ] {
        let n: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                .fetch_one(&app.pool)
                .await
                .unwrap();
        assert_eq!(n, 0, "{table}");
    }
    let s = app
        .search(
            &app.admin_token,
            &json!({ "task": "x", "error": "error[E0308]: mismatched types" }),
        )
        .await;
    assert_eq!(s["suggestions"].as_array().unwrap().len(), 0);
    let audit = TestApp::json(
        app.get("/v1/admin/audit?action=solution.deleted", &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(audit.as_array().unwrap().len(), 1);
}
