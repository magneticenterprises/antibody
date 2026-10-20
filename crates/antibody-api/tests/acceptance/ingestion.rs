//! A02: durable, idempotent ingestion.

use super::REV;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// A02: durable, idempotent ingestion

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a02_ingestion_is_idempotent(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let body = session_body("acme/api", Some(REV), Some("fix/nullable"));

    let first = TestApp::json(
        app.ingest(&app.admin_token, "key-00000001", &body).await,
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(first["duplicate"], false);
    let retry = TestApp::json(
        app.ingest(&app.admin_token, "key-00000001", &body).await,
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(
        retry["id"], first["id"],
        "identical retry returns the same ingestion id"
    );
    assert_eq!(retry["duplicate"], true);

    let mut changed = body.clone();
    changed["external_id"] = json!("other");
    let conflict = TestApp::json(
        app.ingest(&app.admin_token, "key-00000001", &changed).await,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(conflict["error"]["code"], "idempotency_conflict");

    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'extract_session'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        (sessions, jobs),
        (1, 1),
        "exactly one session and one extraction job"
    );
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a02_worker_crash_is_recovered_by_lease_expiry(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let v = TestApp::json(
        app.ingest(
            &app.admin_token,
            "key-crash-001",
            &session_body("acme/api", Some(REV), None),
        )
        .await,
        StatusCode::ACCEPTED,
    )
    .await;
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // A worker claims the job and "crashes" (never completes or fails it).
    let claimed = antibody_db::jobs::claim(
        &app.pool,
        "crashed-worker",
        10,
        chrono::Duration::milliseconds(300),
    )
    .await
    .unwrap();
    assert_eq!(claimed.len(), 1);
    // While the lease is live, nobody else can take it.
    assert!(
        antibody_db::jobs::claim(&app.pool, "other", 10, chrono::Duration::seconds(30))
            .await
            .unwrap()
            .is_empty()
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // After expiry another worker picks it up and finishes it (plus the
    // embedding job it enqueues).
    assert!(app.drain().await >= 1);
    let status = TestApp::json(
        app.get(&format!("/v1/sessions/{id}"), &app.admin_token)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["extraction_status"], "succeeded");
    assert_eq!(status["ingestion_status"], "accepted");
    let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM jobs WHERE session_id = $1")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(attempts, 2);

    // Re-running extraction is idempotent: no duplicate solutions.
    let n_before: i64 = sqlx::query_scalar("SELECT count(*) FROM solutions")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET extraction_status = 'pending' WHERE id = $1")
        .bind(id)
        .execute(&app.pool)
        .await
        .unwrap();
    antibody_db::jobs::enqueue(
        &app.pool,
        antibody_db::jobs::NewJob::new(
            antibody_core::model::JobKind::ExtractSession,
            "rerun",
            json!({ "session_id": id }),
        ),
    )
    .await
    .unwrap();
    // The structured-only payload is gone after the first extraction, so the
    // rerun records the session as failed rather than inventing knowledge.
    app.drain().await;
    let n_after: i64 = sqlx::query_scalar("SELECT count(*) FROM solutions")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(n_before, n_after);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a02_failures_retry_with_backoff_then_dead_letter(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    // A job whose payload can never succeed is dead-lettered, not retried forever.
    antibody_db::jobs::enqueue(
        &app.pool,
        antibody_db::jobs::NewJob::new(
            antibody_core::model::JobKind::ExtractSession,
            "bad",
            json!({ "nope": 1 }),
        )
        .org(app.org_id),
    )
    .await
    .unwrap();
    app.drain().await;
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE dedupe_key = 'bad'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(state, "dead");

    // Transient failures are requeued with a future run_at.
    let ctx = app.worker();
    let id = antibody_db::jobs::enqueue(
        &app.pool,
        antibody_db::jobs::NewJob::new(
            antibody_core::model::JobKind::RetentionSweep,
            "t",
            json!({}),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let job = antibody_db::jobs::claim(
        &app.pool,
        &ctx.1.worker_id,
        1,
        chrono::Duration::seconds(30),
    )
    .await
    .unwrap()
    .remove(0);
    let out = antibody_db::jobs::fail(
        &app.pool,
        job.id,
        &ctx.1.worker_id,
        "boom",
        false,
        chrono::Duration::seconds(60),
    )
    .await
    .unwrap();
    assert_eq!(out, antibody_db::jobs::FailOutcome::Retrying);
    let (st, future): (String, bool) =
        sqlx::query_as("SELECT state, run_at > now() FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!((st.as_str(), future), ("queued", true));
}
