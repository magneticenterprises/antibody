//! Opt-in retrieval latency check against the spec target (indexed retrieval
//! p95 <= 1.5 s under pilot load, no LLM inference). Run with:
//!   DATABASE_URL=... cargo test -p antibody-api --release --test load -- --ignored --nocapture

mod common;

use antibody_core::normalize;
use common::*;
use serde_json::json;
use std::time::Instant;

#[sqlx::test(migrator = "antibody_db::MIGRATOR")]
#[ignore = "load test; run explicitly"]
async fn retrieval_p95_under_pilot_load(pool: sqlx::PgPool) {
    let app = TestApp::spawn_with(
        pool,
        antibody_providers::Providers {
            embedding: Some(std::sync::Arc::new(
                antibody_providers::hash::HashEmbedding::new(384),
            )),
            llm: None,
        },
        |_| {},
    )
    .await;
    let mut repos = Vec::new();
    for i in 0..5 {
        repos.push(app.create_repo(&format!("acme/svc{i}")).await);
    }
    // 5,000 solutions with distinct signatures, plus embeddings for the vector stage.
    let words = [
        "pool",
        "timeout",
        "nullable",
        "migration",
        "deadlock",
        "serde",
        "tokio",
        "panic",
        "auth",
        "cache",
        "index",
        "schema",
    ];
    let embedder = antibody_providers::hash::HashEmbedding::new(384);
    let started = Instant::now();
    for i in 0..5000usize {
        let sig = format!(
            "error[e{:04}]: {} {} failed in module_{}",
            i % 9000,
            words[i % 12],
            words[(i / 12) % 12],
            i
        );
        let title = format!("{} {} issue {}", words[i % 12], words[(i * 7) % 12], i);
        let remedy = format!(
            "Adjust the {} configuration and retry the {} step; see handler_{}.",
            words[(i * 3) % 12],
            words[(i * 5) % 12],
            i
        );
        let id = uuid::Uuid::now_v7();
        sqlx::query(
            "INSERT INTO solutions (id, organization_id, repository_id, lineage_key, fingerprint, title, symptom, remedy,
                                    error_signatures, signature_hashes, keywords, extractor, state)
             VALUES ($1, $2, $3, $4, $4, $5, $5, $6, ARRAY[$7], ARRAY[$8], $7, 'load', 'verified')",
        )
        .bind(id)
        .bind(app.org_id)
        .bind(repos[i % 5])
        .bind(format!("sig:{i}"))
        .bind(&title)
        .bind(&remedy)
        .bind(&sig)
        .bind(normalize::signature_hash(&sig))
        .execute(&app.pool)
        .await
        .unwrap();
        let v = embedder.embed_one(&format!("{title}\n{remedy}"));
        sqlx::query("INSERT INTO embeddings (id, organization_id, solution_id, content_hash, provider, model, model_version, dimensions, embedding) VALUES ($1, $2, $3, 'x', 'local-hash', 'hash-ngram', '1', 384, $4)")
            .bind(uuid::Uuid::now_v7()).bind(app.org_id).bind(id).bind(pgvector::Vector::from(v))
            .execute(&app.pool).await.unwrap();
    }
    sqlx::query("ANALYZE").execute(&app.pool).await.unwrap();
    eprintln!("seeded 5000 solutions in {:?}", started.elapsed());

    // 400 searches, 8 concurrent, mixing exact-error and free-text queries.
    let mut latencies = Vec::new();
    for batch in 0..50 {
        let mut handles = Vec::new();
        for j in 0..8 {
            let n = batch * 8 + j;
            let body = if n % 2 == 0 {
                json!({ "task": "build is failing", "error": format!("error[E{:04}]: {} {} failed in module_{}", n % 9000, words[n % 12], words[(n / 12) % 12], n), "repository": "acme/svc0" })
            } else {
                json!({ "task": format!("{} {} problem after upgrade, deadlock in cache", words[n % 12], words[(n * 7) % 12]), "repository": "acme/svc1" })
            };
            let http = app.http.clone();
            let url = app.url("/v1/context/search");
            let token = app.admin_token.clone();
            handles.push(tokio::spawn(async move {
                let t = Instant::now();
                let r = http
                    .post(url)
                    .bearer_auth(token)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                assert!(r.status().is_success());
                let v: serde_json::Value = r.json().await.unwrap();
                (
                    t.elapsed().as_millis() as u64,
                    v["suggestions"].as_array().map(|a| a.len()).unwrap_or(0),
                )
            }));
        }
        for h in handles {
            latencies.push(h.await.unwrap());
        }
    }
    let mut ms: Vec<u64> = latencies.iter().map(|l| l.0).collect();
    ms.sort();
    let p = |q: f64| ms[((ms.len() as f64 * q).ceil() as usize).saturating_sub(1)];
    let hits = latencies.iter().filter(|l| l.1 > 0).count();
    eprintln!(
        "searches={} p50={}ms p95={}ms p99={}ms max={}ms with_results={}",
        ms.len(),
        p(0.5),
        p(0.95),
        p(0.99),
        ms[ms.len() - 1],
        hits
    );
    assert!(p(0.95) <= 1500, "p95 {}ms exceeds the 1.5s target", p(0.95));
}
