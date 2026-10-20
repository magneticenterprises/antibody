use serde::{Deserialize, Serialize};

/// Typed error codes shared by the HTTP API and MCP tools.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Missing, malformed, expired or revoked credentials.
    Unauthenticated,
    /// Authenticated but lacking the required scope or role.
    Forbidden,
    /// The repository does not exist or the caller cannot access it. The two
    /// cases are deliberately indistinguishable.
    RepositoryAccessDenied,
    NotFound,
    InvalidRequest,
    PayloadTooLarge,
    UnsupportedMediaType,
    IdempotencyKeyRequired,
    /// The idempotency key was reused with a different request body.
    IdempotencyConflict,
    InvalidStateTransition,
    InvalidSignature,
    ReplayedDelivery,
    CsrfFailed,
    RateLimited,
    Conflict,
    ServiceUnavailable,
    Internal,
}

impl ErrorCode {
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::Unauthenticated | ErrorCode::InvalidSignature => 401,
            ErrorCode::Forbidden | ErrorCode::RepositoryAccessDenied | ErrorCode::CsrfFailed => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::InvalidRequest | ErrorCode::IdempotencyKeyRequired => 400,
            ErrorCode::PayloadTooLarge => 413,
            ErrorCode::UnsupportedMediaType => 415,
            ErrorCode::IdempotencyConflict
            | ErrorCode::InvalidStateTransition
            | ErrorCode::Conflict
            | ErrorCode::ReplayedDelivery => 409,
            ErrorCode::RateLimited => 429,
            ErrorCode::ServiceUnavailable => 503,
            ErrorCode::Internal => 500,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Unauthenticated => "unauthenticated",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::RepositoryAccessDenied => "repository_access_denied",
            ErrorCode::NotFound => "not_found",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::PayloadTooLarge => "payload_too_large",
            ErrorCode::UnsupportedMediaType => "unsupported_media_type",
            ErrorCode::IdempotencyKeyRequired => "idempotency_key_required",
            ErrorCode::IdempotencyConflict => "idempotency_conflict",
            ErrorCode::InvalidStateTransition => "invalid_state_transition",
            ErrorCode::InvalidSignature => "invalid_signature",
            ErrorCode::ReplayedDelivery => "replayed_delivery",
            ErrorCode::CsrfFailed => "csrf_failed",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::Conflict => "conflict",
            ErrorCode::ServiceUnavailable => "service_unavailable",
            ErrorCode::Internal => "internal",
        }
    }
}

/// A domain error carrying a typed code and a caller-safe message.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct DomainError {
    pub code: ErrorCode,
    pub message: String,
}

impl DomainError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }
    pub fn not_found(what: &str) -> Self {
        Self::new(ErrorCode::NotFound, format!("{what} not found"))
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, message)
    }
    pub fn repository_denied() -> Self {
        Self::new(
            ErrorCode::RepositoryAccessDenied,
            "repository not found or not accessible with these credentials",
        )
    }
}
