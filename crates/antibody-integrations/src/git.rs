//! Git heuristics: revert detection and dependency manifest changes.

use regex::Regex;
use std::sync::LazyLock;

static RE_REVERTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)This reverts commit ([0-9a-f]{7,40})").unwrap());

/// Commits reverted by a commit message (git's default revert template).
pub fn reverted_commits(message: &str) -> Vec<String> {
    RE_REVERTS
        .captures_iter(message)
        .map(|c| c[1].to_lowercase())
        .collect()
}

/// Whether a commit message looks like a revert even without the trailer.
pub fn looks_like_revert(message: &str) -> bool {
    message.trim_start().starts_with("Revert \"") || !reverted_commits(message).is_empty()
}

const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "go.mod",
    "go.sum",
    "requirements.txt",
    "pyproject.toml",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "Gemfile",
    "Gemfile.lock",
    "composer.json",
    "composer.lock",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "gradle.lockfile",
    "mix.lock",
    "Package.resolved",
    "flake.lock",
];

/// Whether a changed path is a dependency manifest or lockfile.
pub fn is_dependency_manifest(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    MANIFESTS.contains(&file) || (file.starts_with("requirements") && file.ends_with(".txt"))
}

/// Directory prefix of a manifest path ("" for the repository root).
pub fn manifest_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_reverts() {
        let msg = "Revert \"Fix pool timeout\"\n\nThis reverts commit 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b.";
        assert_eq!(
            reverted_commits(msg),
            vec!["1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b"]
        );
        assert!(looks_like_revert(msg));
        assert!(!looks_like_revert("Fix pool timeout"));
    }

    #[test]
    fn detects_manifests() {
        assert!(is_dependency_manifest("services/api/Cargo.lock"));
        assert!(is_dependency_manifest("requirements-dev.txt"));
        assert!(!is_dependency_manifest("src/cargo.rs"));
        assert_eq!(manifest_dir("services/api/Cargo.lock"), "services/api");
        assert_eq!(manifest_dir("Cargo.lock"), "");
    }
}
