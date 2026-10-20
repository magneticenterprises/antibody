use antibody_core::applicability::Applicability;
use antibody_core::model::JobKind;
use antibody_db::jobs::{self, NewJob};
use antibody_db::solutions::{self, ExtractedSolution, UpsertOutcome};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

pub struct Fixture {
    pub org: Uuid,
    pub repo: Uuid,
}

impl Fixture {
    pub async fn new(pool: &PgPool) -> Self {
        let org = antibody_db::orgs::create(pool, "integrity", "Integrity")
            .await
            .unwrap()
            .id;
        let repo = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO repositories (id, organization_id, provider, full_name)
             VALUES ($1, $2, 'generic', 'test/integrity')",
        )
        .bind(repo)
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
        Self { org, repo }
    }

    pub async fn session(&self, pool: &PgPool) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO sessions (id, organization_id, repository_id, agent_type,
                                   idempotency_key, request_hash, capture_mode, message_count, payload_bytes)
             VALUES ($1, $2, $3, 'other', $4, 'synthetic'::bytea, 'raw', 1, 2)",
        )
        .bind(id)
        .bind(self.org)
        .bind(self.repo)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO session_payloads (session_id, organization_id, payload, purge_after)
             VALUES ($1, $2, '{}'::jsonb, now() + interval '30 days')",
        )
        .bind(id)
        .bind(self.org)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO source_artifacts (id, organization_id, repository_id, kind, external_id)
             VALUES ($1, $2, $3, 'session', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(self.org)
        .bind(self.repo)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
        jobs::enqueue(
            pool,
            NewJob::new(
                JobKind::ExtractSession,
                format!("session:{id}"),
                serde_json::json!({ "session_id": id }),
            )
            .org(self.org)
            .session(id),
        )
        .await
        .unwrap();
        id
    }

    pub fn extracted(&self, session: Uuid, platform: &str) -> ExtractedSolution {
        ExtractedSolution {
            org_id: self.org,
            repo_id: self.repo,
            component_id: None,
            session_id: session,
            owner_principal_id: None,
            title: "Restore connection after timeout".into(),
            symptom: "Connection timed out".into(),
            root_cause: None,
            remedy: "Reconnect before retrying".into(),
            error_signatures: vec!["E_CONNECTION_TIMEOUT".into()],
            symbols: vec![],
            paths: vec![],
            applicability: Applicability {
                platforms: vec![platform.into()],
                ..Applicability::default()
            },
            fix_revision: None,
            fix_branch: None,
            confidence: 0.5,
            extractor: "test".into(),
        }
    }
}

pub async fn upsert(pool: &PgPool, extracted: &ExtractedSolution) -> UpsertOutcome {
    let mut tx = pool.begin().await.unwrap();
    let outcome = solutions::upsert_extracted(&mut tx, extracted)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    outcome
}

pub async fn wait_for_blocked(pool: &PgPool, count: i64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname = current_database() AND cardinality(pg_blocking_pids(pid)) > 0",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if blocked >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("concurrent transactions did not reach the controlled lock barrier");
}

pub async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}
