//! Git metadata collected by running `git` with fixed argv (never via a shell
//! and never with transcript-derived arguments).

use crate::config::env;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitInfo {
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `None` when `cwd` is not inside a git work tree.
pub fn collect(cwd: &Path) -> Option<GitInfo> {
    if git(cwd, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return None;
    }
    let repository = parse_remote(&git(cwd, &["remote", "get-url", "origin"]).unwrap_or_default());
    let revision = git(cwd, &["rev-parse", "HEAD"]).filter(|s| !s.is_empty());
    let branch =
        git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|s| !s.is_empty() && s != "HEAD");
    let dirty = git(cwd, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    Some(GitInfo {
        repository,
        revision,
        branch,
        dirty,
    })
}

/// Origin lookup only: prompt hooks do not need a full worktree status scan.
pub fn repository(cwd: &Path) -> Option<String> {
    parse_remote(&git(cwd, &["remote", "get-url", "origin"])?)
}

/// Repository for requests: `ANTIBODY_REPOSITORY` wins over the git origin.
pub fn resolve_repository(info: Option<&GitInfo>) -> Option<String> {
    env("ANTIBODY_REPOSITORY").or_else(|| info.and_then(|g| g.repository.clone()))
}

/// Convert an origin URL to `owner/name`. Handles https/http/git, `ssh://`,
/// scp-like `git@host:owner/name.git`, credentials and trailing `.git`.
pub fn parse_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let path = if let Some((_, rest)) = url.split_once("://") {
        // scheme://[user[:pass]@]host[:port]/path
        let (_, path) = rest.split_once('/')?;
        path
    } else {
        // scp-like: [user@]host:path
        let (host, path) = url.split_once(':')?;
        if host.contains('/') {
            return None;
        }
        path
    };
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 || segs.iter().any(|s| *s == ".." || *s == ".") {
        return None;
    }
    Some(segs.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remote_forms() {
        for (input, want) in [
            ("https://github.com/acme/api.git", Some("acme/api")),
            ("https://github.com/acme/api", Some("acme/api")),
            (
                "https://user:tok@github.com/acme/api.git/",
                Some("acme/api"),
            ),
            ("http://host:8080/acme/api.git", Some("acme/api")),
            ("git@github.com:acme/api.git", Some("acme/api")),
            ("git@github.com:acme/api", Some("acme/api")),
            ("ssh://git@github.com/acme/api.git", Some("acme/api")),
            ("ssh://git@host:2222/acme/api.git", Some("acme/api")),
            ("git://github.com/acme/api.git", Some("acme/api")),
            (
                "https://gitlab.com/group/sub/api.git",
                Some("group/sub/api"),
            ),
            ("", None),
            ("/local/path/repo", None),
            ("https://github.com/onlyone", None),
        ] {
            assert_eq!(parse_remote(input).as_deref(), want, "{input}");
        }
    }
}
