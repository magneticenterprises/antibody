//! Pluggable model providers behind Rust traits.
//!
//! - `none`: no model at all. Ingestion and exact/full-text retrieval work.
//! - `hash`: a deterministic local feature-hashing embedder (no network). It
//!   captures lexical overlap only, but keeps the vector path exercised.
//! - `openai-compatible`: any `/v1/embeddings` + `/v1/chat/completions`
//!   endpoint (Ollama, vLLM, LM Studio, llama.cpp server, hosted APIs).
//! - `openai`, `gemini`, `deepseek`: presets of `openai-compatible` for those
//!   hosted APIs (DeepSeek has no embeddings).
//! - `anthropic`: the native Messages API, LLM only (no embeddings).
//!
//! Every provider declares where data goes (`DataLocation`). Callers must not
//! send private content to an `External` provider unless the organization
//! approved it.

pub mod anthropic;
pub mod config;
pub mod hash;
pub mod openai;

use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataLocation {
    /// Runs inside the deployment's trusted network.
    Local,
    /// Content leaves the deployment; requires administrator approval.
    External,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderDescriptor {
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub dimensions: Option<usize>,
    pub location: DataLocation,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider request timed out")]
    Timeout,
    #[error("provider unavailable: {0}")]
    Unavailable(String),
    #[error("provider returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("provider rejected the request: {0}")]
    Rejected(String),
}

impl ProviderError {
    /// Whether retrying later may succeed.
    pub fn is_transient(&self) -> bool {
        matches!(self, ProviderError::Timeout | ProviderError::Unavailable(_))
    }
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;
    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, ProviderError>;
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;
    /// Ask for a single JSON object. The output is untrusted and must be
    /// validated by the caller.
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, ProviderError>;
}

/// The configured providers for a process.
#[derive(Clone, Default)]
pub struct Providers {
    pub embedding: Option<Arc<dyn EmbeddingProvider>>,
    pub llm: Option<Arc<dyn LlmProvider>>,
}

impl Providers {
    pub fn none() -> Self {
        Self::default()
    }

    /// Embedding provider usable for this organization's content.
    pub fn embedding_for(&self, external_approved: bool) -> Option<Arc<dyn EmbeddingProvider>> {
        self.embedding
            .as_ref()
            .filter(|p| external_approved || p.descriptor().location == DataLocation::Local)
            .cloned()
    }

    pub fn llm_for(&self, external_approved: bool) -> Option<Arc<dyn LlmProvider>> {
        self.llm
            .as_ref()
            .filter(|p| external_approved || p.descriptor().location == DataLocation::Local)
            .cloned()
    }

    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "embedding": self.embedding.as_ref().map(|p| p.descriptor()),
            "llm": self.llm.as_ref().map(|p| p.descriptor()),
        })
    }
}

/// Cosine similarity for already-normalized or raw vectors.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}
