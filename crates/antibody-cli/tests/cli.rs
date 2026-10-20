//! Integration tests: run the real binary against a tiny local HTTP server.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.as_str())
    }
}

/// Serve canned `(status, body)` responses in order, repeating the last one.
fn serve(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        let mut i = 0;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let (head_end, len) = loop {
                let n = stream.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break (0, 0);
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (pos + 4, len);
                }
            };
            if head_end == 0 {
                continue;
            }
            while buf.len() < head_end + len {
                let n = stream.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut lines = head.lines();
            let path = lines
                .next()
                .unwrap_or("")
                .split(' ')
                .nth(1)
                .unwrap_or("")
                .to_string();
            let headers = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .collect();
            let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
            s2.lock().unwrap().push(Seen {
                path,
                headers,
                body,
            });
            let (status, resp) = responses[i.min(responses.len() - 1)].clone();
            i += 1;
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                resp.len()
            );
        }
    });
    (url, seen)
}

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "antibody-it-{tag}-{}-{:?}",
            std::process::id(),
            Instant::now()
        ));
        let p = PathBuf::from(p.to_string_lossy().replace(['(', ')', ' ', ':'], "_"));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_antibody"));
    for k in [
        "ANTIBODY_URL",
        "ANTIBODY_TOKEN",
        "ANTIBODY_REPOSITORY",
        "ANTIBODY_COMPONENT",
        "ANTIBODY_ORG",
        "ANTIBODY_CI_SECRET",
    ] {
        c.env_remove(k);
    }
    c
}

fn run_with_stdin(mut cmd: Command, input: &str) -> (i32, String) {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

const ACCEPTED: &str = r#"{"id":"00000000-0000-0000-0000-000000000001","status_url":"/v1/sessions/x","duplicate":false}"#;

fn hook_response() -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": "Pool timeout: raise pool size"
        }
    })
    .to_string()
}

fn write_spool(dir: &Path, key: &str) -> PathBuf {
    let spool = serde_json::json!({
        "idempotency_key": key,
        "request": {"agent_type": "claude_code", "external_id": "s1", "repository": "acme/api",
            "worktree_dirty": false,
            "messages": [{"role": "user", "content": "hello"}]}
    });
    let p = dir.join("spool.json");
    std::fs::write(&p, spool.to_string()).unwrap();
    p
}

#[test]
fn upload_retries_with_same_idempotency_key_and_removes_spool() {
    let (url, seen) = serve(vec![
        (503, "{}".into()),
        (429, "{}".into()),
        (202, ACCEPTED.into()),
    ]);
    let tmp = Tmp::new("upload");
    let file = write_spool(&tmp.0, "cc-0123456789abcdef0123456789abcdef");
    let status = bin()
        .args(["upload", "--file"])
        .arg(&file)
        .env("ANTIBODY_URL", &url)
        .env("ANTIBODY_TOKEN", "tok")
        .env("ANTIBODY_HOME", &tmp.0)
        .env("ANTIBODY_UPLOAD_BACKOFF_MS", "5")
        .status()
        .unwrap();
    assert!(status.success());
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    for s in seen.iter() {
        assert_eq!(s.path, "/v1/sessions");
        assert_eq!(
            s.header("idempotency-key"),
            Some("cc-0123456789abcdef0123456789abcdef")
        );
        assert_eq!(s.header("authorization"), Some("Bearer tok"));
        assert!(s.body.contains("\"repository\":\"acme/api\""));
    }
    assert!(!file.exists(), "spool file must be deleted");
    let log = std::fs::read_to_string(tmp.0.join("upload.log")).unwrap();
    assert!(log.contains("Accepted") && !log.contains("hello") && !log.contains("tok"));
}

#[test]
fn upload_stops_on_client_error() {
    let (url, seen) = serve(vec![(422, "{}".into())]);
    let tmp = Tmp::new("upload4xx");
    let file = write_spool(&tmp.0, "cc-0123456789abcdef0123456789abcdef");
    bin()
        .args(["upload", "--file"])
        .arg(&file)
        .env("ANTIBODY_URL", &url)
        .env("ANTIBODY_TOKEN", "tok")
        .env("ANTIBODY_HOME", &tmp.0)
        .env("ANTIBODY_UPLOAD_BACKOFF_MS", "5")
        .status()
        .unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(!file.exists());
    assert!(
        std::fs::read_to_string(tmp.0.join("upload.log"))
            .unwrap()
            .contains("Permanent")
    );
}

#[test]
fn user_prompt_hook_prints_claude_code_json() {
    let (url, seen) = serve(vec![(200, hook_response()), (200, "{}".into())]);
    let command = || {
        let mut cmd = bin();
        cmd.args(["hook", "user-prompt", "--agent", "claude-code"])
            .env("ANTIBODY_URL", &url)
            .env("ANTIBODY_TOKEN", "tok")
            .env("ANTIBODY_REPOSITORY", "acme/api")
            .env("ANTIBODY_COMPONENT", "apps/server");
        cmd
    };
    let input = r#"{"session_id":"s9","cwd":"/","hook_event_name":"UserPromptSubmit","prompt":"cargo test fails with pool timed out"}"#;
    let (code, out) = run_with_stdin(command(), input);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    assert!(
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Pool timeout")
    );
    // Native hook session dedupe returns an empty HookOutput on the next prompt.
    assert_eq!(run_with_stdin(command(), input), (0, String::new()));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].path, "/v1/hooks/user-prompt-submit");
    assert_eq!(seen[0].header("x-antibody-repository"), Some("acme/api"));
    assert_eq!(seen[0].header("x-antibody-component"), Some("apps/server"));
    assert!(seen[0].header("x-antibody-every-prompt").is_none());
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(body["session_id"], "s9");
    assert_eq!(body["hook_event_name"], "UserPromptSubmit");
    assert_eq!(body["prompt"], "cargo test fails with pool timed out");
}

#[test]
fn user_prompt_hook_short_prompt_prints_nothing_and_skips_request() {
    let (url, seen) = serve(vec![(200, hook_response())]);
    let mut cmd = bin();
    cmd.args(["hook", "user-prompt", "--agent", "codex"])
        .env("ANTIBODY_URL", &url)
        .env("ANTIBODY_TOKEN", "tok");
    let (code, out) = run_with_stdin(cmd, r#"{"prompt":"yes"}"#);
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn user_prompt_hook_fails_open() {
    // Unreachable server, server error, garbage stdin, missing config: always exit 0, no stdout.
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let (err_url, _) = serve(vec![(500, "oops".into())]);
    let input = r#"{"prompt":"cargo test fails with pool timed out"}"#;
    for (url, stdin) in [
        (dead.as_str(), input),
        (err_url.as_str(), input),
        (err_url.as_str(), "not json"),
    ] {
        let mut cmd = bin();
        cmd.args(["hook", "user-prompt", "--agent", "claude-code"])
            .env("ANTIBODY_URL", url)
            .env("ANTIBODY_TOKEN", "t");
        assert_eq!(run_with_stdin(cmd, stdin), (0, String::new()));
    }
    let mut cmd = bin();
    cmd.args(["hook", "user-prompt", "--agent", "claude-code"]);
    assert_eq!(run_with_stdin(cmd, input), (0, String::new()));
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

#[test]
fn session_end_spools_and_uploads_detached() {
    let (url, seen) = serve(vec![(202, ACCEPTED.into())]);
    let tmp = Tmp::new("sessionend");
    let repo = tmp.0.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:acme/api.git"],
    );
    std::fs::write(repo.join("a.txt"), "x").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    std::fs::write(repo.join("dirty.txt"), "y").unwrap();

    let transcript = repo.join("..").join("t.jsonl");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/claude_transcript.jsonl"
        ),
        &transcript,
    )
    .unwrap();
    let hook = serde_json::json!({
        "session_id": "sess-abc-123", "transcript_path": transcript, "cwd": repo,
        "hook_event_name": "SessionEnd", "reason": "other"
    });
    let home = tmp.0.join("home");
    let mut cmd = bin();
    cmd.args(["hook", "session-end", "--agent", "claude-code"])
        .env("ANTIBODY_URL", &url)
        .env("ANTIBODY_TOKEN", "tok")
        .env("ANTIBODY_HOME", &home);
    let started = Instant::now();
    let (code, out) = run_with_stdin(cmd, &hook.to_string());
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(started.elapsed() < Duration::from_secs(5));

    let deadline = Instant::now() + Duration::from_secs(15);
    while seen.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "detached upload should reach the server");
    let s = &seen[0];
    assert_eq!(s.path, "/v1/sessions");
    let key = s.header("idempotency-key").unwrap();
    assert!(key.starts_with("cc-") && key.len() == 35);
    let body: serde_json::Value = serde_json::from_str(&s.body).unwrap();
    assert_eq!(body["agent_type"], "claude_code");
    assert_eq!(body["external_id"], "sess-abc-123");
    assert_eq!(body["repository"], "acme/api");
    assert_eq!(body["worktree_dirty"], true);
    assert_eq!(body["messages"].as_array().unwrap().len(), 3);
    assert_eq!(body["revision"].as_str().unwrap().len(), 40);
}

#[test]
fn session_end_outside_git_repo_is_silent() {
    let (url, seen) = serve(vec![(202, ACCEPTED.into())]);
    let tmp = Tmp::new("nogit");
    let transcript = tmp.0.join("t.jsonl");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/claude_transcript.jsonl"
        ),
        &transcript,
    )
    .unwrap();
    let hook = serde_json::json!({"session_id": "s", "transcript_path": transcript, "cwd": tmp.0});
    let mut cmd = bin();
    cmd.args(["hook", "session-end", "--agent", "codex"])
        .env("ANTIBODY_URL", &url)
        .env("ANTIBODY_TOKEN", "tok")
        .env("ANTIBODY_HOME", tmp.0.join("home"))
        .env("GIT_CEILING_DIRECTORIES", &tmp.0);
    assert_eq!(run_with_stdin(cmd, &hook.to_string()), (0, String::new()));
    std::thread::sleep(Duration::from_millis(300));
    assert!(seen.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[path = "cli/slow_git.rs"]
mod slow_git;

#[test]
fn ci_report_signs_request_and_always_exits_zero() {
    let resp = serde_json::json!({
        "ci_event_id": "00000000-0000-0000-0000-000000000009", "duplicate": false,
        "signature": "sig", "suggestions": [], "blocking": false, "notice": ""
    })
    .to_string();
    let (url, seen) = serve(vec![(200, resp)]);
    let tmp = Tmp::new("ci");
    let status = bin()
        .args(["ci", "report", "--check", "test", "--revision", "abc123"])
        .current_dir(&tmp.0)
        .env("ANTIBODY_URL", &url)
        .env("ANTIBODY_TOKEN", "tok")
        .env("ANTIBODY_REPOSITORY", "acme/api")
        .env("ANTIBODY_ORG", "acme")
        .env("ANTIBODY_CI_SECRET", "s3cret")
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let seen = seen.lock().unwrap();
    let s = &seen[0];
    assert_eq!(s.path, "/v1/webhooks/ci");
    assert_eq!(s.header("x-antibody-org"), Some("acme"));
    let ts = s.header("x-antibody-timestamp").unwrap();
    let sig = s.header("x-antibody-signature").unwrap();
    assert!(s.header("x-antibody-delivery").unwrap().len() >= 16);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert_eq!(
        antibody_integrations::ci::verify(b"s3cret", ts, s.body.as_bytes(), sig, now),
        Ok(())
    );

    // Server down / missing secret still exits 0.
    let status = bin()
        .args(["ci", "report", "--check", "test", "--revision", "abc"])
        .env("ANTIBODY_URL", "http://127.0.0.1:1")
        .env("ANTIBODY_TOKEN", "t")
        .env("ANTIBODY_REPOSITORY", "acme/api")
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
}
