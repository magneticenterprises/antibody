//! `antibody upload --file`: deliver a spooled session with retries.

use crate::client;
use crate::config::{self, Config};
use crate::session::Spooled;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const ATTEMPTS: u32 = 5;
const LOG_MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Permanent(String),
    Exhausted(String),
}

fn retryable(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

/// Base backoff; tests lower it with `ANTIBODY_UPLOAD_BACKOFF_MS`.
fn base_backoff() -> Duration {
    let ms = config::env("ANTIBODY_UPLOAD_BACKOFF_MS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000u64);
    Duration::from_millis(ms.min(30_000))
}

pub fn deliver(cfg: &Config, spooled: &Spooled) -> Outcome {
    let http = match client::http(Duration::from_secs(30)) {
        Ok(c) => c,
        Err(e) => return Outcome::Permanent(format!("client: {e}")),
    };
    let body = match serde_json::to_vec(&spooled.request) {
        Ok(b) => b,
        Err(e) => return Outcome::Permanent(format!("serialize: {e}")),
    };
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(base_backoff() * 2u32.pow(attempt - 1));
        }
        let rb = client::authed(http.post(client::url(cfg, "/v1/sessions")), cfg)
            .header("Idempotency-Key", &spooled.idempotency_key)
            .header("Content-Type", "application/json")
            .body(body.clone());
        match rb.send() {
            Ok(resp) => {
                let s = resp.status().as_u16();
                if s == 202 || (200..300).contains(&s) {
                    return Outcome::Accepted;
                }
                last = format!("http {s}");
                if !retryable(s) {
                    return Outcome::Permanent(last);
                }
            }
            Err(e) => {
                last = if e.is_timeout() {
                    "timeout".into()
                } else {
                    "network error".into()
                }
            }
        }
    }
    Outcome::Exhausted(last)
}

pub fn run_upload(file: &Path) {
    let parsed: Option<Spooled> = std::fs::read(file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let line = match (Config::from_env(), parsed) {
        (Ok(cfg), Some(sp)) => {
            let outcome = deliver(&cfg, &sp);
            format!(
                "{} {:?} key={} messages={}",
                sp.request.repository,
                outcome,
                sp.idempotency_key,
                sp.request.messages.len()
            )
        }
        (Err(_), _) => "skipped: ANTIBODY_URL/ANTIBODY_TOKEN not set".to_string(),
        (_, None) => "skipped: unreadable spool file".to_string(),
    };
    // Spooled data is removed on success and on permanent failure alike.
    let _ = std::fs::remove_file(file);
    append_log(&line);
}

pub(crate) fn append_log(line: &str) {
    let Some(dir) = config::state_dir() else {
        return;
    };
    let path = dir.join("upload.log");
    if let Ok(md) = std::fs::metadata(&path)
        && md.len() > LOG_MAX_BYTES
        && let Ok(bytes) = std::fs::read(&path)
    {
        // Keep the newest half, starting at a line boundary.
        let text = String::from_utf8_lossy(&bytes[bytes.len() / 2..]).into_owned();
        let keep = text.split_once('\n').map(|(_, r)| r).unwrap_or("");
        let _ = std::fs::write(&path, keep);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut f) = opts.open(path) {
        let _ = writeln!(f, "{secs} {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_classification() {
        for s in [408, 429, 500, 502, 503] {
            assert!(retryable(s), "{s}");
        }
        for s in [400, 401, 403, 404, 409, 413, 422] {
            assert!(!retryable(s), "{s}");
        }
    }
}
