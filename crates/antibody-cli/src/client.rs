//! Thin blocking HTTP helpers.

use crate::config::Config;
use anyhow::{Context, Result};
use reqwest::blocking::{Client, RequestBuilder};
use std::time::Duration;

pub fn http(timeout: Duration) -> Result<Client> {
    Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(5)))
        .user_agent(concat!("antibody-cli/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building HTTP client")
}

pub fn url(cfg: &Config, path: &str) -> String {
    format!("{}{}", cfg.base_url, path)
}

pub fn authed(rb: RequestBuilder, cfg: &Config) -> RequestBuilder {
    rb.bearer_auth(&cfg.token)
}
