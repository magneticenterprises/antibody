//! Request body extraction with typed errors (415/413/400) instead of
//! framework defaults, so every failure follows the API error contract.

use crate::error::ApiError;
use antibody_core::ErrorCode;
use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use serde::de::DeserializeOwned;

/// Raw JSON body bytes (content type and size checked).
pub struct JsonBytes(pub Bytes);

impl<S: Send + Sync> FromRequest<S> for JsonBytes {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = req
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| {
                let mime = ct
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                mime == "application/json" || mime.ends_with("+json")
            })
            .unwrap_or(false);
        if !is_json {
            return Err(ApiError::new(
                ErrorCode::UnsupportedMediaType,
                "Content-Type must be application/json",
            ));
        }
        let bytes = Bytes::from_request(req, state).await.map_err(|rejection| {
            if rejection.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::new(
                    ErrorCode::PayloadTooLarge,
                    "request body exceeds the size limit",
                )
            } else {
                ApiError::invalid("could not read request body")
            }
        })?;
        Ok(JsonBytes(bytes))
    }
}

/// Parsed JSON body.
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let JsonBytes(bytes) = JsonBytes::from_request(req, state).await?;
        parse_json(&bytes).map(ApiJson)
    }
}

pub fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes).map_err(|e| {
        // serde messages describe structure (field names, line/column), not values.
        ApiError::invalid(format!("invalid JSON body: {e}"))
    })
}
