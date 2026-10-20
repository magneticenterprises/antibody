//! `hook user-prompt` and manual `search`.

use crate::config::{Config, env};
use crate::{Agent, client, git};
use antibody_core::api::{ContextSearchRequest, ContextSearchResponse};
use antibody_integrations::hooks::{HookInput, HookOutput, render_context};
use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

pub const MIN_PROMPT_CHARS: usize = 12;
pub const HOOK_TIMEOUT: Duration = Duration::from_millis(2500);
const MAX_ERROR_BYTES: usize = 16 * 1024;

fn search(
    cfg: &Config,
    req: &ContextSearchRequest,
    timeout: Duration,
) -> Result<ContextSearchResponse> {
    let http = client::http(timeout)?;
    let resp = client::authed(http.post(client::url(cfg, "/v1/context/search")), cfg)
        .json(req)
        .send()
        .context("search request failed")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("search returned http {}", status.as_u16());
    }
    resp.json().context("decoding search response")
}

/// JSON hook output for both agents (`hookSpecificOutput.additionalContext`).
/// Plain stdout is also accepted by both, but JSON keeps the shape explicit.
pub fn render_hook_json(resp: &HookOutput) -> Option<String> {
    resp.hook_specific_output.as_ref()?;
    serde_json::to_string(resp).ok()
}

pub fn run_user_prompt(_agent: Agent) -> Result<Option<String>> {
    let cfg = Config::from_env()?;
    let mut raw = String::new();
    std::io::stdin()
        .take(2 * 1024 * 1024)
        .read_to_string(&mut raw)?;
    let input: HookInput = serde_json::from_str(&raw).context("stdin is not valid hook JSON")?;
    let prompt = input.prompt.clone().unwrap_or_default();
    let prompt = prompt.trim();
    if prompt.chars().count() < MIN_PROMPT_CHARS {
        return Ok(None);
    }
    let repository = env("ANTIBODY_REPOSITORY").or_else(|| {
        input
            .cwd
            .as_deref()
            .and_then(|c| git::repository(Path::new(c)))
    });
    let http = client::http(HOOK_TIMEOUT)?;
    let mut req = client::authed(
        http.post(client::url(&cfg, "/v1/hooks/user-prompt-submit")),
        &cfg,
    );
    if let Some(repository) = repository {
        req = req.header("X-Antibody-Repository", repository);
    }
    if let Some(component) = env("ANTIBODY_COMPONENT") {
        req = req.header("X-Antibody-Component", component);
    }
    let resp = req.json(&input).send().context("hook request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("hook returned http {}", resp.status().as_u16());
    }
    let output = resp
        .json::<HookOutput>()
        .context("decoding hook response")?;
    Ok(render_hook_json(&output))
}

pub fn run_search(task: &str, error_file: Option<&Path>, json: bool) -> Result<()> {
    let cfg = Config::from_env()?;
    let error = match error_file {
        Some(p) => {
            let mut buf = Vec::new();
            std::fs::File::open(p)
                .context("opening error file")?
                .take(MAX_ERROR_BYTES as u64)
                .read_to_end(&mut buf)?;
            Some(String::from_utf8_lossy(&buf).into_owned())
        }
        None => None,
    };
    let cwd = std::env::current_dir().ok();
    let info = cwd.as_deref().and_then(git::collect);
    let req = ContextSearchRequest {
        task: task.to_string(),
        repository: git::resolve_repository(info.as_ref()),
        revision: info.as_ref().and_then(|g| g.revision.clone()),
        component_path: env("ANTIBODY_COMPONENT"),
        error,
        ..Default::default()
    };
    let resp = search(&cfg, &req, Duration::from_secs(15))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        match render_context(&resp) {
            Some(text) => println!("{text}"),
            None => println!("No relevant prior solutions found."),
        }
        if resp.degraded {
            println!(
                "(search was degraded: {})",
                resp.degraded_reasons.join(", ")
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_suggestions_prints_nothing() {
        assert!(render_hook_json(&HookOutput::empty()).is_none());
    }

    #[test]
    fn missing_config_fails_open_as_error() {
        // run_user_prompt returns Err (caller prints nothing, exits 0) without config.
        if std::env::var("ANTIBODY_URL").is_err() {
            assert!(run_user_prompt(Agent::Codex).is_err());
        }
    }
}
