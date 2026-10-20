//! Authentication extractors and helpers.
//!
//! - API clients (hooks, MCP, CI) use `Authorization: Bearer abt_…` tokens.
//! - The browser dashboard uses an HttpOnly, SameSite=Strict session cookie;
//!   mutating cookie requests require `X-CSRF-Token` and a same-origin
//!   `Origin` header when present. Long-lived tokens never touch the browser.

use crate::error::ApiError;
use crate::state::AppState;
use antibody_core::ErrorCode;
use antibody_core::model::Scope;
use antibody_db::access::{AccessScope, AuthContext, AuthMethod};
use antibody_db::tokens;
use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use subtle::ConstantTimeEq;

/// An authenticated caller.
#[derive(Debug, Clone)]
pub struct Auth(pub AuthContext);

impl Auth {
    pub fn require(&self, scope: Scope) -> Result<(), ApiError> {
        if self.0.has_scope(scope) {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "this credential lacks the {scope} scope"
            )))
        }
    }

    pub fn require_any(&self, scopes: &[Scope]) -> Result<(), ApiError> {
        if scopes.iter().any(|s| self.0.has_scope(*s)) {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "this credential lacks the required scope",
            ))
        }
    }

    pub fn require_admin(&self) -> Result<(), ApiError> {
        if self.0.is_admin() {
            Ok(())
        } else {
            Err(ApiError::forbidden("administrator access required"))
        }
    }

    pub async fn scope(&self, state: &AppState) -> Result<AccessScope, ApiError> {
        Ok(antibody_db::access::access_scope(&state.pool, &self.0).await?)
    }
}

pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

pub fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

fn is_safe_method(m: &Method) -> bool {
    matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// CSRF and Origin checks for cookie-authenticated mutating requests.
pub fn check_csrf(state: &AppState, parts: &Parts, expected: &str) -> Result<(), ApiError> {
    if is_safe_method(&parts.method) {
        return Ok(());
    }
    if let Some(origin) = parts
        .headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        && origin != state.config.public_origin()
    {
        return Err(ApiError::new(
            ErrorCode::CsrfFailed,
            "cross-origin request rejected",
        ));
    }
    if let Some(site) = parts
        .headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        && site != "same-origin"
        && site != "none"
    {
        return Err(ApiError::new(
            ErrorCode::CsrfFailed,
            "cross-site request rejected",
        ));
    }
    let provided = parts
        .headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided.is_empty() || !bool::from(provided.as_bytes().ct_eq(expected.as_bytes())) {
        return Err(ApiError::new(
            ErrorCode::CsrfFailed,
            "missing or invalid CSRF token",
        ));
    }
    Ok(())
}

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(ctx) = parts.extensions.get::<AuthContext>() {
            return Ok(Auth(ctx.clone()));
        }
        if let Some(token) = bearer_token(&parts.headers) {
            let ctx = tokens::authenticate_token(&state.pool, token)
                .await?
                .ok_or_else(ApiError::unauthenticated)?;
            parts.extensions.insert(ctx.clone());
            return Ok(Auth(ctx));
        }
        if let Some(sid) = cookie_value(&parts.headers, state.config.cookie_name()) {
            let ctx = tokens::authenticate_web_session(&state.pool, sid)
                .await?
                .ok_or_else(ApiError::unauthenticated)?;
            if let AuthMethod::BrowserSession { csrf_token, .. } = &ctx.method {
                check_csrf(state, parts, csrf_token)?;
            }
            parts.extensions.insert(ctx.clone());
            return Ok(Auth(ctx));
        }
        Err(ApiError::unauthenticated())
    }
}

pub fn hash_password(password: &str) -> Result<String, ApiError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|_| ApiError::internal())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// A dummy hash so unknown usernames cost the same as wrong passwords.
pub fn dummy_verify(password: &str) {
    static DUMMY: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| hash_password("antibody-timing-equalizer").unwrap_or_default());
    let _ = verify_password(password, &DUMMY);
}

pub fn validate_new_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < 12 || password.len() > 256 {
        return Err(ApiError::invalid(
            "password must contain at least 12 characters and at most 256 bytes",
        ));
    }
    Ok(())
}

pub fn validate_username(username: &str) -> Result<(), ApiError> {
    if username.is_empty()
        || username.len() > 128
        || !username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._@-".contains(c))
    {
        return Err(ApiError::invalid(
            "username must be 1-128 chars of [A-Za-z0-9._@-]",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_round_trip() {
        let h = hash_password("correct horse battery staple").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn parses_bearer_and_cookies() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer abt_x".parse().unwrap());
        h.insert("cookie", "a=1; antibody_session=xyz; b=2".parse().unwrap());
        assert_eq!(bearer_token(&h), Some("abt_x"));
        assert_eq!(cookie_value(&h, "antibody_session"), Some("xyz"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }

    #[test]
    fn password_validation_uses_character_minimum_and_byte_maximum() {
        assert!(validate_new_password(&"x".repeat(11)).is_err());
        assert!(validate_new_password(&"x".repeat(256)).is_ok());
        assert!(validate_new_password(&"x".repeat(257)).is_err());
        assert!(validate_new_password(&"界".repeat(12)).is_ok());
        assert!(validate_new_password(&"界".repeat(86)).is_err());
    }
}
