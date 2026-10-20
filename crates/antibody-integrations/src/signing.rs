//! HMAC-SHA256 signing helpers. Verification always runs over the raw
//! request bytes and compares in constant time.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn sign(secret: &[u8], message: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message);
    hex::encode(mac.finalize().into_bytes())
}

/// Verify a `sha256=<hex>` style signature (GitHub `X-Hub-Signature-256`).
pub fn verify_prefixed(secret: &[u8], body: &[u8], header: &str) -> bool {
    let Some(hex_sig) = header.trim().strip_prefix("sha256=") else {
        return false;
    };
    verify_hex(secret, body, hex_sig)
}

pub fn verify_hex(secret: &[u8], message: &[u8], hex_sig: &str) -> bool {
    let Ok(expected) = hex::decode(hex_sig.trim()) else {
        return false;
    };
    if expected.len() != 32 {
        return false;
    }
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message);
    mac.verify_slice(&expected).is_ok()
}

/// Derive a per-purpose secret from the server master key, e.g. the GitHub
/// webhook secret for one organization. Nothing derived is stored.
pub fn derive_secret(master_key: &[u8], label: &str) -> String {
    sign(master_key, label.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_example_signature() {
        // Example from GitHub's webhook documentation.
        let secret = b"It's a Secret to Everybody";
        let body = b"Hello, World!";
        let header = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(verify_prefixed(secret, body, header));
        assert!(!verify_prefixed(secret, b"Hello, World?", header));
        assert!(!verify_prefixed(b"wrong", body, header));
        assert!(!verify_prefixed(secret, body, "sha1=abc"));
        assert!(!verify_prefixed(secret, body, "sha256=zz"));
    }

    #[test]
    fn derived_secrets_are_stable_and_distinct() {
        let a = derive_secret(b"master", "github:org1:v1");
        assert_eq!(a, derive_secret(b"master", "github:org1:v1"));
        assert_ne!(a, derive_secret(b"master", "github:org1:v2"));
        assert_ne!(a, derive_secret(b"other", "github:org1:v1"));
    }
}
