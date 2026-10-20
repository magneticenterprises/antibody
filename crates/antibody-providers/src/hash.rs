//! Deterministic local embedding via signed feature hashing of word unigrams,
//! bigrams and character trigrams. No network, no model download.

use crate::{DataLocation, EmbeddingProvider, ProviderDescriptor, ProviderError};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

pub struct HashEmbedding {
    descriptor: ProviderDescriptor,
    dims: usize,
}

impl HashEmbedding {
    pub fn new(dims: usize) -> Self {
        Self {
            dims,
            descriptor: ProviderDescriptor {
                provider: "local-hash".into(),
                model: "hash-ngram".into(),
                model_version: "1".into(),
                dimensions: Some(dims),
                location: DataLocation::Local,
            },
        }
    }

    fn add(&self, v: &mut [f32], feature: &str, weight: f32) {
        let h = Sha256::digest(feature.as_bytes());
        let idx = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as usize % self.dims;
        let sign = if h[4] & 1 == 0 { 1.0 } else { -1.0 };
        v[idx] += sign * weight;
    }

    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dims];
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|w| w.len() >= 2)
            .collect();
        for w in &words {
            self.add(&mut v, &format!("w:{w}"), 1.0);
            let chars: Vec<char> = format!("#{w}#").chars().collect();
            for tri in chars.windows(3) {
                self.add(
                    &mut v,
                    &format!("c:{}", tri.iter().collect::<String>()),
                    0.35,
                );
            }
        }
        for pair in words.windows(2) {
            self.add(&mut v, &format!("b:{} {}", pair[0], pair[1]), 0.7);
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }
}

#[async_trait]
impl EmbeddingProvider for HashEmbedding {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        Ok(inputs.iter().map(|t| self.embed_one(t)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cosine;

    #[test]
    fn similar_texts_are_closer_than_unrelated() {
        let e = HashEmbedding::new(384);
        let a = e.embed_one("connection pool timed out waiting for postgres connection");
        let b = e.embed_one("postgres pool timeout while waiting for a connection");
        let c = e.embed_one("css grid layout breaks on safari");
        assert!(
            cosine(&a, &b) > cosine(&a, &c) + 0.2,
            "{} vs {}",
            cosine(&a, &b),
            cosine(&a, &c)
        );
        assert_eq!(e.embed_one("same"), e.embed_one("same"));
    }
}
