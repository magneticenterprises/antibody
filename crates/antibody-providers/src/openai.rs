//! OpenAI-compatible HTTP adapters (`/embeddings`, `/chat/completions`).
//! Works with Ollama, vLLM, LM Studio, llama.cpp server and hosted APIs.

use crate::{DataLocation, EmbeddingProvider, LlmProvider, ProviderDescriptor, ProviderError};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    /// Base URL including the API prefix, e.g. `http://ollama:11434/v1`.
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub location: DataLocation,
    pub timeout: Duration,
    /// Request `response_format: json_object` (not all servers support it).
    pub json_mode: bool,
    /// OpenAI's own API: send `max_completion_tokens` (which includes
    /// reasoning tokens) and no `temperature`, which reasoning models reject.
    pub openai_reasoning: bool,
}

pub(crate) fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(3))
        .build()
        .expect("reqwest client")
}

pub(crate) fn map_err(e: reqwest::Error) -> ProviderError {
    if e.is_timeout() {
        ProviderError::Timeout
    } else {
        // Never include request bodies (private content) in errors.
        ProviderError::Unavailable(e.without_url().to_string())
    }
}

pub(crate) async fn check_status(
    resp: reqwest::Response,
) -> Result<reqwest::Response, ProviderError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    if status.is_server_error() || status.as_u16() == 429 {
        Err(ProviderError::Unavailable(format!("HTTP {status}")))
    } else {
        Err(ProviderError::Rejected(format!("HTTP {status}")))
    }
}

pub struct OpenAiEmbedding {
    cfg: OpenAiCompatibleConfig,
    http: reqwest::Client,
    descriptor: ProviderDescriptor,
}

impl OpenAiEmbedding {
    pub fn new(cfg: OpenAiCompatibleConfig, dimensions: Option<usize>) -> Self {
        let descriptor = ProviderDescriptor {
            provider: "openai-compatible".into(),
            model: cfg.model.clone(),
            model_version: String::new(),
            dimensions,
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
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}
#[derive(Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
    #[serde(default)]
    index: Option<usize>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbedding {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        let url = format!("{}/embeddings", self.cfg.base_url.trim_end_matches('/'));
        let mut req = self
            .http
            .post(url)
            .json(&serde_json::json!({ "model": self.cfg.model, "input": inputs }));
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        let resp = check_status(req.send().await.map_err(map_err)?).await?;
        let mut body: EmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;
        if body.data.len() != inputs.len() {
            return Err(ProviderError::InvalidResponse(
                "embedding count mismatch".into(),
            ));
        }
        body.data.sort_by_key(|d| d.index.unwrap_or(0));
        let out: Vec<Vec<f32>> = body.data.into_iter().map(|d| d.embedding).collect();
        if let Some(dims) = self.descriptor.dimensions
            && out.iter().any(|v| v.len() != dims)
        {
            return Err(ProviderError::InvalidResponse(
                "unexpected embedding dimensions".into(),
            ));
        }
        if out
            .iter()
            .any(|v| v.is_empty() || v.len() > 16000 || v.iter().any(|x| !x.is_finite()))
        {
            return Err(ProviderError::InvalidResponse(
                "invalid embedding vector".into(),
            ));
        }
        Ok(out)
    }
}

pub struct OpenAiChat {
    cfg: OpenAiCompatibleConfig,
    http: reqwest::Client,
    descriptor: ProviderDescriptor,
}

impl OpenAiChat {
    pub fn new(cfg: OpenAiCompatibleConfig) -> Self {
        let descriptor = ProviderDescriptor {
            provider: "openai-compatible".into(),
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
struct ChatResponse {
    choices: Vec<ChatChoice>,
}
#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}
#[derive(Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: Option<String>,
}

#[async_trait]
impl LlmProvider for OpenAiChat {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, ProviderError> {
        let url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        let mut body = serde_json::json!({
            "model": self.cfg.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        if self.cfg.openai_reasoning {
            body["max_completion_tokens"] = max_tokens.max(16_000).into();
        } else {
            body["temperature"] = 0.into();
            body["max_tokens"] = max_tokens.into();
        }
        if self.cfg.json_mode {
            body["response_format"] = serde_json::json!({ "type": "json_object" });
        }
        let mut req = self.http.post(url).json(&body);
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        let resp = check_status(req.send().await.map_err(map_err)?).await?;
        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;
        parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| ProviderError::InvalidResponse("empty completion".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};

    async fn spawn() -> String {
        let app = Router::new()
            .route(
                "/v1/embeddings",
                post(|Json(body): Json<serde_json::Value>| async move {
                    let n = body["input"].as_array().map(|a| a.len()).unwrap_or(0);
                    let data: Vec<_> = (0..n)
                        .rev()
                        .map(|i| serde_json::json!({ "index": i, "embedding": [i as f32, 1.0, 0.0] }))
                        .collect();
                    Json(serde_json::json!({ "data": data }))
                }),
            )
            .route(
                "/v1/chat/completions",
                post(|Json(_): Json<serde_json::Value>| async move {
                    Json(serde_json::json!({ "choices": [{ "message": { "content": "{\"ok\":true}" } }] }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/v1")
    }

    fn cfg(base_url: String) -> OpenAiCompatibleConfig {
        OpenAiCompatibleConfig {
            base_url,
            model: "m".into(),
            api_key: None,
            location: DataLocation::Local,
            timeout: Duration::from_secs(5),
            json_mode: true,
            openai_reasoning: false,
        }
    }

    #[tokio::test]
    async fn embeddings_are_reordered_by_index() {
        let base = spawn().await;
        let e = OpenAiEmbedding::new(cfg(base), Some(3));
        let out = e.embed(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(out[0][0], 0.0);
        assert_eq!(out[1][0], 1.0);
    }

    #[tokio::test]
    async fn chat_returns_content() {
        let base = spawn().await;
        let c = OpenAiChat::new(cfg(base));
        assert_eq!(
            c.complete_json("s", "u", 100).await.unwrap(),
            "{\"ok\":true}"
        );
    }

    #[tokio::test]
    async fn unreachable_provider_is_transient() {
        let e = OpenAiEmbedding::new(cfg("http://127.0.0.1:9/v1".into()), None);
        let err = e.embed(&["a".into()]).await.unwrap_err();
        assert!(err.is_transient(), "{err:?}");
    }
}
