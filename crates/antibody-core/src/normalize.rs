//! Deterministic normalization used for exact matching, deduplication and
//! full-text queries: error signatures, symbols, paths and fingerprints.

use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::LazyLock;

const MAX_SIGNATURE_CHARS: usize = 240;

static RE_UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").unwrap()
});
static RE_HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b0x[0-9a-f]+\b").unwrap());
static RE_SHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b[0-9a-f]{12,64}\b").unwrap());
static RE_TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?\b")
        .unwrap()
});
// Paths such as /home/me/proj/src/lib.rs, C:\x\y.ts or ./a/b.py -> keep the basename.
static RE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[A-Za-z]:)?(?:\.{0,2}[/\\])?(?:[\w.@\-+]+[/\\])+([\w.@\-+]+)").unwrap()
});
static RE_LINECOL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":\d+(?::\d+)?\b").unwrap());
static RE_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{2,}\b").unwrap());
static RE_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap());

/// Lines that look like error output across common ecosystems.
static RE_ERROR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        (?:^|\s)(?i:error)(?:\[E\d{4}\])?:\s    # rustc / generic
        | \berror\s+TS\d{4}:                      # TypeScript
        | \b[A-Z][A-Za-z]+(?:Error|Exception)\b:? # Python / JS / Java exceptions
        | \bpanicked\s+at\b                       # Rust panics
        | ^panic:\s                               # Go panics
        | ^---\s+FAIL:                            # Go tests
        | \bFAILED\b                              # pytest / generic
        | ^fatal:\s                               # git
        | \bnpm\s+ERR!                            # npm
        | \bERR_[A-Z_]{3,}\b                      # node error codes
        | \bundefined\s+reference\s+to\b          # linkers
        | \bcannot\s+find\s+(?:module|symbol|value|type|crate)\b
        | \bSegmentation\s+fault\b
        | \bE\d{4}\b                              # error code references
        ",
    )
    .unwrap()
});

static RE_SYMBOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        \b[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+\b      # rust paths a::b
        | \b[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+\(\)  # calls a.b()
        | \b[a-z]+(?:[A-Z][a-z0-9]+)+\b                              # camelCase
        | \b[A-Z][a-z0-9]+(?:[A-Z][a-z0-9]+)+\b                      # PascalCase
        | \b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b                          # snake_case
        ",
    )
    .unwrap()
});

static RE_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[\w.@\-+]+/)*[\w@\-+]+\.(?:rs|ts|tsx|js|jsx|mjs|cjs|py|go|java|kt|rb|php|cs|cpp|cc|c|h|hpp|swift|scala|sql|toml|yaml|yml|json|lock|gradle|sh|tf|proto|vue|svelte)\b").unwrap()
});

/// Normalize an error line so the same failure on different machines,
/// checkouts and runs produces the same string.
pub fn normalize_error_signature(line: &str) -> String {
    let s = RE_ANSI.replace_all(line, "");
    let s = RE_TIMESTAMP.replace_all(&s, "<ts>");
    let s = RE_UUID.replace_all(&s, "<uuid>");
    let s = RE_HEX.replace_all(&s, "<hex>");
    let s = RE_PATH.replace_all(&s, "$1");
    let s = RE_LINECOL.replace_all(&s, ":<n>");
    let s = RE_SHA.replace_all(&s, "<sha>");
    let s = RE_NUMBER.replace_all(&s, "<n>");
    let s = RE_WS.replace_all(s.trim(), " ");
    let lowered = s.to_lowercase();
    let trimmed = lowered.trim_matches(|c: char| c == '-' || c == '=' || c == ' ');
    trimmed.chars().take(MAX_SIGNATURE_CHARS).collect()
}

/// Stable short hash of a normalized signature (used for exact matching).
pub fn signature_hash(normalized: &str) -> String {
    short_hash(normalized.as_bytes())
}

pub fn short_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(&digest[..16])
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Extract up to `max` distinct, normalized error signatures from free text.
pub fn extract_error_signatures(text: &str, max: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.len() < 6 || line.len() > 2000 {
            continue;
        }
        if !RE_ERROR_LINE.is_match(line) {
            continue;
        }
        let norm = normalize_error_signature(line);
        if norm.len() < 6 {
            continue;
        }
        if seen.insert(norm.clone()) {
            out.push(norm);
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

/// Extract identifier-like symbols (bounded, deduplicated, lowercase).
pub fn extract_symbols(text: &str, max: usize) -> Vec<String> {
    let mut set = BTreeSet::new();
    for m in RE_SYMBOL.find_iter(text) {
        let s = m.as_str().trim_end_matches("()");
        if s.len() >= 4 && s.len() <= 120 {
            set.insert(s.to_lowercase());
        }
        if set.len() >= max {
            break;
        }
    }
    set.into_iter().collect()
}

/// Extract repository-relative-looking file paths (bounded, deduplicated).
pub fn extract_paths(text: &str, max: usize) -> Vec<String> {
    let mut set = BTreeSet::new();
    for m in RE_FILE.find_iter(text) {
        let p = normalize_path(m.as_str());
        if !p.is_empty() && p.len() <= 200 {
            set.insert(p);
        }
        if set.len() >= max {
            break;
        }
    }
    set.into_iter().collect()
}

/// Strip leading `./`, absolute prefixes up to well-known roots, and backslashes.
pub fn normalize_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.trim_start_matches("./").trim_start_matches('/');
    p.to_lowercase()
}

/// Sanitized lexemes for building an OR-ed PostgreSQL tsquery. Only
/// `[a-z0-9_]` survives, so user text can never inject tsquery syntax.
pub fn query_terms(text: &str, max_terms: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        let w = word.to_ascii_lowercase();
        if w.len() < 2 || w.len() > 40 || w.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if STOPWORDS.contains(&w.as_str()) {
            continue;
        }
        if seen.insert(w.clone()) {
            out.push(w);
            if out.len() >= max_terms {
                break;
            }
        }
    }
    out
}

/// Build a tsquery string `a | b | c` from sanitized terms (empty if none).
pub fn or_tsquery(terms: &[String]) -> String {
    terms.join(" | ")
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "when", "what", "how", "why",
    "are", "was", "were", "has", "have", "not", "but", "can", "fix", "please", "help", "issue",
    "error", "need", "want", "make", "use", "using", "our", "its", "it", "is", "in", "on", "of",
    "to", "an", "be", "we", "you", "me", "my", "do", "does", "should", "would", "could", "there",
    "then",
];

/// Fingerprint of a task for retrieval analytics (not reversible to text).
pub fn task_fingerprint(task: &str, repository: Option<&str>) -> String {
    let mut terms = query_terms(task, 64);
    terms.sort();
    let basis = format!("{}|{}", repository.unwrap_or(""), terms.join(" "));
    short_hash(basis.as_bytes())
}

/// Lineage key: identifies "the same problem" regardless of applicability.
pub fn lineage_key(signatures: &[String], title: &str) -> String {
    if let Some(primary) = signatures.iter().min() {
        format!("sig:{}", signature_hash(primary))
    } else {
        let mut terms = query_terms(title, 12);
        terms.sort();
        format!("title:{}", short_hash(terms.join(" ").as_bytes()))
    }
}

/// Fingerprint: same problem *and* same applicability conditions.
pub fn solution_fingerprint(lineage: &str, applicability_key: &str) -> String {
    short_hash(format!("{lineage}|{applicability_key}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_normalization_is_machine_independent() {
        let a =
            "error[E0308]: mismatched types --> /home/alice/work/api/src/handlers/user.rs:42:17";
        let b = "error[E0308]: mismatched types --> /Users/bob/code/api/src/handlers/user.rs:7:3";
        assert_eq!(normalize_error_signature(a), normalize_error_signature(b));
        assert_eq!(
            normalize_error_signature(a),
            "error[e0308]: mismatched types --> user.rs:<n>"
        );
    }

    #[test]
    fn normalization_strips_volatile_tokens() {
        let a = "panicked at 0x7ffd1234 request 3f2a9c1e-1111-2222-3333-444455556666 at 2026-01-02T03:04:05Z";
        let n = normalize_error_signature(a);
        assert!(
            n.contains("<hex>") && n.contains("<uuid>") && n.contains("<ts>"),
            "{n}"
        );
    }

    #[test]
    fn extracts_signatures_from_mixed_output() {
        let log = "Compiling api v0.1.0\nerror[E0432]: unresolved import `crate::db`\n  --> src/main.rs:3:5\nok\nTypeError: Cannot read properties of undefined (reading 'id')\n";
        let sigs = extract_error_signatures(log, 10);
        assert_eq!(sigs.len(), 2, "{sigs:?}");
        assert!(sigs[0].starts_with("error[e0432]: unresolved import"));
        assert!(sigs[1].starts_with("typeerror: cannot read properties"));
    }

    #[test]
    fn query_terms_cannot_inject_tsquery_syntax() {
        let terms = query_terms("foo & !bar | (baz:*) 'quux' <-> zz", 10);
        for t in &terms {
            assert!(
                t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{t}"
            );
        }
        assert_eq!(or_tsquery(&terms), "foo | bar | baz | quux | zz");
    }

    #[test]
    fn extracts_symbols_and_paths() {
        let text =
            "the bug was in sqlx::PgPool and handleRequest inside src/db/pool.rs and Cargo.toml";
        let syms = extract_symbols(text, 10);
        assert!(syms.contains(&"sqlx::pgpool".to_string()));
        assert!(syms.contains(&"handlerequest".to_string()));
        let paths = extract_paths(text, 10);
        assert!(paths.contains(&"src/db/pool.rs".to_string()));
        assert!(paths.contains(&"cargo.toml".to_string()));
    }

    #[test]
    fn lineage_prefers_signatures() {
        let a = lineage_key(&["b".into(), "a".into()], "x");
        let b = lineage_key(&["a".into(), "b".into()], "y");
        assert_eq!(a, b);
        assert!(lineage_key(&[], "Fix login timeout").starts_with("title:"));
    }
}
