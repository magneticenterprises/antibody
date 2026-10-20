//! Generic signed CI events (`POST /v1/webhooks/ci`) for non-GitHub CI.
//!
//! Headers:
//! - `X-Antibody-Org`: organization slug
//! - `X-Antibody-Delivery`: unique delivery id (replay protection)
//! - `X-Antibody-Timestamp`: unix seconds; must be within the skew window
//! - `X-Antibody-Signature`: `sha256=` HMAC over `"{timestamp}.{raw body}"`

use crate::signing;

pub const MAX_SKEW_SECS: i64 = 300;

pub fn signed_message(timestamp: &str, body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(timestamp.len() + 1 + body.len());
    m.extend_from_slice(timestamp.as_bytes());
    m.push(b'.');
    m.extend_from_slice(body);
    m
}

pub fn sign(secret: &[u8], timestamp: &str, body: &[u8]) -> String {
    format!(
        "sha256={}",
        signing::sign(secret, &signed_message(timestamp, body))
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum CiSignatureError {
    BadTimestamp,
    Stale,
    BadSignature,
}

pub fn verify(
    secret: &[u8],
    timestamp: &str,
    body: &[u8],
    signature: &str,
    now_unix: i64,
) -> Result<(), CiSignatureError> {
    let ts: i64 = timestamp
        .trim()
        .parse()
        .map_err(|_| CiSignatureError::BadTimestamp)?;
    if (now_unix - ts).abs() > MAX_SKEW_SECS {
        return Err(CiSignatureError::Stale);
    }
    if !signing::verify_prefixed(secret, &signed_message(timestamp.trim(), body), signature) {
        return Err(CiSignatureError::BadSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_skew() {
        let sig = sign(b"k", "1000", b"{}");
        assert_eq!(verify(b"k", "1000", b"{}", &sig, 1100), Ok(()));
        assert_eq!(
            verify(b"k", "1000", b"{}", &sig, 2000),
            Err(CiSignatureError::Stale)
        );
        assert_eq!(
            verify(b"k", "1000", b"{ }", &sig, 1000),
            Err(CiSignatureError::BadSignature)
        );
        assert_eq!(
            verify(b"k", "x", b"{}", &sig, 1000),
            Err(CiSignatureError::BadTimestamp)
        );
    }
}
