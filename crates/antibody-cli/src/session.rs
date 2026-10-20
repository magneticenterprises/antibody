//! `hook session-end`: spool the bounded hook input privately and detach.
//! Git and transcript processing run in the child, outside the hook deadline.

use crate::{Agent, config, git, upload};
use antibody_core::api::{SessionHints, SessionIngestRequest, TranscriptMessage};
use antibody_core::applicability::DependencyVersion;
use antibody_core::model::AgentType;
use antibody_integrations::hooks::HookInput;
use antibody_integrations::transcripts::{Limits, parse_claude_code, parse_codex};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MAX_TRANSCRIPT_BYTES: u64 = 20 * 1024 * 1024;
const MAX_STDIN_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DEPS: usize = 100;

/// What is written to the spool file and consumed by `antibody upload`.
#[derive(Serialize, Deserialize)]
pub struct Spooled {
    pub idempotency_key: String,
    pub request: SessionIngestRequest,
}

#[derive(Serialize, Deserialize)]
struct PendingSession {
    agent: Agent,
    input: HookInput,
}

pub fn agent_type(agent: Agent) -> AgentType {
    match agent {
        Agent::ClaudeCode => AgentType::ClaudeCode,
        Agent::Codex => AgentType::Codex,
    }
}

/// Deterministic key: same agent + session + transcript bytes => same key.
pub fn idempotency_key(agent: Agent, session_id: &str, transcript: &[u8]) -> String {
    let (prefix, tag) = match agent {
        Agent::ClaudeCode => ("cc-", "claude-code"),
        Agent::Codex => ("cx-", "codex"),
    };
    let mut h = Sha256::new();
    h.update(tag.as_bytes());
    h.update([0]);
    h.update(session_id.as_bytes());
    h.update([0]);
    h.update(transcript);
    format!("{prefix}{}", &hex::encode(h.finalize())[..32])
}

/// Read the transcript, keeping only the tail when it exceeds the cap.
pub fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let mut f = std::fs::File::open(path).context("opening transcript")?;
    let len = f.metadata()?.len();
    let mut buf = Vec::new();
    if len > cap {
        f.seek(SeekFrom::Start(len - cap))?;
        f.take(cap).read_to_end(&mut buf)?;
        // Drop the partial first line.
        if let Some(nl) = buf.iter().position(|b| *b == b'\n') {
            buf.drain(..=nl);
        }
    } else {
        f.read_to_end(&mut buf)?;
    }
    Ok(buf)
}

pub fn parse_messages(agent: Agent, bytes: &[u8]) -> Vec<TranscriptMessage> {
    let text = String::from_utf8_lossy(bytes);
    let limits = Limits::default();
    match agent {
        Agent::ClaudeCode => parse_claude_code(&text, &limits),
        Agent::Codex => parse_codex(&text, &limits),
    }
}

/// Build the ingest request. `Ok(None)` means "skip quietly".
pub fn build_request(agent: Agent, input: &HookInput) -> Result<Option<Spooled>> {
    let (Some(session_id), Some(tp), Some(cwd)) = (
        input.session_id.as_deref(),
        input.transcript_path.as_deref(),
        input.cwd.as_deref(),
    ) else {
        return Ok(None);
    };
    let cwd = Path::new(cwd);
    let Some(info) = git::collect(cwd) else {
        return Ok(None);
    };
    let Some(repository) = git::resolve_repository(Some(&info)) else {
        return Ok(None);
    };
    let tp = Path::new(tp);
    if !tp.is_file() {
        return Ok(None);
    }
    let bytes = read_capped(tp, MAX_TRANSCRIPT_BYTES)?;
    let messages = parse_messages(agent, &bytes);
    if messages.is_empty() {
        return Ok(None);
    }
    let dependencies = collect_dependencies(cwd);
    let hints = (!dependencies.is_empty()).then(|| SessionHints {
        dependencies,
        ..Default::default()
    });
    let request = SessionIngestRequest {
        agent_type: agent_type(agent),
        external_id: Some(session_id.to_string()),
        repository,
        revision: info.revision,
        branch: info.branch,
        worktree_dirty: info.dirty,
        component_path: config::env("ANTIBODY_COMPONENT"),
        messages,
        hints,
        started_at: None,
        ended_at: None,
    };
    Ok(Some(Spooled {
        idempotency_key: idempotency_key(agent, session_id, &bytes),
        request,
    }))
}

/// Direct dependencies of a root Cargo workspace/package with versions from
/// `Cargo.lock`. Cheap line scanning, bounded; returns empty on any problem.
pub fn collect_dependencies(root: &Path) -> Vec<DependencyVersion> {
    fn read(p: PathBuf) -> Option<String> {
        let md = std::fs::metadata(&p).ok()?;
        if md.len() > 5 * 1024 * 1024 {
            return None;
        }
        std::fs::read_to_string(p).ok()
    }
    let (Some(manifest), Some(lock)) =
        (read(root.join("Cargo.toml")), read(root.join("Cargo.lock")))
    else {
        return vec![];
    };
    let direct = direct_dep_names(&manifest);
    let mut out: Vec<DependencyVersion> = Vec::new();
    for (name, version) in lock_versions(&lock) {
        if direct.contains(&name) && !out.iter().any(|d| d.name == name) {
            out.push(DependencyVersion {
                name,
                version,
                ecosystem: Some("cargo".into()),
            });
            if out.len() >= MAX_DEPS {
                break;
            }
        }
    }
    out
}

fn direct_dep_names(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            let sec = line.trim_matches(|c| c == '[' || c == ']');
            in_deps = sec == "dependencies"
                || sec == "dev-dependencies"
                || sec == "build-dependencies"
                || sec == "workspace.dependencies";
            continue;
        }
        if in_deps
            && !line.starts_with('#')
            && let Some((k, _)) = line.split_once('=')
        {
            let k = k
                .trim()
                .split('.')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"');
            if !k.is_empty() {
                names.push(k.to_string());
            }
        }
    }
    names
}

fn lock_versions(lock: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            name = None;
        } else if let Some(v) = line.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("version = ")
            && let Some(n) = name.take()
        {
            out.push((n, v.trim_matches('"').to_string()));
        }
    }
    out
}

fn random_token() -> String {
    let mut h = Sha256::new();
    let mut seed = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut seed);
    }
    h.update(seed);
    h.update(std::process::id().to_le_bytes());
    h.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    hex::encode(h.finalize())[..32].to_string()
}

pub fn random_hex() -> String {
    random_token()
}

/// Write the spool file privately (dir 0700, file 0600).
pub fn write_spool(dir: &Path, spooled: &impl Serialize) -> Result<PathBuf> {
    let spool = dir.join("spool");
    create_private_dir(&spool)?;
    let path = spool.join(format!("{}.json", random_token()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&path).context("creating spool file")?;
    f.write_all(&serde_json::to_vec(spooled)?)?;
    Ok(path)
}

fn create_private_dir(p: &Path) -> Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(p).context("creating state directory")
}

fn spawn_preparation(file: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("locating current executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("prepare-upload")
        .arg("--file")
        .arg(file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().context("spawning session preparation")?;
    Ok(())
}

pub fn run_session_end(agent: Agent) -> Result<()> {
    // Without credentials there is nothing to upload to; skip quietly.
    if config::Config::from_env().is_err() {
        return Ok(());
    }
    let mut raw = String::new();
    std::io::stdin()
        .take(MAX_STDIN_BYTES)
        .read_to_string(&mut raw)
        .context("reading stdin")?;
    let input: HookInput =
        serde_json::from_str(&raw).map_err(|_| anyhow!("stdin is not valid hook JSON"))?;
    let Some(dir) = config::state_dir() else {
        bail!("no home directory for spool")
    };
    let path = write_spool(&dir, &PendingSession { agent, input })?;
    if let Err(error) = spawn_preparation(&path) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

/// The detached child owns both preparation and delivery. Keep the input file
/// until preparation finishes, then remove it even when there is no transcript.
pub fn run_prepare_upload(file: &Path) {
    let prepare = || -> Result<Option<PathBuf>> {
        let pending: PendingSession =
            serde_json::from_slice(&read_capped(file, MAX_STDIN_BYTES + 1024)?)?;
        let Some(spooled) = build_request(pending.agent, &pending.input)? else {
            return Ok(None);
        };
        let dir = config::state_dir().context("no home directory for spool")?;
        write_spool(&dir, &spooled).map(Some)
    };
    let result = prepare();
    let _ = std::fs::remove_file(file);
    match result {
        Ok(Some(path)) => upload::run_upload(&path),
        Ok(None) => {}
        // Keep transcript contents and filesystem paths out of the log.
        Err(_) => upload::append_log("skipped: session preparation failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_key_is_deterministic_and_valid() {
        let a = idempotency_key(Agent::ClaudeCode, "s1", b"abc");
        assert_eq!(a, idempotency_key(Agent::ClaudeCode, "s1", b"abc"));
        assert_ne!(a, idempotency_key(Agent::ClaudeCode, "s1", b"abd"));
        assert_ne!(a, idempotency_key(Agent::ClaudeCode, "s2", b"abc"));
        assert_ne!(a, idempotency_key(Agent::Codex, "s1", b"abc"));
        assert!(a.starts_with("cc-") && a.len() == 35);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn fixture_transcript_parses() {
        let bytes = include_bytes!("../tests/fixtures/claude_transcript.jsonl");
        let msgs = parse_messages(Agent::ClaudeCode, bytes);
        assert_eq!(msgs.len(), 3);
        assert!(msgs[0].content.contains("pool timed out"));
    }

    #[test]
    fn fixture_hook_input_deserializes() {
        let raw = include_str!("../tests/fixtures/hook_session_end.json");
        let input: HookInput = serde_json::from_str(raw).unwrap();
        assert_eq!(input.session_id.as_deref(), Some("sess-abc-123"));
        assert_eq!(input.hook_event_name.as_deref(), Some("SessionEnd"));
    }

    #[test]
    fn skips_when_not_a_git_repo_or_missing_fields() {
        assert!(
            build_request(Agent::ClaudeCode, &HookInput::default())
                .unwrap()
                .is_none()
        );
        let input = HookInput {
            session_id: Some("s".into()),
            transcript_path: Some("/nonexistent/t.jsonl".into()),
            cwd: Some("/".into()),
            ..Default::default()
        };
        assert!(build_request(Agent::ClaudeCode, &input).unwrap().is_none());
    }

    #[test]
    fn dependency_scan_is_direct_only() {
        let manifest = "[workspace.dependencies]\ntokio = { version = \"1\" }\nserde.workspace = true\n[package]\nname = \"x\"\n";
        let lock = "[[package]]\nname = \"tokio\"\nversion = \"1.38.0\"\n[[package]]\nname = \"mio\"\nversion = \"1.0.0\"\n[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n";
        let names = direct_dep_names(manifest);
        assert_eq!(names, vec!["tokio", "serde"]);
        let v = lock_versions(lock);
        assert!(v.contains(&("tokio".into(), "1.38.0".into())));
    }

    #[test]
    fn read_capped_keeps_tail_on_line_boundary() {
        let dir = std::env::temp_dir().join(format!("antibody-cli-test-{}", random_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.jsonl");
        std::fs::write(&p, "aaaaaaaa\nbbbb\ncccc\n").unwrap();
        let got = read_capped(&p, 12).unwrap();
        assert_eq!(String::from_utf8(got).unwrap(), "bbbb\ncccc\n");
        std::fs::remove_dir_all(dir).ok();
    }
}
