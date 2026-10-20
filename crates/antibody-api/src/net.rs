//! Network policy: client address resolution, HTTPS enforcement for
//! untrusted networks, and keyed rate limiting.

use crate::auth::{bearer_token, cookie_value};
use crate::error::ApiError;
use crate::state::AppState;
use antibody_core::ErrorCode;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl FromStr for Cidr {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        let (ip, prefix) = match s.split_once('/') {
            Some((ip, p)) => (ip, p.parse::<u8>().map_err(|_| ())?),
            None => (s, if s.contains(':') { 128 } else { 32 }),
        };
        let addr: IpAddr = ip.parse().map_err(|_| ())?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        if prefix > max {
            return Err(());
        }
        Ok(Self { addr, prefix })
    }
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                (u128::from(net) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
        }
    }
}

/// Client IP: the socket peer, or the right-most X-Forwarded-For entry when
/// the API sits behind a trusted reverse proxy.
pub fn client_ip(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    trust_proxy: bool,
) -> Option<IpAddr> {
    if trust_proxy
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse().ok())
    {
        return Some(ip);
    }
    peer.map(|p| p.ip())
}

fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

/// Reject plain-HTTP requests from outside trusted networks.
pub async fn transport_guard(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = req.uri().path();
    if path.starts_with("/v1/health/") {
        return Ok(next.run(req).await);
    }
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let https = state.config.trust_proxy && forwarded_https(req.headers());
    if !https {
        let ip = client_ip(req.headers(), peer, state.config.trust_proxy);
        let trusted = ip.is_some_and(|ip| {
            state
                .config
                .http_trusted_networks
                .iter()
                .any(|n| n.contains(ip))
        });
        // Requests without connection info only happen in-process (tests).
        if peer.is_some() && !trusted {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "HTTPS is required outside trusted networks",
            ));
        }
    }
    Ok(next.run(req).await)
}

pub struct RateLimiters {
    pub general: DefaultKeyedRateLimiter<String>,
    pub login: DefaultKeyedRateLimiter<String>,
    started_at: Instant,
    next_cleanup_secs: AtomicU64,
}

impl RateLimiters {
    pub fn new(per_minute: u32, login_per_minute: u32) -> Self {
        let q = |n: u32| {
            Quota::per_minute(NonZeroU32::new(n.max(1)).expect("non-zero"))
                .allow_burst(NonZeroU32::new((n / 4).max(5)).expect("non-zero"))
        };
        Self {
            general: RateLimiter::keyed(q(per_minute)),
            login: RateLimiter::keyed(q(login_per_minute)),
            started_at: Instant::now(),
            next_cleanup_secs: AtomicU64::new(60),
        }
    }

    /// Remove fully replenished buckets at most once per minute. Request-driven
    /// cleanup needs no background task and also covers the login limiter.
    fn prune_if_due(&self) {
        let elapsed = self.started_at.elapsed().as_secs();
        let next = self.next_cleanup_secs.load(Ordering::Relaxed);
        if elapsed >= next
            && self
                .next_cleanup_secs
                .compare_exchange(
                    next,
                    elapsed.saturating_add(60),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
        {
            self.general.retain_recent();
            self.login.retain_recent();
        }
    }
}

fn credential_key(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    // Use exactly the credential parser used by authentication. Scheme casing,
    // bearer whitespace and unrelated cookies must not create fresh buckets.
    let (kind, secret) = if let Some(token) = bearer_token(headers) {
        ("auth", token)
    } else {
        ("session", cookie_value(headers, cookie_name)?)
    };
    if secret.is_empty() || secret.len() > 128 {
        return None;
    }
    Some(format!(
        "{kind}:{}",
        hex::encode(Sha256::digest(secret.as_bytes()))
    ))
}

/// Enforce the shared client-IP budget before the canonical credential budget.
/// Arbitrary or rotating credentials cannot bypass unauthenticated protection.
pub async fn rate_limit(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let headers = req.headers();
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    state.limiters.prune_if_due();
    let ip_key = format!(
        "ip:{}",
        client_ip(headers, peer, state.config.trust_proxy)
            .map(|i| i.to_string())
            .unwrap_or_default()
    );
    if state.limiters.general.check_key(&ip_key).is_err()
        || credential_key(headers, state.config.cookie_name())
            .is_some_and(|key| state.limiters.general.check_key(&key).is_err())
    {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            "rate limit exceeded; retry later",
        ));
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cidr_matching() {
        let n: Cidr = "10.0.0.0/8".parse().unwrap();
        assert!(n.contains("10.1.2.3".parse().unwrap()));
        assert!(!n.contains("11.0.0.1".parse().unwrap()));
        assert!(n.contains("::ffff:10.0.0.1".parse().unwrap()));
        let l: Cidr = "::1/128".parse().unwrap();
        assert!(l.contains("::1".parse().unwrap()));
        assert!("1.2.3.4/33".parse::<Cidr>().is_err());
        let any: Cidr = "0.0.0.0/0".parse().unwrap();
        assert!(any.contains("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn forwarded_for_uses_last_hop() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.1.1.1, 2.2.2.2".parse().unwrap());
        assert_eq!(client_ip(&h, None, true), Some("2.2.2.2".parse().unwrap()));
        assert_eq!(
            client_ip(&h, Some("9.9.9.9:1".parse().unwrap()), false),
            Some("9.9.9.9".parse().unwrap())
        );
    }

    #[test]
    fn credentials_ignore_header_formatting_and_unrelated_cookies() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer abt_example".parse().unwrap());
        let key = credential_key(&h, "antibody_session");
        h.insert("authorization", "bEaReR    abt_example  ".parse().unwrap());
        assert_eq!(credential_key(&h, "antibody_session"), key);
        h.remove("authorization");
        h.insert(
            "cookie",
            "other=one; antibody_session=session-id".parse().unwrap(),
        );
        let key = credential_key(&h, "antibody_session");
        h.insert(
            "cookie",
            "antibody_session=session-id; other=two".parse().unwrap(),
        );
        assert_eq!(credential_key(&h, "antibody_session"), key);
        h.insert("cookie", "other=three".parse().unwrap());
        assert_eq!(credential_key(&h, "antibody_session"), None);
    }

    #[test]
    fn cleanup_reclaims_replenished_general_and_login_buckets() {
        let mut limiters = RateLimiters::new(60_000, 60_000);
        assert!(limiters.general.check_key(&"old-ip".into()).is_ok());
        assert!(limiters.login.check_key(&"old-login-ip".into()).is_ok());
        assert_eq!(limiters.general.len(), 1);
        assert_eq!(limiters.login.len(), 1);
        std::thread::sleep(Duration::from_millis(10));
        limiters.started_at = Instant::now() - Duration::from_secs(61);
        limiters.prune_if_due();
        assert!(limiters.general.is_empty());
        assert!(limiters.login.is_empty());
    }
}
