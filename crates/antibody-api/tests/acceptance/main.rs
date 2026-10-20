//! Acceptance tests mapped to spec section 13 (IDs in test names).
//! Requires PostgreSQL with pgvector; `DATABASE_URL` must point at a
//! database the test user can use to create per-test databases, e.g.
//! `postgres://antibody:antibody@localhost:55432/antibody`.

#[path = "../common/mod.rs"]
mod common;

use common::*;
use uuid::Uuid;

const REV: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";

mod admin;
mod ci_and_git;
mod hooks;
mod ingestion;
mod knowledge;
mod outcomes;
mod providers;
mod request_safety;
mod retention;
mod retrieval;

// Shared fixture: a solution verified by a successful check run.

async fn verified_fix(app: &TestApp) -> Uuid {
    let (_, sols) = app
        .ingest_and_extract(
            &app.admin_token,
            "key-verified1",
            &session_body("acme/api", Some(REV), Some("fix/nullable")),
        )
        .await;
    app.github_event(
        "check_run",
        "v-1",
        &check_run("acme/api", 1, "test", "success", REV, "ok"),
    )
    .await;
    app.drain().await;
    assert_eq!(
        app.solution(sols[0]).await["verification"]["state"],
        "verified"
    );
    sols[0]
}
