use antibody_core::api::{SourceLink, VerificationSummary};
use antibody_core::model::{MergeState, SolutionState, SourceKind};
use antibody_db::solutions::SolutionRow;
use antibody_db::sources::SourceRow;
use std::str::FromStr;

/// Human-readable verification summary. Wording never upgrades evidence:
/// merges and agent claims are labelled as such.
pub fn verification_summary(
    row: &SolutionRow,
    trusted_count: i64,
    last_check: Option<&str>,
    last_rev: Option<&str>,
) -> VerificationSummary {
    let state = row.state_enum();
    let merge = row.merge_enum();
    let mut summary = match state {
        SolutionState::Verified => match (last_check, last_rev) {
            (Some(check), Some(rev)) => format!(
                "verified: trusted CI check `{}` passed on {}",
                antibody_core::text::one_line(check, 80),
                &rev[..rev.len().min(10)]
            ),
            _ => "verified by an administrator".to_string(),
        },
        SolutionState::Corroborated => {
            let mut parts = Vec::new();
            if row.corroboration_count > 1 {
                parts.push(format!("seen in {} sessions", row.corroboration_count));
            }
            if merge == MergeState::Merged {
                parts.push("fix merged".into());
            }
            format!(
                "corroborated ({}); not verified by CI",
                if parts.is_empty() {
                    "independent evidence".into()
                } else {
                    parts.join(", ")
                }
            )
        }
        SolutionState::Candidate => {
            "candidate: extracted from one session; no independent evidence yet".to_string()
        }
        SolutionState::Superseded => "superseded by a newer solution".to_string(),
        SolutionState::Withdrawn => "withdrawn".to_string(),
    };
    if merge == MergeState::Reverted {
        summary.push_str("; fix was reverted");
    }
    if row.needs_revalidation {
        summary.push_str(&format!(
            "; needs revalidation ({})",
            row.revalidation_reason
                .as_deref()
                .unwrap_or("evidence may be stale")
        ));
    }
    VerificationSummary {
        state,
        merge_state: merge,
        deployment_state: row.deployment_enum(),
        needs_revalidation: row.needs_revalidation,
        revalidation_reason: row.revalidation_reason.clone(),
        trusted_evidence_count: trusted_count,
        last_verified_at: row.last_verified_at,
        summary,
    }
}

pub fn source_link(src: &SourceRow, public_url: Option<&str>) -> SourceLink {
    let kind = SourceKind::from_str(&src.kind).unwrap_or(SourceKind::Url);
    let (label, url) = match kind {
        SourceKind::Session => (
            format!(
                "agent session {}",
                &src.external_id[..src.external_id.len().min(8)]
            ),
            public_url.map(|u| {
                format!(
                    "{}/activity?session={}",
                    u.trim_end_matches('/'),
                    src.external_id
                )
            }),
        ),
        SourceKind::Commit => (
            format!(
                "commit {}",
                &src.external_id[..src.external_id.len().min(10)]
            ),
            src.url.clone(),
        ),
        SourceKind::PullRequest => (format!("PR #{}", src.external_id), src.url.clone()),
        _ => (
            src.title
                .clone()
                .unwrap_or_else(|| format!("{} {}", src.kind, src.external_id)),
            src.url.clone(),
        ),
    };
    SourceLink {
        kind,
        label: antibody_core::text::one_line(&label, 160),
        url,
        revision: src.revision.clone(),
        state: src.state.clone(),
    }
}
