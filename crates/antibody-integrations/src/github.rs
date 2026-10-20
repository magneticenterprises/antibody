//! GitHub webhook events normalized into provider-neutral `GitEvent`s.
//! Only the fields Antibody needs are parsed; everything else is ignored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[path = "github_validation.rs"]
mod validation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    pub full_name: String,
    pub provider_id: Option<String>,
    pub url: Option<String>,
    pub default_branch: Option<String>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub url: Option<String>,
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GitEvent {
    Ping,
    Push {
        repo: RepoRef,
        branch: Option<String>,
        after: String,
        commits: Vec<CommitInfo>,
        default_branch_push: bool,
    },
    PullRequest {
        repo: RepoRef,
        number: i64,
        action: String,
        merged: bool,
        merge_commit_sha: Option<String>,
        head_sha: String,
        head_ref: String,
        title: String,
        url: Option<String>,
        #[serde(default)]
        updated_at: Option<DateTime<Utc>>,
    },
    /// A completed check (check_run or workflow_run).
    CheckCompleted {
        repo: RepoRef,
        kind: String,
        id: String,
        #[serde(default = "first_attempt")]
        run_attempt: i64,
        #[serde(default)]
        completed_at: Option<DateTime<Utc>>,
        name: String,
        conclusion: String,
        head_sha: String,
        head_branch: Option<String>,
        url: Option<String>,
        /// Failure title/summary/text (untrusted, bounded later).
        excerpt: Option<String>,
    },
    DeploymentStatus {
        repo: RepoRef,
        id: String,
        state: String,
        sha: String,
        environment: Option<String>,
        url: Option<String>,
    },
    /// Repository deleted, archived, transferred or visibility changed.
    RepositoryChanged {
        repo: RepoRef,
        action: String,
        #[serde(default)]
        previous_full_name: Option<String>,
    },
    /// Collaborator/team membership changed: permissions may differ now.
    PermissionChanged {
        repo: Option<RepoRef>,
        action: String,
    },
    Unsupported {
        event: String,
    },
}

impl GitEvent {
    pub fn repo(&self) -> Option<&RepoRef> {
        match self {
            GitEvent::Push { repo, .. }
            | GitEvent::PullRequest { repo, .. }
            | GitEvent::CheckCompleted { repo, .. }
            | GitEvent::DeploymentStatus { repo, .. }
            | GitEvent::RepositoryChanged { repo, .. } => Some(repo),
            GitEvent::PermissionChanged { repo, .. } => repo.as_ref(),
            GitEvent::Ping | GitEvent::Unsupported { .. } => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid {event} payload: {message}")]
pub struct ParseError {
    pub event: String,
    pub message: String,
}

#[derive(Deserialize)]
struct GhRepo {
    full_name: String,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
}

impl From<GhRepo> for RepoRef {
    fn from(r: GhRepo) -> Self {
        RepoRef {
            full_name: r.full_name,
            provider_id: r.id.map(|v| v.to_string().trim_matches('"').to_string()),
            url: r.html_url,
            default_branch: r.default_branch,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct GhPush {
    #[serde(rename = "ref")]
    git_ref: String,
    after: String,
    repository: GhRepo,
    #[serde(default)]
    commits: Vec<GhCommit>,
}

#[derive(Deserialize)]
struct GhCommit {
    id: String,
    message: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    added: Vec<String>,
    #[serde(default)]
    modified: Vec<String>,
    #[serde(default)]
    removed: Vec<String>,
}

#[derive(Deserialize)]
struct GhPullRequestEvent {
    action: String,
    number: i64,
    pull_request: GhPullRequest,
    repository: GhRepo,
}

#[derive(Deserialize)]
struct GhPullRequest {
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    merge_commit_sha: Option<String>,
    head: GhHead,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
    title: String,
    #[serde(default)]
    html_url: Option<String>,
}

#[derive(Deserialize)]
struct GhHead {
    sha: String,
    #[serde(rename = "ref")]
    git_ref: String,
}

#[derive(Deserialize)]
struct GhCheckRunEvent {
    action: String,
    check_run: GhCheckRun,
    repository: GhRepo,
}

#[derive(Deserialize)]
struct GhCheckRun {
    id: serde_json::Value,
    name: String,
    #[serde(default)]
    conclusion: Option<String>,
    head_sha: String,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    output: Option<GhCheckOutput>,
    #[serde(default)]
    check_suite: Option<GhCheckSuite>,
    #[serde(default)]
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct GhCheckSuite {
    #[serde(default)]
    head_branch: Option<String>,
}

#[derive(Deserialize)]
struct GhCheckOutput {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct GhWorkflowRunEvent {
    action: String,
    workflow_run: GhWorkflowRun,
    repository: GhRepo,
}

#[derive(Deserialize)]
struct GhWorkflowRun {
    id: serde_json::Value,
    name: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    head_sha: String,
    #[serde(default)]
    head_branch: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    display_title: Option<String>,
    #[serde(default = "first_attempt")]
    run_attempt: i64,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct GhDeploymentStatusEvent {
    deployment_status: GhDeploymentStatus,
    deployment: GhDeployment,
    repository: GhRepo,
}

#[derive(Deserialize)]
struct GhDeploymentStatus {
    id: serde_json::Value,
    state: String,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
}

#[derive(Deserialize)]
struct GhDeployment {
    sha: String,
}

#[derive(Deserialize)]
struct GhRepositoryEvent {
    action: String,
    repository: GhRepo,
    #[serde(default)]
    changes: serde_json::Value,
}

fn first_attempt() -> i64 {
    1
}

#[derive(Deserialize)]
struct GhMaybeRepoEvent {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    repository: Option<GhRepo>,
}

fn parse<T: for<'de> Deserialize<'de>>(event: &str, body: &[u8]) -> Result<T, ParseError> {
    serde_json::from_slice(body).map_err(|e| ParseError {
        event: event.to_string(),
        message: e.to_string(),
    })
}

fn id_string(v: &serde_json::Value) -> String {
    v.to_string().trim_matches('"').to_string()
}

/// Parse a verified GitHub webhook body according to `X-GitHub-Event`.
pub fn parse_event(event: &str, body: &[u8]) -> Result<GitEvent, ParseError> {
    if matches!(
        event,
        "push"
            | "pull_request"
            | "check_run"
            | "workflow_run"
            | "deployment_status"
            | "repository"
            | "member"
            | "team_add"
            | "membership"
            | "team"
            | "organization"
    ) {
        let value: serde_json::Value = parse(event, body)?;
        if !validation::matches_event(event, &value) {
            return Ok(GitEvent::Unsupported {
                event: event.to_string(),
            });
        }
    }
    Ok(match event {
        "ping" => GitEvent::Ping,
        "push" => {
            let p: GhPush = parse(event, body)?;
            let branch = p.git_ref.strip_prefix("refs/heads/").map(str::to_string);
            let default_branch_push =
                branch.is_some() && branch.as_deref() == p.repository.default_branch.as_deref();
            GitEvent::Push {
                branch,
                after: p.after,
                default_branch_push,
                commits: p
                    .commits
                    .into_iter()
                    .take(200)
                    .map(|c| CommitInfo {
                        sha: c.id,
                        message: c.message,
                        url: c.url,
                        changed_paths: c
                            .added
                            .into_iter()
                            .chain(c.modified)
                            .chain(c.removed)
                            .take(500)
                            .collect(),
                    })
                    .collect(),
                repo: p.repository.into(),
            }
        }
        "pull_request" => {
            let p: GhPullRequestEvent = parse(event, body)?;
            GitEvent::PullRequest {
                number: p.number,
                action: p.action,
                merged: p.pull_request.merged,
                merge_commit_sha: p.pull_request.merge_commit_sha,
                head_sha: p.pull_request.head.sha,
                head_ref: p.pull_request.head.git_ref,
                title: p.pull_request.title,
                url: p.pull_request.html_url,
                updated_at: p.pull_request.updated_at,
                repo: p.repository.into(),
            }
        }
        "check_run" => {
            let p: GhCheckRunEvent = parse(event, body)?;
            if p.action != "completed" {
                return Ok(GitEvent::Unsupported {
                    event: format!("check_run.{}", p.action),
                });
            }
            let excerpt = p.check_run.output.map(|o| {
                [o.title, o.summary, o.text]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            GitEvent::CheckCompleted {
                kind: "check_run".into(),
                id: id_string(&p.check_run.id),
                run_attempt: 1,
                completed_at: p.check_run.completed_at,
                name: p.check_run.name,
                conclusion: p.check_run.conclusion.unwrap_or_else(|| "neutral".into()),
                head_sha: p.check_run.head_sha,
                head_branch: p.check_run.check_suite.and_then(|s| s.head_branch),
                url: p.check_run.html_url,
                excerpt: excerpt.filter(|e| !e.trim().is_empty()),
                repo: p.repository.into(),
            }
        }
        "workflow_run" => {
            let p: GhWorkflowRunEvent = parse(event, body)?;
            if p.action != "completed" {
                return Ok(GitEvent::Unsupported {
                    event: format!("workflow_run.{}", p.action),
                });
            }
            GitEvent::CheckCompleted {
                kind: "workflow_run".into(),
                id: id_string(&p.workflow_run.id),
                run_attempt: p.workflow_run.run_attempt.max(1),
                completed_at: p.workflow_run.updated_at,
                name: p.workflow_run.name.unwrap_or_else(|| "workflow".into()),
                conclusion: p
                    .workflow_run
                    .conclusion
                    .unwrap_or_else(|| "neutral".into()),
                head_sha: p.workflow_run.head_sha,
                head_branch: p.workflow_run.head_branch,
                url: p.workflow_run.html_url,
                excerpt: p.workflow_run.display_title,
                repo: p.repository.into(),
            }
        }
        "deployment_status" => {
            let p: GhDeploymentStatusEvent = parse(event, body)?;
            GitEvent::DeploymentStatus {
                id: id_string(&p.deployment_status.id),
                state: p.deployment_status.state,
                sha: p.deployment.sha,
                environment: p.deployment_status.environment,
                url: p.deployment_status.target_url,
                repo: p.repository.into(),
            }
        }
        "repository" => {
            let p: GhRepositoryEvent = parse(event, body)?;
            let (owner, name) = p.repository.full_name.split_once('/').unwrap_or(("", ""));
            let old_name = p.changes.pointer("/name/from").and_then(|v| v.as_str());
            let old_owner = p
                .changes
                .pointer("/owner/from/login")
                .or_else(|| p.changes.pointer("/owner/from"))
                .and_then(|v| v.as_str());
            let previous_full_name = (old_name.is_some() || old_owner.is_some()).then(|| {
                format!(
                    "{}/{}",
                    old_owner.unwrap_or(owner),
                    old_name.unwrap_or(name)
                )
            });
            GitEvent::RepositoryChanged {
                action: p.action,
                repo: p.repository.into(),
                previous_full_name,
            }
        }
        "member" | "team_add" | "membership" | "team" | "organization" => {
            let p: GhMaybeRepoEvent = parse(event, body)?;
            GitEvent::PermissionChanged {
                action: format!("{event}.{}", p.action.unwrap_or_default()),
                repo: p.repository.map(Into::into),
            }
        }
        other => GitEvent::Unsupported {
            event: other.to_string(),
        },
    })
}

/// Whether a check conclusion counts as a failure worth a CI lookup.
pub fn is_failure(conclusion: &str) -> bool {
    matches!(conclusion, "failure" | "timed_out" | "startup_failure")
}

pub fn is_success(conclusion: &str) -> bool {
    conclusion == "success"
}

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;
