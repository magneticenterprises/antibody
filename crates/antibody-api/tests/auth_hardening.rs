//! Regression coverage for credential budgets and atomic account provisioning.

mod common;

use antibody_api::bootstrap::bootstrap;
use antibody_db::tokens;
use common::{ADMIN_PASSWORD, TestApp};
use reqwest::StatusCode;
use serde_json::json;

const RECOVERY_PASSWORD: &str = "fresh-recovery-admin-password";

async fn account_counts(pool: &sqlx::PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM organizations), (SELECT count(*) FROM principals),
                (SELECT count(*) FROM users), (SELECT count(*) FROM api_tokens),
                (SELECT count(*) FROM audit_events)",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn password_recovery_sql() -> String {
    // Repository-owned SQL with fixed fixture literals; no request data.
    include_str!("../../../docs/security.md")
        .split("### Scenario C:")
        .nth(1)
        .unwrap()
        .split("```sql")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap()
        .lines()
        .map(str::trim_start)
        .filter(|line| !line.starts_with("\\set"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace(":'organization_slug'", "'acme'")
        .replace(":'compromised_username'", "'admin'")
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn bootstrap_duplicate_username_rolls_back_and_can_be_retried(pool: sqlx::PgPool) {
    bootstrap(
        &pool,
        "first",
        "First",
        "admin",
        Some(ADMIN_PASSWORD.into()),
    )
    .await
    .unwrap();
    assert!(
        bootstrap(
            &pool,
            "second",
            "Second",
            "admin",
            Some(ADMIN_PASSWORD.into())
        )
        .await
        .is_err()
    );
    assert_eq!(account_counts(&pool).await, (1, 1, 1, 1, 1));
    let second = bootstrap(
        &pool,
        "second",
        "Second",
        "other",
        Some(ADMIN_PASSWORD.into()),
    )
    .await
    .unwrap();
    assert!(
        tokens::authenticate_token(&pool, &second.admin_token)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(account_counts(&pool).await, (2, 2, 2, 2, 2));
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn bootstrap_validates_credentials_before_writes(pool: sqlx::PgPool) {
    for password in ["x".repeat(257), "界".repeat(86), "short".into()] {
        assert!(
            bootstrap(&pool, "acme", "Acme", "admin", Some(password))
                .await
                .unwrap_err()
                .to_string()
                .contains("password")
        );
    }
    assert!(
        bootstrap(
            &pool,
            "acme",
            "Acme",
            "invalid user",
            Some(ADMIN_PASSWORD.into())
        )
        .await
        .is_err()
    );
    assert_eq!(account_counts(&pool).await, (0, 0, 0, 0, 0));
}

async fn reject_audit(pool: &sqlx::PgPool, action: &str) {
    // Test-only failure injection after account records have been inserted.
    sqlx::raw_sql(
        "CREATE TABLE rejected_audit_action (action text NOT NULL);
         CREATE FUNCTION reject_account_audit() RETURNS trigger LANGUAGE plpgsql AS $$
           BEGIN
             IF NEW.action IN (SELECT action FROM rejected_audit_action) THEN
               RAISE EXCEPTION 'injected account audit failure';
             END IF;
             RETURN NEW;
           END $$;
         CREATE TRIGGER reject_account_audit BEFORE INSERT ON audit_events
         FOR EACH ROW EXECUTE FUNCTION reject_account_audit();",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO rejected_audit_action VALUES ($1)")
        .bind(action)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn bootstrap_audit_failure_rolls_back_every_record(pool: sqlx::PgPool) {
    reject_audit(&pool, "organization.bootstrapped").await;
    assert!(
        bootstrap(&pool, "acme", "Acme", "admin", Some(ADMIN_PASSWORD.into()))
            .await
            .is_err()
    );
    assert_eq!(account_counts(&pool).await, (0, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn create_user_conflicts_and_audit_failures_leave_no_orphan_principals(pool: sqlx::PgPool) {
    let app = TestApp::spawn(pool).await;
    let body = json!({
        "kind": "user", "display_name": "Duplicate", "role": "admin",
        "username": "admin", "password": ADMIN_PASSWORD,
    });
    assert_eq!(
        app.post("/v1/admin/principals", &app.admin_token, &body)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(account_counts(&app.pool).await, (1, 1, 1, 1, 1));
    reject_audit(&app.pool, "principal.created").await;
    let mut body = body;
    body["username"] = json!("another-admin");
    assert_eq!(
        app.post("/v1/admin/principals", &app.admin_token, &body)
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(account_counts(&app.pool).await, (1, 1, 1, 1, 1));
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn bearer_whitespace_cannot_reset_credential_budget(pool: sqlx::PgPool) {
    let app = TestApp::spawn_with(pool, antibody_providers::Providers::none(), |cfg| {
        cfg.rate_limit_per_minute = 1;
        cfg.trust_proxy = true;
    })
    .await;
    for i in 0..6 {
        let resp = app
            .http
            .get(app.url("/v1/auth/session"))
            .header(
                "authorization",
                format!("bEaReR {}{}", " ".repeat(i), app.admin_token),
            )
            .header("x-forwarded-for", format!("10.0.0.{}", i + 1))
            .header("x-forwarded-proto", "https")
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            if i < 5 {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn rotating_invalid_credentials_and_arbitrary_cookies_share_ip_budget(pool: sqlx::PgPool) {
    let app = TestApp::spawn_with(pool, antibody_providers::Providers::none(), |cfg| {
        cfg.rate_limit_per_minute = 1;
    })
    .await;
    for i in 0..6 {
        let request = app
            .http
            .get(app.url("/v1/auth/session"))
            .header("cookie", format!("unrelated={i}"));
        let request = if i % 2 == 0 {
            request.bearer_auth(format!("abt_invalid{i}"))
        } else {
            request
        };
        let resp = request.send().await.unwrap();
        assert_eq!(
            resp.status(),
            if i < 5 {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn unrelated_cookies_cannot_reset_browser_credential_budget(pool: sqlx::PgPool) {
    let app = TestApp::spawn_with(pool, antibody_providers::Providers::none(), |cfg| {
        cfg.rate_limit_per_minute = 1;
        cfg.trust_proxy = true;
    })
    .await;
    let login = app
        .http
        .post(app.url("/v1/auth/login"))
        .header("x-forwarded-for", "10.1.0.1")
        .header("x-forwarded-proto", "https")
        .json(&json!({"username": "admin", "password": ADMIN_PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let client = reqwest::Client::new();
    for i in 0..6 {
        let resp = client
            .get(app.url("/v1/auth/session"))
            .header("cookie", format!("unrelated={i}; {cookie}"))
            .header("x-forwarded-for", format!("10.2.0.{}", i + 1))
            .header("x-forwarded-proto", "https")
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            if i < 5 {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
}

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
async fn compromised_principal_recovery_disables_old_login_and_only_its_credentials(
    pool: sqlx::PgPool,
) {
    let app = TestApp::spawn(pool).await;
    let mut conn = app.pool.acquire().await.unwrap();
    let failure = sqlx::raw_sql(sqlx::AssertSqlSafe(password_recovery_sql()))
        .execute(&mut *conn)
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("replacement administrator"));
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        tokens::authenticate_token(&app.pool, &app.admin_token)
            .await
            .unwrap()
            .is_some()
    );
    let old_login = app
        .http
        .post(app.url("/v1/auth/login"))
        .json(&json!({"username": "admin", "password": ADMIN_PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(old_login.status(), StatusCode::OK);
    let replacement = TestApp::json(
        app.post(
            "/v1/admin/principals",
            &app.admin_token,
            &json!({
                "kind": "user", "display_name": "Recovery admin", "role": "admin",
                "username": "recovery-admin", "password": RECOVERY_PASSWORD,
            }),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    let replacement_id: uuid::Uuid = replacement["id"].as_str().unwrap().parse().unwrap();
    let replacement_client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    assert_eq!(
        replacement_client
            .post(app.url("/v1/auth/login"))
            .json(&json!({"username": "recovery-admin", "password": RECOVERY_PASSWORD}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let replacement_token = tokens::issue_token(
        &app.pool,
        tokens::NewToken {
            org_id: app.org_id,
            principal_id: replacement_id,
            name: "recovery",
            scopes: &[antibody_core::model::Scope::Admin],
            repository_ids: None,
            expires_at: None,
            created_by: Some(replacement_id),
        },
    )
    .await
    .unwrap();

    sqlx::raw_sql(sqlx::AssertSqlSafe(password_recovery_sql()))
        .execute(&app.pool)
        .await
        .unwrap();

    assert_eq!(
        app.get("/v1/auth/session", &app.admin_token).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.http
            .get(app.url("/v1/auth/session"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.http
            .post(app.url("/v1/auth/login"))
            .json(&json!({"username": "admin", "password": ADMIN_PASSWORD}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.get("/v1/auth/session", &replacement_token.token)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        replacement_client
            .get(app.url("/v1/auth/session"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        replacement_client
            .post(app.url("/v1/auth/login"))
            .json(&json!({"username": "recovery-admin", "password": RECOVERY_PASSWORD}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
