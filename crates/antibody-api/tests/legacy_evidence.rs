//! Upgrade preserves historical observations while qualifying invalid linkage.

mod common;

use antibody_core::model::{EvidenceKind, EvidenceResult};
use antibody_db::evidence::{self, NewEvidence};
use common::*;
use serde_json::json;
use uuid::Uuid;

const REV: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn legacy_dirty_verification_requires_review_but_manual_and_valid_fixes_remain(
    pool: sqlx::PgPool,
) {
    let app = TestApp::spawn(pool).await;
    app.create_repo("acme/api").await;
    let mut ids = Vec::new();
    for n in 0..3 {
        let mut body = session_body("acme/api", Some(REV), None);
        body["hints"] = json!({
            "title": format!("Legacy problem {n}"),
            "error_signatures": [format!("error: legacy_problem_{n}")],
        });
        let (_, found) = app
            .ingest_and_extract(&app.admin_token, &format!("legacy-session-{n}"), &body)
            .await;
        ids.push(found[0]);
    }
    app.github_event(
        "check_run",
        "legacy-check",
        &check_run("acme/api", 80, "test", "success", REV, "ok"),
    )
    .await;
    app.drain().await;
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    // Simulate the identifiable old dirty-session bug. The former extractor
    // already cleared fix_revision, but its fallback lookup still verified it.
    sqlx::query("UPDATE solutions SET fix_revision = NULL WHERE id = ANY($1)")
        .bind(&ids[..2])
        .execute(&app.pool)
        .await
        .unwrap();
    let mut tx = app.pool.begin().await.unwrap();
    evidence::insert(
        &mut tx,
        NewEvidence {
            org_id: app.org_id,
            solution_id: ids[1],
            kind: EvidenceKind::ManualVerification,
            result: EvidenceResult::Pass,
            trusted: true,
            tested_revision: None,
            check_name: Some("owner review".into()),
            environment: None,
            source_artifact_id: None,
            detail: None,
            dedupe_key: "manual-review".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    sqlx::raw_sql(include_str!(
        "../../antibody-db/migrations/0003_worker_evidence.sql"
    ))
    .execute(&app.pool)
    .await
    .unwrap();
    let flags: Vec<(Uuid, bool, Option<String>)> = sqlx::query_as(
        "SELECT id, needs_revalidation, revalidation_reason FROM solutions WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&app.pool)
    .await
    .unwrap();
    let dirty = flags.iter().find(|r| r.0 == ids[0]).unwrap();
    assert!(dirty.1);
    assert!(dirty.2.as_deref().unwrap().contains("legacy CI evidence"));
    assert!(!flags.iter().find(|r| r.0 == ids[1]).unwrap().1);
    assert!(!flags.iter().find(|r| r.0 == ids[2]).unwrap().1);
    let summary = evidence::trusted_summary(&app.pool, &ids).await.unwrap();
    assert_eq!(summary.iter().find(|r| r.0 == ids[0]).unwrap().1, 0);
    assert_eq!(summary.iter().find(|r| r.0 == ids[1]).unwrap().1, 1);
    assert_eq!(summary.iter().find(|r| r.0 == ids[2]).unwrap().1, 1);
    let historical: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM evidence WHERE solution_id = $1 AND kind = 'ci_check' AND trusted",
    )
    .bind(ids[0])
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(historical, 1, "signed historical evidence remains intact");
}
