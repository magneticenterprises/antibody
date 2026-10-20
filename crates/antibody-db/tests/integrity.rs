//! Real PostgreSQL regressions for retention, deletion, and concurrent versions.
#[path = "integrity/support.rs"]
mod support;

use antibody_db::{retention, sessions, solutions};
use sqlx::PgPool;
use std::time::Duration;
use support::{Fixture, count, upsert, wait_for_blocked};
use uuid::Uuid;

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn concurrent_last_supporting_session_deletions_remove_derived_content(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let first = f.session(&pool).await;
    let second = f.session(&pool).await;
    let solution = upsert(&pool, &f.extracted(first, "linux")).await.id();
    assert_eq!(
        upsert(&pool, &f.extracted(second, "linux")).await.id(),
        solution
    );
    sqlx::query(
        "INSERT INTO embeddings (id, organization_id, solution_id, content_hash, provider, model, dimensions, embedding)
         VALUES ($1, $2, $3, 'test-hash', 'test', 'test', 2, '[1,0]'::vector)",
    )
    .bind(Uuid::now_v7())
    .bind(f.org)
    .bind(solution)
    .execute(&pool)
    .await
    .unwrap();
    antibody_db::jobs::enqueue(
        &pool,
        antibody_db::jobs::NewJob::new(
            antibody_core::model::JobKind::EmbedSolution,
            format!("solution:{solution}"),
            serde_json::json!({ "solution_id": solution }),
        )
        .org(f.org),
    )
    .await
    .unwrap();

    // Both deletes reach cancellation while a separate transaction holds the
    // extraction jobs. Without repository serialization both read "supported
    // elsewhere" before either deletes its session.
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM jobs WHERE kind = 'extract_session' FOR UPDATE")
        .fetch_all(&mut *barrier)
        .await
        .unwrap();
    let p1 = pool.clone();
    let p2 = pool.clone();
    let a = tokio::spawn(async move { sessions::delete_session(&p1, f.org, first).await });
    let b = tokio::spawn(async move { sessions::delete_session(&p2, f.org, second).await });
    wait_for_blocked(&pool, 2).await;
    barrier.commit().await.unwrap();
    let (a, b) = tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(a, b) })
        .await
        .unwrap();
    let a = a.unwrap().unwrap().unwrap();
    let b = b.unwrap().unwrap().unwrap();
    assert_eq!(a.sessions_deleted + b.sessions_deleted, 2);
    assert_eq!(a.solutions_deleted + b.solutions_deleted, 1);
    assert_eq!(a.solutions_detached + b.solutions_detached, 1);
    for table in [
        "sessions",
        "session_payloads",
        "solutions",
        "solution_sessions",
        "solution_sources",
        "source_artifacts",
        "evidence",
        "embeddings",
    ] {
        assert_eq!(count(&pool, table).await, 0, "{table}");
    }
    let live_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE state IN ('queued', 'running', 'dead')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live_jobs, 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn concurrent_variants_allocate_unique_lineage_versions(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let first = f.session(&pool).await;
    let second = f.session(&pool).await;
    let x = f.extracted(first, "linux");
    let y = f.extracted(second, "macos");
    // Block INSERT after allocation. Previously both transactions selected
    // version 1 while neither could insert.
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE solutions IN SHARE MODE")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let p1 = pool.clone();
    let p2 = pool.clone();
    let a = tokio::spawn(async move { upsert(&p1, &x).await });
    let b = tokio::spawn(async move { upsert(&p2, &y).await });
    wait_for_blocked(&pool, 2).await;
    barrier.commit().await.unwrap();
    let (a, b) = tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(a, b) })
        .await
        .unwrap();
    assert_ne!(a.unwrap().id(), b.unwrap().id());
    let versions: Vec<i32> = sqlx::query_scalar("SELECT version FROM solutions ORDER BY version")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(versions, vec![1, 2]);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn concurrent_identical_fingerprints_corroborate_without_unique_errors(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let first = f.session(&pool).await;
    let second = f.session(&pool).await;
    let x = f.extracted(first, "linux");
    let y = f.extracted(second, "linux");
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE solutions IN SHARE MODE")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let p1 = pool.clone();
    let p2 = pool.clone();
    let a = tokio::spawn(async move { upsert(&p1, &x).await });
    let b = tokio::spawn(async move { upsert(&p2, &y).await });
    wait_for_blocked(&pool, 2).await;
    barrier.commit().await.unwrap();
    let (a, b) = tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(a, b) })
        .await
        .unwrap();
    assert_eq!(a.unwrap().id(), b.unwrap().id());
    assert_eq!(count(&pool, "solutions").await, 1);
    assert_eq!(count(&pool, "solution_sessions").await, 2);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn deletion_and_new_corroboration_preserve_the_remaining_session(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let first = f.session(&pool).await;
    let second = f.session(&pool).await;
    let old_solution = upsert(&pool, &f.extracted(first, "linux")).await.id();
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM jobs WHERE session_id = $1 FOR UPDATE")
        .bind(first)
        .fetch_all(&mut *barrier)
        .await
        .unwrap();
    let p1 = pool.clone();
    let deleting = tokio::spawn(async move { sessions::delete_session(&p1, f.org, first).await });
    wait_for_blocked(&pool, 1).await;
    let p2 = pool.clone();
    let x = f.extracted(second, "linux");
    let extracting = tokio::spawn(async move { upsert(&p2, &x).await });
    wait_for_blocked(&pool, 2).await;
    barrier.commit().await.unwrap();
    let (deleted, extracted) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(deleting, extracting)
    })
    .await
    .unwrap();
    assert_eq!(deleted.unwrap().unwrap().unwrap().solutions_deleted, 1);
    let new_solution = extracted.unwrap().id();
    assert_ne!(new_solution, old_solution);
    let supporting: Vec<(Uuid, Uuid)> =
        sqlx::query_as("SELECT solution_id, session_id FROM solution_sessions")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(supporting, vec![(new_solution, second)]);
    assert_eq!(count(&pool, "solutions").await, 1);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn extraction_commit_and_deletion_use_the_same_lock_order(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let session = f.session(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    solutions::upsert_extracted(&mut tx, &f.extracted(session, "linux"))
        .await
        .unwrap();
    let p = pool.clone();
    let deleting = tokio::spawn(async move { sessions::delete_session(&p, f.org, session).await });
    wait_for_blocked(&pool, 1).await;
    tx.commit().await.unwrap();
    let report = tokio::time::timeout(Duration::from_secs(5), deleting)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(report.solutions_deleted, 1);
    assert_eq!(count(&pool, "solutions").await, 0);
    let mut tx = pool.begin().await.unwrap();
    assert!(
        !sessions::lock_for_extraction(&mut tx, f.org, f.repo, session)
            .await
            .unwrap()
    );
    assert!(
        solutions::upsert_extracted(&mut tx, &f.extracted(session, "linux"))
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    assert_eq!(count(&pool, "solutions").await, 0);
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn hard_solution_deletion_and_extraction_do_not_invert_job_locks(pool: PgPool) {
    use antibody_core::model::JobKind;
    use antibody_db::jobs::{self, NewJob};

    let f = Fixture::new(&pool).await;
    let first = f.session(&pool).await;
    let second = f.session(&pool).await;
    let solution = upsert(&pool, &f.extracted(first, "linux")).await.id();
    let embedding_job = NewJob::new(
        JobKind::EmbedSolution,
        format!("solution:{solution}"),
        serde_json::json!({ "solution_id": solution }),
    )
    .org(f.org);
    jobs::enqueue(&pool, embedding_job.clone()).await.unwrap();
    let mut extracting = pool.begin().await.unwrap();
    solutions::upsert_extracted(&mut extracting, &f.extracted(second, "linux"))
        .await
        .unwrap();
    let p = pool.clone();
    let deleting = tokio::spawn(async move { solutions::delete(&p, f.org, solution).await });
    wait_for_blocked(&pool, 1).await;

    // Extraction holds the solution and then enqueues its embedding. Hard
    // deletion must wait before taking a job lock, otherwise these two
    // transactions wait for each other's solution/job locks.
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            jobs::enqueue(&mut *extracting, embedding_job),
        )
        .await
        .unwrap()
        .unwrap()
        .is_none()
    );
    extracting.commit().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), deleting)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    );
    assert_eq!(count(&pool, "solutions").await, 0);
    assert_eq!(count(&pool, "solution_sessions").await, 0);
    let job_state: String =
        sqlx::query_scalar("SELECT state FROM jobs WHERE kind = 'embed_solution'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(job_state, "cancelled");
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn shortened_raw_retention_applies_to_every_extraction_status(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    sqlx::query("UPDATE organizations SET raw_retention_days = 1 WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await
        .unwrap();
    for status in ["pending", "running", "succeeded", "failed", "skipped"] {
        let id = f.session(&pool).await;
        sqlx::query("UPDATE sessions SET extraction_status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
    }
    let structured = f.session(&pool).await;
    sqlx::query("UPDATE sessions SET capture_mode = 'structured_only' WHERE id = $1")
        .bind(structured)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE session_payloads SET created_at = now() - interval '3 days'")
        .execute(&pool)
        .await
        .unwrap();
    let fresh = f.session(&pool).await;
    let report = retention::sweep(&pool).await.unwrap();
    assert_eq!(report.payloads_purged, 5);
    let mut remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT session_id FROM session_payloads ORDER BY session_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    remaining.sort();
    let mut expected = vec![structured, fresh];
    expected.sort();
    assert_eq!(remaining, expected);
    assert_eq!(count(&pool, "sessions").await, 7);
}

#[sqlx::test(migrations = false)]
async fn migration_repairs_only_duplicate_versions_before_enforcing_uniqueness(pool: PgPool) {
    sqlx::raw_sql(include_str!("../migrations/0001_init.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let f = Fixture::new(&pool).await;
    let session = f.session(&pool).await;
    let a = upsert(&pool, &f.extracted(session, "linux")).await.id();
    let b = upsert(&pool, &f.extracted(session, "macos")).await.id();
    let c = upsert(&pool, &f.extracted(session, "windows")).await.id();
    let corroborating_session = f.session(&pool).await;
    upsert(&pool, &f.extracted(corroborating_session, "linux")).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE solutions SET version = 1 WHERE id = $1")
        .bind(b)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0002_data_integrity.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for (id, expected) in [(a, 1), (b, 4), (c, 3)] {
        let version: i32 = sqlx::query_scalar("SELECT version FROM solutions WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(version, expected);
    }
    assert_eq!(count(&pool, "solution_sessions").await, 4);
    assert_eq!(count(&pool, "solution_sources").await, 4);
    assert_eq!(count(&pool, "evidence").await, 1);
    let error = sqlx::query("UPDATE solutions SET version = 1 WHERE id = $1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(error.as_database_error().unwrap().is_unique_violation());
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn a_later_worker_failure_cannot_downgrade_completed_extraction(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    for status in ["succeeded", "skipped"] {
        let session = f.session(&pool).await;
        sqlx::query("UPDATE sessions SET extraction_status = $2 WHERE id = $1")
            .bind(session)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
        sessions::mark_extraction_failed(&pool, session, "correlation failed", true)
            .await
            .unwrap();
        sessions::mark_extraction_running(&pool, session)
            .await
            .unwrap();
        let actual: String =
            sqlx::query_scalar("SELECT extraction_status FROM sessions WHERE id = $1")
                .bind(session)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(actual, status);
    }
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn legacy_applicability_fingerprints_upgrade_without_duplicate_solutions(pool: PgPool) {
    use antibody_core::normalize;

    let f = Fixture::new(&pool).await;
    let first = f.session(&pool).await;
    let second = f.session(&pool).await;
    let mut x = f.extracted(first, "linux");
    x.applicability.components = vec!["services/api/**".into()];
    let id = upsert(&pool, &x).await.id();
    let lineage = normalize::lineage_key(&x.error_signatures, &x.title);
    let legacy = normalize::solution_fingerprint(&lineage, "deps=;platforms=linux");
    sqlx::query("UPDATE solutions SET fingerprint = $2 WHERE id = $1")
        .bind(id)
        .bind(legacy)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        upsert(&pool, &x).await,
        solutions::UpsertOutcome::Unchanged { id }
    );
    x.session_id = second;
    assert_eq!(
        upsert(&pool, &x).await,
        solutions::UpsertOutcome::Corroborated { id }
    );
    let fingerprint: String = sqlx::query_scalar("SELECT fingerprint FROM solutions WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        fingerprint,
        normalize::solution_fingerprint(&lineage, &x.applicability.key())
    );
    assert_eq!(count(&pool, "solutions").await, 1);
    assert_eq!(count(&pool, "solution_sessions").await, 2);
    assert_eq!(count(&pool, "evidence").await, 1);

    x.applicability.components = vec!["services/web/**".into()];
    assert!(matches!(
        upsert(&pool, &x).await,
        solutions::UpsertOutcome::Created { version: 2, .. }
    ));
    assert_eq!(count(&pool, "solutions").await, 2);
}
