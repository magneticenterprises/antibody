//! Anthropic Messages API adapter (`POST /v1/messages`) for LLM extraction.
//! Anthropic has no embeddings endpoint; pair it with another embedding
//! provider.

use crate::openai::{check_status, client, map_err};
use crate::{DataLocation, LlmProvider, ProviderDescriptor, ProviderError};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const API_VERSION: &str = "2023-06-01";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Thinking counts toward `max_tokens`, so never ask for less than this.
const MIN_MAX_TOKENS: u32 = 16_000;

#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    /// Base URL without the `/v1` prefix, e.g. `https://api.anthropic.com`.
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub location: DataLocation,
    pub timeout: Duration,
    /// `output_config.effort` (`low` .. `max`); `None` uses the model default.
    pub effort: Option<String>,
}

pub struct AnthropicChat {
    cfg: AnthropicConfig,
    http: reqwest::Client,
    descriptor: ProviderDescriptor,
}

impl AnthropicChat {
    pub fn new(cfg: AnthropicConfig) -> Self {
        let descriptor = ProviderDescriptor {
            provider: "anthropic".into(),
            model: cfg.model.clone(),
            model_version: String::new(),
            dimensions: None,
            location: cfg.location,
        };
        Self {
            http: client(cfg.timeout),
            cfg,
            descriptor,
        }
    }
}

#[derive(Deserialize)]
struct MessageResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl LlmProvider for AnthropicChat {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, ProviderError> {
        let url = format!("{}/v1/messages", self.cfg.base_url.trim_end_matches('/'));
        let mut body = serde_json::json!({
            "model": self.cfg.model,
            "max_tokens": max_tokens.max(MIN_MAX_TOKENS),
            "system": format!("{system}\n\nRespond with a single JSON object and nothing else."),
            "messages": [{ "role": "user", "content": user }],
            // On a safety decline, re-run on a fallback model chosen by the API.
            "fallbacks": "default",
        });
        if let Some(effort) = &self.cfg.effort {
            body["output_config"] = serde_json::json!({ "effort": effort });
        }
        let req = self
            .http
            .post(url)
            .header("x-api-key", &self.cfg.api_key)
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .json(&body);
        let resp = check_status(req.send().await.map_err(map_err)?).await?;
        let parsed: MessageResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;
        match parsed.stop_reason.as_deref() {
            Some("refusal") => return Err(ProviderError::Rejected("model declined".into())),
            Some("max_tokens") => {
                return Err(ProviderError::InvalidResponse(
                    "output hit max_tokens".into(),
                ));
            }
            _ => {}
        }
        // Thinking blocks are skipped; only the final text is the answer.
        let text: String = parsed
            .content
            .into_iter()
            .filter(|b| b.kind == "text")
            .filter_map(|b| b.text)
            .collect();
        if text.trim().is_empty() {
            return Err(ProviderError::InvalidResponse("empty completion".into()));
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::HeaderMap, routing::post};

    async fn spawn(stop_reason: &'static str) -> String {
        let app = Router::new().route(
            "/v1/messages",
            post(
                move |headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(headers["x-api-key"], "k");
                    assert_eq!(headers["anthropic-version"], API_VERSION);
                    assert!(body["max_tokens"].as_u64().unwrap() >= MIN_MAX_TOKENS as u64);
                    assert_eq!(body["output_config"]["effort"], "low");
                    assert!(body.get("temperature").is_none());
                    Json(serde_json::json!({
                        "stop_reason": stop_reason,
                        "content": [
                            { "type": "thinking", "thinking": "" },
                            { "type": "text", "text": "{\"ok\":true}" },
                        ],
                    }))
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn chat(base_url: String) -> AnthropicChat {
        AnthropicChat::new(AnthropicConfig {
            base_url,
            model: DEFAULT_MODEL.into(),
            api_key: "k".into(),
            location: DataLocation::External,
            timeout: Duration::from_secs(5),
            effort: Some("low".into()),
        })
    }

    #[tokio::test]
    async fn returns_text_and_skips_thinking() {
        let c = chat(spawn("end_turn").await);
        assert_eq!(
            c.complete_json("s", "u", 100).await.unwrap(),
            "{\"ok\":true}"
        );
    }

    #[tokio::test]
    async fn refusal_is_not_retried() {
        let c = chat(spawn("refusal").await);
        let err = c.complete_json("s", "u", 100).await.unwrap_err();
        assert!(matches!(err, ProviderError::Rejected(_)), "{err:?}");
    }
}
