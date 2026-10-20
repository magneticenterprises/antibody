use antibody_core::ErrorCode;
use antibody_core::api::{ErrorBody, ErrorDetail};
use antibody_core::error::DomainError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

tokio::task_local! {
    pub static REQUEST_ID: String;
}

/// API error: a typed code and a caller-safe message. Internal details are
/// logged, never returned.
#[derive(Debug)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn unauthenticated() -> Self {
        Self::new(ErrorCode::Unauthenticated, "missing or invalid credentials")
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, msg)
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, msg)
    }
    pub fn not_found(what: &str) -> Self {
        Self::new(ErrorCode::NotFound, format!("{what} not found"))
    }
    pub fn internal() -> Self {
        Self::new(ErrorCode::Internal, "internal error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.code.http_status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let request_id = REQUEST_ID.try_with(|id| id.clone()).ok();
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: self.message,
                request_id,
            },
        };
        let mut resp = (status, axum::Json(body)).into_response();
        if self.code == ErrorCode::RateLimited {
            resp.headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("10"));
        }
        if self.code == ErrorCode::Unauthenticated {
            resp.headers_mut().insert(
                "www-authenticate",
                axum::http::HeaderValue::from_static("Bearer"),
            );
        }
        resp
    }
}

impl From<DomainError> for ApiError {
    fn from(e: DomainError) -> Self {
        Self::new(e.code, e.message)
    }
}

impl From<antibody_db::DbError> for ApiError {
    fn from(e: antibody_db::DbError) -> Self {
        match e {
            antibody_db::DbError::Domain(d) => d.into(),
            other => {
                tracing::error!(error = %other, "database error");
                Self::internal()
            }
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        Self::internal()
    }
}

impl From<antibody_search::SearchError> for ApiError {
    fn from(e: antibody_search::SearchError) -> Self {
        match e {
            antibody_search::SearchError::Domain(d) => d.into(),
            antibody_search::SearchError::Db(db) => db.into(),
            other => {
                tracing::error!(error = %other, "search error");
                Self::internal()
            }
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
