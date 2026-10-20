//! Provider configuration from environment variables.
//!
//! Besides the generic `openai-compatible` adapter there are presets for
//! hosted APIs. A preset fills in the base URL (and a default model where one
//! is stable) and reads the vendor's usual key variable when
//! `ANTIBODY_*_API_KEY` is not set.

use crate::anthropic::{self, AnthropicChat, AnthropicConfig};
use crate::hash::HashEmbedding;
use crate::openai::{OpenAiChat, OpenAiCompatibleConfig, OpenAiEmbedding};
use crate::{DataLocation, Providers};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
#[error("invalid provider configuration: {0}")]
pub struct ConfigError(pub String);

/// Hosted OpenAI-compatible APIs.
struct Preset {
    base_url: &'static str,
    key_var: &'static str,
    /// `(model, dimensions)` used when `ANTIBODY_EMBEDDING_MODEL` is unset;
    /// `None` when the vendor has no embeddings API.
    embedding: Option<(&'static str, usize)>,
    has_chat: bool,
}

fn preset(name: &str) -> Option<Preset> {
    Some(match name {
        "openai" => Preset {
            base_url: "https://api.openai.com/v1",
            key_var: "OPENAI_API_KEY",
            embedding: Some(("text-embedding-3-small", 1536)),
            has_chat: true,
        },
        "gemini" => Preset {
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
            key_var: "GEMINI_API_KEY",
            embedding: Some(("gemini-embedding-001", 3072)),
            has_chat: true,
        },
        "deepseek" => Preset {
            base_url: "https://api.deepseek.com/v1",
            key_var: "DEEPSEEK_API_KEY",
            embedding: None,
            has_chat: true,
        },
        "anthropic" => Preset {
            base_url: anthropic::DEFAULT_BASE_URL,
            key_var: "ANTHROPIC_API_KEY",
            embedding: None,
            has_chat: false, // native adapter, handled separately
        },
        _ => return None,
    })
}

/// Infer data location from the URL host when not explicitly configured:
/// loopback, private and single-label (compose service) hosts are local.
pub fn infer_location(base_url: &str) -> DataLocation {
    let Ok(url) = url::Url::parse(base_url) else {
        return DataLocation::External;
    };
    match url.host() {
        Some(url::Host::Ipv4(ip)) if ip.is_loopback() || ip.is_private() || ip.is_link_local() => {
            DataLocation::Local
        }
        Some(url::Host::Ipv6(ip)) if ip.is_loopback() || ip.is_unique_local() => {
            DataLocation::Local
        }
        Some(url::Host::Domain(d))
            if d == "localhost"
                || !d.contains('.')
                || d.ends_with(".internal")
                || d.ends_with(".local") =>
        {
            DataLocation::Local
        }
        _ => DataLocation::External,
    }
}

/// Reads one trimmed, non-empty variable.
type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn required(get: Lookup, var: &str) -> Result<String, ConfigError> {
    get(var).ok_or_else(|| ConfigError(format!("{var} is required")))
}

fn location(get: Lookup, var: &str, base_url: &str) -> Result<DataLocation, ConfigError> {
    match get(var).as_deref() {
        None => Ok(infer_location(base_url)),
        Some("local") => Ok(DataLocation::Local),
        Some("external") => Ok(DataLocation::External),
        Some(other) => Err(ConfigError(format!(
            "{var} must be local or external, got {other}"
        ))),
    }
}

fn millis(get: Lookup, var: &str, default: u64) -> Duration {
    Duration::from_millis(get(var).and_then(|v| v.parse().ok()).unwrap_or(default))
}

/// `ANTIBODY_<KIND>_API_KEY`, else the preset's vendor variable.
fn api_key(get: Lookup, kind: &str, preset: &Preset) -> Result<String, ConfigError> {
    let var = format!("ANTIBODY_{kind}_API_KEY");
    get(&var)
        .or_else(|| get(preset.key_var))
        .ok_or_else(|| ConfigError(format!("{var} or {} is required", preset.key_var)))
}

/// Build providers from `ANTIBODY_EMBEDDING_*` and `ANTIBODY_LLM_*`.
pub fn from_env() -> Result<Providers, ConfigError> {
    from_lookup(&env)
}

pub fn from_lookup(get: Lookup) -> Result<Providers, ConfigError> {
    Ok(Providers {
        embedding: embedding(get)?,
        llm: llm(get)?,
    })
}

fn embedding(get: Lookup) -> Result<Option<Arc<dyn crate::EmbeddingProvider>>, ConfigError> {
    let name = get("ANTIBODY_EMBEDDING_PROVIDER").unwrap_or_else(|| "none".into());
    let dims_var = get("ANTIBODY_EMBEDDING_DIMENSIONS").and_then(|d| d.parse().ok());
    let (base_url, model, api_key, dims) = match name.as_str() {
        "none" => return Ok(None),
        "hash" => return Ok(Some(Arc::new(HashEmbedding::new(dims_var.unwrap_or(384))))),
        "openai-compatible" => (
            required(get, "ANTIBODY_EMBEDDING_BASE_URL")?,
            required(get, "ANTIBODY_EMBEDDING_MODEL")?,
            get("ANTIBODY_EMBEDDING_API_KEY"),
            dims_var,
        ),
        other => {
            let p = preset(other).ok_or_else(|| {
                ConfigError(format!("unknown ANTIBODY_EMBEDDING_PROVIDER: {other}"))
            })?;
            let Some((default_model, default_dims)) = p.embedding else {
                return Err(ConfigError(format!(
                    "{other} has no embeddings API; set ANTIBODY_EMBEDDING_PROVIDER to openai, gemini or a local model"
                )));
            };
            let model = get("ANTIBODY_EMBEDDING_MODEL");
            // Default dimensions only apply to the default model.
            let dims = dims_var.or(model.is_none().then_some(default_dims));
            (
                get("ANTIBODY_EMBEDDING_BASE_URL").unwrap_or_else(|| p.base_url.into()),
                model.unwrap_or_else(|| default_model.into()),
                Some(api_key(get, "EMBEDDING", &p)?),
                dims,
            )
        }
    };
    let cfg = OpenAiCompatibleConfig {
        location: location(get, "ANTIBODY_EMBEDDING_DATA_LOCATION", &base_url)?,
        base_url,
        model,
        api_key,
        timeout: millis(get, "ANTIBODY_EMBEDDING_TIMEOUT_MS", 10_000),
        json_mode: false,
        openai_reasoning: false,
    };
    Ok(Some(Arc::new(OpenAiEmbedding::new(cfg, dims))))
}

fn llm(get: Lookup) -> Result<Option<Arc<dyn crate::LlmProvider>>, ConfigError> {
    let name = get("ANTIBODY_LLM_PROVIDER").unwrap_or_else(|| "none".into());
    let timeout = millis(get, "ANTIBODY_LLM_TIMEOUT_MS", 90_000);
    let json_mode = get("ANTIBODY_LLM_JSON_MODE")
        .map(|v| v != "false")
        .unwrap_or(true);
    let (base_url, api_key) = match name.as_str() {
        "none" => return Ok(None),
        "openai-compatible" => (
            required(get, "ANTIBODY_LLM_BASE_URL")?,
            get("ANTIBODY_LLM_API_KEY"),
        ),
        "anthropic" => {
            let p = preset("anthropic").expect("anthropic preset");
            let base_url = get("ANTIBODY_LLM_BASE_URL").unwrap_or_else(|| p.base_url.into());
            let effort = get("ANTIBODY_LLM_EFFORT");
            if let Some(e) = &effort
                && !["low", "medium", "high", "xhigh", "max"].contains(&e.as_str())
            {
                return Err(ConfigError(format!(
                    "ANTIBODY_LLM_EFFORT must be low, medium, high, xhigh or max, got {e}"
                )));
            }
            let cfg = AnthropicConfig {
                location: location(get, "ANTIBODY_LLM_DATA_LOCATION", &base_url)?,
                api_key: api_key(get, "LLM", &p)?,
                model: get("ANTIBODY_LLM_MODEL").unwrap_or_else(|| anthropic::DEFAULT_MODEL.into()),
                base_url,
                timeout,
                effort,
            };
            return Ok(Some(Arc::new(AnthropicChat::new(cfg))));
        }
        other => {
            let p = preset(other)
                .filter(|p| p.has_chat)
                .ok_or_else(|| ConfigError(format!("unknown ANTIBODY_LLM_PROVIDER: {other}")))?;
            (
                get("ANTIBODY_LLM_BASE_URL").unwrap_or_else(|| p.base_url.into()),
                Some(api_key(get, "LLM", &p)?),
            )
        }
    };
    let cfg = OpenAiCompatibleConfig {
        location: location(get, "ANTIBODY_LLM_DATA_LOCATION", &base_url)?,
        base_url,
        model: required(get, "ANTIBODY_LLM_MODEL")?,
        api_key,
        timeout,
        json_mode,
        openai_reasoning: name == "openai",
    };
    Ok(Some(Arc::new(OpenAiChat::new(cfg))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn build(vars: &[(&str, &str)]) -> Result<Providers, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        from_lookup(&move |k| map.get(k).cloned())
    }

    fn describe(vars: &[(&str, &str)]) -> serde_json::Value {
        build(vars).unwrap().describe()
    }

    #[test]
    fn infers_local_hosts() {
        assert_eq!(
            infer_location("http://localhost:11434/v1"),
            DataLocation::Local
        );
        assert_eq!(
            infer_location("http://ollama:11434/v1"),
            DataLocation::Local
        );
        assert_eq!(infer_location("http://10.1.2.3/v1"), DataLocation::Local);
        assert_eq!(
            infer_location("https://api.example.com/v1"),
            DataLocation::External
        );
        assert_eq!(infer_location("not a url"), DataLocation::External);
    }

    #[test]
    fn anthropic_uses_vendor_key_and_default_model() {
        let d = describe(&[
            ("ANTIBODY_LLM_PROVIDER", "anthropic"),
            ("ANTHROPIC_API_KEY", "k"),
        ]);
        assert_eq!(d["llm"]["provider"], "anthropic");
        assert_eq!(d["llm"]["model"], anthropic::DEFAULT_MODEL);
        assert_eq!(d["llm"]["location"], "external");
    }

    #[test]
    fn presets_require_a_key() {
        let err = build(&[("ANTIBODY_LLM_PROVIDER", "anthropic")])
            .err()
            .unwrap();
        assert!(err.0.contains("ANTHROPIC_API_KEY"), "{err}");
        let err = build(&[
            ("ANTIBODY_LLM_PROVIDER", "deepseek"),
            ("ANTIBODY_LLM_MODEL", "m"),
        ])
        .err()
        .unwrap();
        assert!(err.0.contains("DEEPSEEK_API_KEY"), "{err}");
    }

    #[test]
    fn chat_presets_need_a_model() {
        for name in ["openai", "gemini", "deepseek"] {
            let err = build(&[
                ("ANTIBODY_LLM_PROVIDER", name),
                ("ANTIBODY_LLM_API_KEY", "k"),
            ])
            .err()
            .unwrap();
            assert!(err.0.contains("ANTIBODY_LLM_MODEL"), "{name}: {err}");
        }
        let d = describe(&[
            ("ANTIBODY_LLM_PROVIDER", "gemini"),
            ("GEMINI_API_KEY", "k"),
            ("ANTIBODY_LLM_MODEL", "m"),
        ]);
        assert_eq!(d["llm"]["model"], "m");
    }

    #[test]
    fn embedding_presets() {
        let d = describe(&[
            ("ANTIBODY_EMBEDDING_PROVIDER", "openai"),
            ("OPENAI_API_KEY", "k"),
        ]);
        assert_eq!(d["embedding"]["model"], "text-embedding-3-small");
        assert_eq!(d["embedding"]["dimensions"], 1536);
        let d = describe(&[
            ("ANTIBODY_EMBEDDING_PROVIDER", "gemini"),
            ("GEMINI_API_KEY", "k"),
            ("ANTIBODY_EMBEDDING_MODEL", "other"),
        ]);
        assert!(d["embedding"]["dimensions"].is_null());
        for name in ["anthropic", "deepseek"] {
            let err = build(&[
                ("ANTIBODY_EMBEDDING_PROVIDER", name),
                ("ANTIBODY_EMBEDDING_API_KEY", "k"),
            ])
            .err()
            .unwrap();
            assert!(err.0.contains("no embeddings API"), "{name}: {err}");
        }
    }

    #[test]
    fn rejects_bad_effort() {
        let err = build(&[
            ("ANTIBODY_LLM_PROVIDER", "anthropic"),
            ("ANTHROPIC_API_KEY", "k"),
            ("ANTIBODY_LLM_EFFORT", "turbo"),
        ])
        .err()
        .unwrap();
        assert!(err.0.contains("ANTIBODY_LLM_EFFORT"), "{err}");
    }
}
