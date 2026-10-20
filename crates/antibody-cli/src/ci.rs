//! `ci report`: signed CI failure lookup. Never blocks the build.

use crate::config::{Config, env};
use crate::{client, git, session};
use antibody_core::api::{CiFailureEvent, CiLookupResponse};
use antibody_integrations::ci::sign;
use antibody_integrations::hooks::render_context;
use anyhow::{Context, Result, anyhow};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

const MAX_EXCERPT_BYTES: u64 = 16 * 1024;

/// Headers for a signed delivery.
pub struct SignedHeaders {
    pub org: String,
    pub delivery: String,
    pub timestamp: String,
    pub signature: String,
}

pub fn signed_headers(
    org: &str,
    secret: &str,
    delivery: &str,
    timestamp: u64,
    body: &[u8],
) -> SignedHeaders {
    let ts = timestamp.to_string();
    SignedHeaders {
        org: org.to_string(),
        delivery: delivery.to_string(),
        signature: sign(secret.as_bytes(), &ts, body),
        timestamp: ts,
    }
}

/// Last `MAX_EXCERPT_BYTES` of the file (errors are usually at the end).
fn read_excerpt(p: &Path) -> Result<String> {
    let mut f = std::fs::File::open(p).context("opening excerpt file")?;
    let len = f.metadata()?.len();
    if len > MAX_EXCERPT_BYTES {
        f.seek(SeekFrom::Start(len - MAX_EXCERPT_BYTES))?;
    }
    let mut buf = Vec::new();
    f.take(MAX_EXCERPT_BYTES).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub fn run_report(
    check: &str,
    revision: &str,
    excerpt_file: Option<&Path>,
    url: Option<String>,
    component: Option<String>,
) -> Result<()> {
    let cfg = Config::from_env()?;
    let org = env("ANTIBODY_ORG").ok_or_else(|| anyhow!("ANTIBODY_ORG is not set"))?;
    let secret =
        env("ANTIBODY_CI_SECRET").ok_or_else(|| anyhow!("ANTIBODY_CI_SECRET is not set"))?;
    let info = std::env::current_dir().ok().and_then(|c| git::collect(&c));
    let repository = git::resolve_repository(info.as_ref())
        .ok_or_else(|| anyhow!("cannot determine repository; set ANTIBODY_REPOSITORY"))?;
    let event = CiFailureEvent {
        repository,
        revision: revision.to_string(),
        check_name: check.to_string(),
        signature: None,
        excerpt: excerpt_file.map(read_excerpt).transpose()?,
        url,
        component_path: component.or_else(|| env("ANTIBODY_COMPONENT")),
    };
    let body = serde_json::to_vec(&event)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let h = signed_headers(&org, &secret, &session::random_hex(), now, &body);
    let http = client::http(Duration::from_secs(20))?;
    let resp = http
        .post(client::url(&cfg, "/v1/webhooks/ci"))
        .header("Content-Type", "application/json")
        .header("X-Antibody-Org", &h.org)
        .header("X-Antibody-Delivery", &h.delivery)
        .header("X-Antibody-Timestamp", &h.timestamp)
        .header("X-Antibody-Signature", &h.signature)
        .body(body)
        .send()
        .context("CI request failed")?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("server returned http {}", status.as_u16()));
    }
    let parsed: CiLookupResponse = resp.json().context("decoding CI response")?;
    if parsed.suggestions.is_empty() {
        println!(
            "Antibody: no known solutions for this failure (signature {}).",
            parsed.signature
        );
    } else {
        // Reuse the compact renderer through a context response shape.
        let ctx = antibody_core::api::ContextSearchResponse {
            retrieval_id: parsed.ci_event_id,
            suggestions: parsed.suggestions,
            degraded: false,
            degraded_reasons: vec![],
            suppressed_incompatible: 0,
            latency_ms: 0,
            notice: parsed.notice,
        };
        if let Some(t) = render_context(&ctx) {
            println!("{t}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use antibody_integrations::ci::verify;

    #[test]
    fn headers_verify_with_server_logic() {
        let body = br#"{"repository":"acme/api"}"#;
        let h = signed_headers("acme", "s3cret", "abc123", 1_700_000_000, body);
        assert_eq!(h.org, "acme");
        assert_eq!(h.delivery, "abc123");
        assert_eq!(h.timestamp, "1700000000");
        assert!(h.signature.starts_with("sha256="));
        assert_eq!(
            verify(b"s3cret", &h.timestamp, body, &h.signature, 1_700_000_010),
            Ok(())
        );
        assert!(verify(b"other", &h.timestamp, body, &h.signature, 1_700_000_010).is_err());
    }

    #[test]
    fn event_serializes_without_empty_fields() {
        let e = CiFailureEvent {
            repository: "acme/api".into(),
            revision: "deadbeef".into(),
            check_name: "test".into(),
            signature: None,
            excerpt: None,
            url: None,
            component_path: None,
        };
        let v: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&e).unwrap()).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 3);
    }
}
