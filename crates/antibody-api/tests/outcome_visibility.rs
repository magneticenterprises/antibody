#[path = "common/mod.rs"]
mod common;

use antibody_core::model::{Permission, Scope};
use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

fn outcome_evidence(detail: &Value) -> Vec<&Value> {
    detail["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["detail"]
                .as_str()
                .is_some_and(|s| s.starts_with("agent reported outcome:"))
        })
        .collect()
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn solution_evidence_requires_consumer_visibility_for_outcomes(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let source = app.create_repo("acme/source").await;
    let consumer = app.create_repo("acme/consumer").await;
    let (_, solutions) = app
        .ingest_and_extract(
            &app.admin_token,
            "outcome-visibility-fixture",
            &session_body("acme/source", None, None),
        )
        .await;
    let solution = solutions[0];
    let path = format!("/v1/solutions/{solution}");
    let baseline = app.solution(solution).await;
    let ordinary_count = baseline["evidence"].as_array().unwrap().len();
    assert!(ordinary_count > 0);
    assert!(outcome_evidence(&baseline).is_empty());
    let (_, source_only) = app
        .member(
            "source-reader",
            &[(source, Permission::Read)],
            &[Scope::ContextRead],
        )
        .await;
    let (_, both) = app
        .member(
            "both-reader",
            &[(source, Permission::Read), (consumer, Permission::Read)],
            &[Scope::ContextRead],
        )
        .await;

    for (repository, outcome, with_check) in [
        (Some("acme/consumer"), "used", true),
        (Some("acme/consumer"), "rejected", false),
        (None, "inapplicable", false),
    ] {
        let found = app
            .search(
                &app.admin_token,
                &json!({"task": "error[E0308]: mismatched types", "repository": repository}),
            )
            .await;
        let mut report = json!({
            "retrieval_id": found["retrieval_id"], "solution_id": solution, "outcome": outcome,
        });
        if with_check {
            report["check"] = json!({
                "name": "synthetic private consumer check",
                "revision": "11223344556677889900aabbccddeeff11223344",
                "result": "pass",
            });
        }
        let recorded = TestApp::json(
            app.post("/v1/outcomes", &app.admin_token, &report).await,
            StatusCode::OK,
        )
        .await;
        if with_check {
            // Previously stored outcomes used the source repository. Reads
            // must still derive consumer visibility from the retrieval.
            let id: Uuid = recorded["id"].as_str().unwrap().parse().unwrap();
            sqlx::query("UPDATE outcomes SET repository_id = $2 WHERE id = $1")
                .bind(id)
                .bind(source)
                .execute(&app.pool)
                .await
                .unwrap();
        }
    }

    let app_ref = &app;
    let narrow = |ids: Vec<Uuid>| async move {
        antibody_db::tokens::issue_token(
            &app_ref.pool,
            antibody_db::tokens::NewToken {
                org_id: app_ref.org_id,
                principal_id: app_ref.admin_principal,
                name: "narrow-evidence-admin",
                scopes: &[Scope::ContextRead, Scope::Admin],
                repository_ids: Some(&ids),
                expires_at: None,
                created_by: None,
            },
        )
        .await
        .unwrap()
        .token
    };
    let narrow_source = narrow(vec![source]).await;
    let narrow_both = narrow(vec![source, consumer]).await;
    for (token, expected_outcomes) in [
        (&source_only, 0),
        (&narrow_source, 0),
        (&both, 2),
        (&narrow_both, 2),
        (&app.admin_token, 3),
    ] {
        let detail = TestApp::json(app.get(&path, token).await, StatusCode::OK).await;
        assert_eq!(outcome_evidence(&detail).len(), expected_outcomes);
        assert_eq!(
            detail["evidence"].as_array().unwrap().len(),
            ordinary_count + expected_outcomes,
            "ordinary source evidence remains visible"
        );
        if expected_outcomes == 0 {
            assert!(
                !detail
                    .to_string()
                    .contains("synthetic private consumer check")
            );
            assert!(!detail.to_string().contains("agent reported outcome:"));
        }

        let mcp = TestApp::json(
            app.http
                .post(app.url("/mcp"))
                .bearer_auth(token)
                .header("accept", "application/json, text/event-stream")
                .header("mcp-protocol-version", "2025-06-18")
                .json(&json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": "get_solution", "arguments": {"solution_id": solution}},
                }))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            outcome_evidence(&mcp["result"]["structuredContent"]).len(),
            expected_outcomes,
            "MCP uses the same outcome visibility"
        );
    }
}
