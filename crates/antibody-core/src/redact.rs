//! Best-effort secret redaction applied before anything is stored.
//!
//! Redaction is pattern based and therefore incomplete: it catches common
//! credential formats, not every secret. `docs/security.md` documents this
//! and the leak-response procedure.

use regex::Regex;
use std::sync::LazyLock;

const CREDENTIAL_KEY: &str = r"(?:[A-Z0-9_]*_)?(?:password|passwd|pwd|secret|api[_-]?key|apikey|access[_-]?token|auth[_-]?token|client[_-]?secret|private[_-]?key|secret[_-]?key|signing[_-]?key|token|authorization)";
static CREDENTIAL_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i)^{CREDENTIAL_KEY}$")).unwrap());

struct Rule {
    kind: &'static str,
    re: Regex,
    /// Replacement template; `{R}` is replaced with the redaction marker.
    replacement: &'static str,
}

fn rule(kind: &'static str, pattern: &str, replacement: &'static str) -> Rule {
    Rule {
        kind,
        re: Regex::new(pattern).expect("valid redaction regex"),
        replacement,
    }
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule(
            "private_key",
            r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----",
            "{R}",
        ),
        rule("antibody_token", r"\babt_[A-Za-z0-9]{20,}\b", "{R}"),
        rule("aws_access_key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "{R}"),
        rule(
            "github_token",
            r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{22,})\b",
            "{R}",
        ),
        rule("gitlab_token", r"\bglpat-[A-Za-z0-9_\-]{20,}\b", "{R}"),
        rule("slack_token", r"\bxox[abposr]-[A-Za-z0-9\-]{10,}\b", "{R}"),
        rule("anthropic_key", r"\bsk-ant-[A-Za-z0-9_\-]{20,}", "{R}"),
        rule(
            "openai_key",
            r"\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_\-]{20,}",
            "{R}",
        ),
        rule("google_api_key", r"\bAIza[0-9A-Za-z_\-]{35}\b", "{R}"),
        rule(
            "stripe_key",
            r"\b(?:sk|rk|pk)_(?:live|test)_[0-9A-Za-z]{16,}\b",
            "{R}",
        ),
        rule("npm_token", r"\bnpm_[A-Za-z0-9]{36}\b", "{R}"),
        rule(
            "jwt",
            r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\b",
            "{R}",
        ),
        rule(
            "bearer_token",
            r"(?i)\b(bearer)\s+[A-Za-z0-9\-._~+/]{16,}=*",
            "$1 {R}",
        ),
        rule(
            "basic_auth_header",
            r"(?i)\b(authorization:\s*basic)\s+[A-Za-z0-9+/]{8,}=*",
            "$1 {R}",
        ),
        rule(
            "url_credentials",
            r"([A-Za-z][A-Za-z0-9+.\-]*://)[^\s:/@]+:[^\s@/]+@",
            "${1}{R}@",
        ),
        rule(
            "secret_assignment",
            &format!(r#"(?i)(\b{CREDENTIAL_KEY}\b["']?\s*[:=]\s*)(")(?:\\.|[^"\\]){{6,}}(")"#),
            "$1$2{R}$3",
        ),
        rule(
            "secret_assignment",
            &format!(r#"(?i)(\b{CREDENTIAL_KEY}\b["']?\s*[:=]\s*)(')(?:\\.|[^'\\]){{6,}}(')"#),
            "$1$2{R}$3",
        ),
        rule(
            "secret_assignment",
            &format!(r#"(?i)(\b{CREDENTIAL_KEY}\b["']?\s*[:=]\s*)[^\s"'{{}}\[\],;]{{6,}}"#),
            "$1{R}",
        ),
    ]
});

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redaction {
    pub text: String,
    pub count: usize,
    pub kinds: Vec<&'static str>,
}

/// Redact recognizable secrets, returning the cleaned text and statistics.
pub fn redact(input: &str) -> Redaction {
    let mut text = input.to_string();
    let mut count = 0;
    let mut kinds = Vec::new();
    for rule in RULES.iter() {
        let matches = rule.re.find_iter(&text).count();
        if matches == 0 {
            continue;
        }
        count += matches;
        kinds.push(rule.kind);
        let marker = format!("[REDACTED:{}]", rule.kind);
        let replacement = rule.replacement.replace("{R}", &marker);
        text = rule
            .re
            .replace_all(&text, replacement.as_str())
            .into_owned();
    }
    Redaction { text, count, kinds }
}

/// Redact strings and explicitly named credential values in JSON.
pub fn redact_json(value: &mut serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(s) => {
            let r = redact(s);
            if r.count > 0 {
                *s = r.text;
            }
            r.count
        }
        serde_json::Value::Array(items) => items.iter_mut().map(redact_json).sum(),
        serde_json::Value::Object(map) => map
            .iter_mut()
            .map(|(key, value)| {
                if CREDENTIAL_KEY_RE.is_match(key)
                    && !value.is_null()
                    && value.as_str() != Some("")
                    && !value
                        .as_str()
                        .is_some_and(|s| s.starts_with("[REDACTED:") && s.ends_with(']'))
                {
                    *value = serde_json::Value::String("[REDACTED:secret_assignment]".into());
                    1
                } else {
                    redact_json(value)
                }
            })
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_credentials() {
        let fake_gh = format!("ghp_{}", "a1B2".repeat(9));
        let fake_aws = "AKIAABCDEFGHIJKLMNOP";
        let input = format!(
            "export GITHUB_TOKEN={fake_gh}\naws key {fake_aws}\npostgres://app:hunter2secret@db:5432/x\nAuthorization: Bearer abcdefghijklmnopqrstuvwxyz012345\nDB_PASSWORD=\"s3cr3tvalue\"\n-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----"
        );
        let r = redact(&input);
        assert!(!r.text.contains(&fake_gh), "{}", r.text);
        assert!(!r.text.contains(fake_aws));
        assert!(!r.text.contains("hunter2secret"));
        assert!(!r.text.contains("abcdefghijklmnopqrstuvwxyz012345"));
        assert!(!r.text.contains("s3cr3tvalue"));
        assert!(!r.text.contains("MIIabc"));
        assert!(
            r.text
                .contains("postgres://[REDACTED:url_credentials]@db:5432/x"),
            "{}",
            r.text
        );
        assert!(
            r.text
                .contains("DB_PASSWORD=\"[REDACTED:secret_assignment]\""),
            "{}",
            r.text
        );
        assert!(r.count >= 6);
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let input = "error[E0308]: mismatched types in src/main.rs; password field is required";
        let r = redact(input);
        assert_eq!(r.count, 0);
        assert_eq!(r.text, input);
    }

    #[test]
    fn redacts_inside_json() {
        let mut v = serde_json::json!({"messages": [{"content": "token sk-ant-abcdefghijklmnopqrstuvwxyz"}]});
        assert_eq!(redact_json(&mut v), 1);
        assert!(v.to_string().contains("[REDACTED:anthropic_key]"));
    }
}
