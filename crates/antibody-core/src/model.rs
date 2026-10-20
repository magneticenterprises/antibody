//! String-backed domain enums. Each enum maps 1:1 to a `CHECK` constraint in
//! the SQL migrations, so the database and the Rust types cannot drift.

use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $( $(#[$vmeta:meta])* $variant:ident => $s:tt ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $( $(#[$vmeta])* #[serde(rename = $s)] $variant ),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = crate::error::DomainError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($s => Ok($name::$variant),)+
                    other => Err(crate::error::DomainError::invalid(format!(
                        concat!("invalid ", stringify!($name), " value: {}"), other
                    ))),
                }
            }
        }
    };
}

string_enum! {
    /// Coding agent that produced a session.
    pub enum AgentType {
        ClaudeCode => "claude_code",
        Codex => "codex",
        Other => "other",
    }
}

string_enum! {
    /// Verification lifecycle of a solution. Merge and deployment are tracked
    /// separately and never imply `verified`.
    pub enum SolutionState {
        /// Extracted from a session without independent evidence.
        Candidate => "candidate",
        /// Independently observed again (another session, a merged change).
        Corroborated => "corroborated",
        /// A trusted, signed CI check passed on a revision containing the fix.
        Verified => "verified",
        Superseded => "superseded",
        Withdrawn => "withdrawn",
    }
}

string_enum! {
    pub enum MergeState {
        Unknown => "unknown",
        Unmerged => "unmerged",
        Merged => "merged",
        Reverted => "reverted",
    }
}

string_enum! {
    pub enum DeploymentState {
        Unknown => "unknown",
        Deployed => "deployed",
    }
}

string_enum! {
    pub enum Role {
        /// Organization administrator: all repositories, settings and tokens.
        Admin => "admin",
        /// Access limited to explicitly granted repositories.
        Member => "member",
    }
}

string_enum! {
    pub enum PrincipalKind {
        User => "user",
        Service => "service",
    }
}

string_enum! {
    /// API token scopes. A token can never exceed its principal's role.
    pub enum Scope {
        SessionsWrite => "sessions:write",
        ContextRead => "context:read",
        OutcomesWrite => "outcomes:write",
        CiWrite => "ci:write",
        Admin => "admin",
    }
}

string_enum! {
    pub enum Permission {
        Read => "read",
        Write => "write",
    }
}

string_enum! {
    pub enum RepositoryProvider {
        Github => "github",
        Gitlab => "gitlab",
        Generic => "generic",
    }
}

string_enum! {
    pub enum CaptureMode {
        /// Redacted transcript retained until the retention expiry.
        Raw => "raw",
        /// Transcript discarded as soon as extraction finishes.
        StructuredOnly => "structured_only",
    }
}

string_enum! {
    pub enum ExtractionStatus {
        Pending => "pending",
        Running => "running",
        Succeeded => "succeeded",
        Failed => "failed",
        Skipped => "skipped",
    }
}

string_enum! {
    pub enum MessageRole {
        User => "user",
        Assistant => "assistant",
        Tool => "tool",
        System => "system",
    }
}

string_enum! {
    pub enum EvidenceKind {
        /// Signed CI check result from a Git provider webhook.
        CiCheck => "ci_check",
        Merge => "merge",
        Revert => "revert",
        Deployment => "deployment",
        /// Reported by an agent via report_outcome; never verification.
        Outcome => "outcome",
        /// An agent's own statement (e.g. "tests pass"); never verification.
        AgentClaim => "agent_claim",
        ManualVerification => "manual_verification",
        DependencyChange => "dependency_change",
        Corroboration => "corroboration",
    }
}

string_enum! {
    pub enum EvidenceResult {
        Pass => "pass",
        Fail => "fail",
        Info => "info",
    }
}

string_enum! {
    pub enum OutcomeKind {
        Used => "used",
        Rejected => "rejected",
        Inapplicable => "inapplicable",
    }
}

string_enum! {
    /// How an outcome's verification claim is backed.
    pub enum OutcomeVerification {
        None => "none",
        /// The agent supplied check details; not yet confirmed.
        Claimed => "claimed",
        /// A signed CI event confirmed the check on the claimed revision.
        CiVerified => "ci_verified",
    }
}

string_enum! {
    pub enum WorkflowState {
        Candidate => "candidate",
        Evaluated => "evaluated",
        Available => "available",
        Promoted => "promoted",
        Retired => "retired",
    }
}

string_enum! {
    pub enum JobState {
        Queued => "queued",
        Running => "running",
        Succeeded => "succeeded",
        Dead => "dead",
        Cancelled => "cancelled",
    }
}

string_enum! {
    pub enum JobKind {
        ExtractSession => "extract_session",
        EmbedSolution => "embed_solution",
        ProcessGitEvent => "process_git_event",
        CiLookup => "ci_lookup",
        Revalidate => "revalidate",
        RetentionSweep => "retention_sweep",
    }
}

string_enum! {
    pub enum RetrievalChannel {
        Http => "http",
        Mcp => "mcp",
        Hook => "hook",
        Ci => "ci",
    }
}

string_enum! {
    pub enum ApplicabilityStatus {
        /// Known constraints are satisfied (or there are none).
        Compatible => "compatible",
        /// Constraints exist but the caller did not supply enough context.
        Unknown => "unknown",
        /// Evidence is stale (revert, dependency change) and needs revalidation.
        Stale => "stale",
        /// A known constraint is violated.
        Incompatible => "incompatible",
    }
}

string_enum! {
    pub enum SourceKind {
        Session => "session",
        Commit => "commit",
        PullRequest => "pull_request",
        Issue => "issue",
        CheckRun => "check_run",
        WorkflowRun => "workflow_run",
        Deployment => "deployment",
        Url => "url",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn enums_round_trip_through_strings_and_serde() {
        for s in SolutionState::ALL {
            assert_eq!(SolutionState::from_str(s.as_str()).unwrap(), *s);
            let json = serde_json::to_string(s).unwrap();
            assert_eq!(json, format!("\"{}\"", s.as_str()));
        }
        assert_eq!(
            Scope::from_str("sessions:write").unwrap(),
            Scope::SessionsWrite
        );
        assert_eq!(
            serde_json::to_string(&Scope::ContextRead).unwrap(),
            "\"context:read\""
        );
        assert!(AgentType::from_str("bogus").is_err());
    }
}
