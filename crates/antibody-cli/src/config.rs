//! Environment-based configuration. Secrets are never printed or logged.

use anyhow::{Result, anyhow};
use std::path::PathBuf;

pub fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[derive(Clone)]
pub struct Config {
    pub base_url: String,
    pub token: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let base = env("ANTIBODY_URL").ok_or_else(|| anyhow!("ANTIBODY_URL is not set"))?;
        let token = env("ANTIBODY_TOKEN").ok_or_else(|| anyhow!("ANTIBODY_TOKEN is not set"))?;
        Ok(Self {
            base_url: base.trim_end_matches('/').to_string(),
            token,
        })
    }
}

/// Directory for the spool and upload log. `ANTIBODY_HOME` overrides `~/.antibody`.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(d) = env("ANTIBODY_HOME") {
        return Some(PathBuf::from(d));
    }
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".antibody"))
}
