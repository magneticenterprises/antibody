//! Test harness: a real API server on a random port over a fresh database
//! (created per test by `#[sqlx::test]`), plus helpers to drive the worker,
//! sign webhooks and create principals.
#![allow(dead_code)]

use antibody_api::{AppState, Config, router};
use antibody_core::model::{Permission, PrincipalKind, Role, Scope};
use antibody_db::PgPool;
use antibody_providers::Providers;
use antibody_search::{SearchConfig, SearchEngine};
use antibody_worker::{WorkerConfig, WorkerContext};
use reqwest::{Response, StatusCode};
use serde_json::{Value, json};
use std::net::SocketAddr;
use uuid::Uuid;

pub struct TestApp {
    pub base: String,
    pub pool: PgPool,
    pub http: reqwest::Client,
    pub state: AppState,
    pub providers: Providers,
    pub org_id: Uuid,
    pub org_slug: String,
    pub admin_token: String,
    pub admin_principal: Uuid,
}

pub const ADMIN_PASSWORD: &str = "correct-horse-battery-staple";

impl TestApp {
    pub async fn spawn(pool: PgPool) -> Self {
        Self::spawn_with(pool, Providers::none(), |_| {}).await
    }

    pub async fn spawn_with(
        pool: PgPool,
        providers: Providers,
        tweak: impl FnOnce(&mut Config),
    ) -> Self {
        let mut cfg = Config::for_tests("postgres://unused");
        tweak(&mut cfg);
        let boot = antibody_api::bootstrap::bootstrap(
            &pool,
            "acme",
            "Acme",
            "admin",
            Some(ADMIN_PASSWORD.into()),
        )
        .await
        .expect("bootstrap");
        let state = AppState::new(pool.clone(), cfg, providers.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(state.clone());
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let admin_principal: Uuid =
            sqlx::query_scalar("SELECT principal_id FROM users WHERE username = 'admin'")
                .fetch_one(&pool)
                .await
                .unwrap();
        Self {
            base: format!("http://{addr}"),
            pool,
            http: reqwest::Client::builder()
                .cookie_store(true)
                .build()
                .unwrap(),
            state,
            providers,
            org_id: boot.organization_id,
            org_slug: boot.organization_slug,
            admin_token: boot.admin_token,
            admin_principal,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub fn worker(&self) -> (WorkerContext, WorkerConfig) {
        let search = SearchEngine::new(
            self.pool.clone(),
            self.providers.clone(),
            SearchConfig::default(),
        );
        (
            WorkerContext {
                pool: self.pool.clone(),
                providers: self.providers.clone(),
                search,
            },
            WorkerConfig {
                worker_id: "test-worker".into(),
                ..Default::default()
            },
        )
    }

    /// Run every due job to completion.
    pub async fn drain(&self) -> usize {
        let (ctx, cfg) = self.worker();
        antibody_worker::drain(&ctx, &cfg).await.expect("drain")
    }

    pub async fn get(&self, path: &str, token: &str) -> Response {
        self.http
            .get(self.url(path))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    pub async fn post(&self, path: &str, token: &str, body: &Value) -> Response {
        self.http
            .post(self.url(path))
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .unwrap()
    }

    pub async fn json(resp: Response, expected: StatusCode) -> Value {
        let status = resp.status();
        let text = resp.text().await.unwrap();
        assert_eq!(status, expected, "unexpected status; body: {text}");
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    pub async fn create_repo(&self, full_name: &str) -> Uuid {
        let v = Self::json(
            self.post(
                "/v1/repositories",
                &self.admin_token,
                &json!({ "provider": "github", "full_name": full_name }),
            )
            .await,
            StatusCode::CREATED,
        )
        .await;
        v["id"].as_str().unwrap().parse().unwrap()
    }

    /// A member service principal with grants and a scoped token.
    pub async fn member(
        &self,
        name: &str,
        grants: &[(Uuid, Permission)],
        scopes: &[Scope],
    ) -> (Uuid, String) {
        let p = antibody_db::access::create_principal(
            &self.pool,
            self.org_id,
            PrincipalKind::Service,
            Role::Member,
            name,
        )
        .await
        .unwrap();
        for (repo, perm) in grants {
            antibody_db::access::grant_repository(&self.pool, self.org_id, p.id, *repo, *perm)
                .await
                .unwrap();
        }
        let t = antibody_db::tokens::issue_token(
            &self.pool,
            antibody_db::tokens::NewToken {
                org_id: self.org_id,
                principal_id: p.id,
                name,
                scopes,
                repository_ids: None,
                expires_at: None,
                created_by: None,
            },
        )
        .await
        .unwrap();
        (p.id, t.token)
    }

    pub async fn ingest(&self, token: &str, key: &str, body: &Value) -> Response {
        self.http
            .post(self.url("/v1/sessions"))
            .bearer_auth(token)
            .header("Idempotency-Key", key)
            .json(body)
            .send()
            .await
            .unwrap()
    }

    /// Ingest a session and run extraction; returns (session_id, solution_ids).
    pub async fn ingest_and_extract(
        &self,
        token: &str,
        key: &str,
        body: &Value,
    ) -> (Uuid, Vec<Uuid>) {
        let v = Self::json(self.ingest(token, key, body).await, StatusCode::ACCEPTED).await;
        let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
        self.drain().await;
        let status = Self::json(
            self.get(&format!("/v1/sessions/{id}"), &self.admin_token)
                .await,
            StatusCode::OK,
        )
        .await;
        let sols = status["solution_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().parse().unwrap())
            .collect();
        (id, sols)
    }

    pub async fn search(&self, token: &str, body: &Value) -> Value {
        Self::json(
            self.post("/v1/context/search", token, body).await,
            StatusCode::OK,
        )
        .await
    }

    pub async fn solution(&self, id: Uuid) -> Value {
        Self::json(
            self.get(&format!("/v1/solutions/{id}"), &self.admin_token)
                .await,
            StatusCode::OK,
        )
        .await
    }

    pub fn github_secret(&self) -> String {
        self.state.github_webhook_secret(self.org_id, 1)
    }

    pub async fn github_event(&self, event: &str, delivery: &str, payload: &Value) -> Response {
        let body = serde_json::to_vec(payload).unwrap();
        let sig = format!(
            "sha256={}",
            antibody_integrations::signing::sign(self.github_secret().as_bytes(), &body)
        );
        self.http
            .post(self.url(&format!("/v1/webhooks/github?org={}", self.org_slug)))
            .header("X-GitHub-Event", event)
            .header("X-GitHub-Delivery", delivery)
            .header("X-Hub-Signature-256", sig)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap()
    }

    pub async fn ci_event(&self, delivery: &str, payload: &Value, ts_offset: i64) -> Response {
        let body = serde_json::to_vec(payload).unwrap();
        let ts = (chrono::Utc::now().timestamp() + ts_offset).to_string();
        let secret = self.state.ci_webhook_secret(self.org_id, 1);
        let sig = antibody_integrations::ci::sign(secret.as_bytes(), &ts, &body);
        self.http
            .post(self.url("/v1/webhooks/ci"))
            .header("X-Antibody-Org", &self.org_slug)
            .header("X-Antibody-Delivery", delivery)
            .header("X-Antibody-Timestamp", ts)
            .header("X-Antibody-Signature", sig)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap()
    }
}

/// A realistic Claude Code session that fixed a Rust type error.
pub fn session_body(repo: &str, revision: Option<&str>, branch: Option<&str>) -> Value {
    json!({
        "agent_type": "claude_code",
        "external_id": "sess-1",
        "repository": repo,
        "revision": revision,
        "branch": branch,
        "messages": [
            { "role": "user", "content": "cargo build fails in the user handler after the sqlx upgrade" },
            { "role": "assistant", "content": "[tool call Bash] command: cargo build", "tool_name": "Bash" },
            { "role": "tool", "content": "error[E0308]: mismatched types\n  --> /home/alice/api/src/handlers/user.rs:42:17\n   expected `i64`, found `Option<i64>`" },
            { "role": "assistant", "content": "The root cause was that sqlx 0.8 now infers nullable columns as Option. I changed UserRow::id to use a non-null cast (`id AS \"id!\"`) in src/handlers/user.rs and the build and tests pass now." }
        ]
    })
}

pub fn pr_merged(
    repo: &str,
    number: i64,
    head_ref: &str,
    head_sha: &str,
    merge_sha: &str,
) -> Value {
    json!({
        "action": "closed",
        "number": number,
        "pull_request": {
            "merged": true, "merge_commit_sha": merge_sha, "title": "Fix nullable id",
            "html_url": format!("https://github.com/{repo}/pull/{number}"),
            "head": { "sha": head_sha, "ref": head_ref }
        },
        "repository": { "full_name": repo, "default_branch": "main" }
    })
}

pub fn check_run(
    repo: &str,
    id: i64,
    name: &str,
    conclusion: &str,
    head_sha: &str,
    summary: &str,
) -> Value {
    json!({
        "action": "completed",
        "check_run": {
            "id": id, "name": name, "conclusion": conclusion, "head_sha": head_sha,
            "html_url": format!("https://github.com/{repo}/runs/{id}"),
            "output": { "title": format!("{name} {conclusion}"), "summary": summary },
            "check_suite": { "head_branch": "main" }
        },
        "repository": { "full_name": repo, "default_branch": "main" }
    })
}

pub fn push(repo: &str, after: &str, commits: Value) -> Value {
    json!({
        "ref": "refs/heads/main", "after": after,
        "repository": { "full_name": repo, "default_branch": "main" },
        "commits": commits
    })
}
