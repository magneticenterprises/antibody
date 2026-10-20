//! Solution and workflow-improvement state machines.
//!
//! Rules (spec section 5 and 9):
//! - A merged change is not automatically verified or deployed.
//! - An agent claim that tests passed is not verification.
//! - Only a trusted (signed webhook or audited admin) passing check verifies.
//! - Withdrawn is terminal; superseded can only be withdrawn.

use crate::error::{DomainError, ErrorCode};
use crate::model::{DeploymentState, MergeState, SolutionState, WorkflowState};

/// Facts that can be applied to a solution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolutionEvent {
    /// The same fix was independently extracted from another session.
    Corroborated,
    /// A change containing the fix was merged.
    Merged,
    /// The fix commit was reverted.
    Reverted,
    /// A deployment containing the fix succeeded.
    Deployed,
    /// A trusted check passed on a revision containing the fix.
    TrustedCheckPassed,
    /// A trusted check failed on a revision containing the fix.
    TrustedCheckFailed,
    /// An agent (or untrusted reporter) claims tests passed.
    AgentClaimedPass,
    /// Dependencies of the repository changed in a way that may affect the fix.
    DependencyChanged,
    /// An administrator verified the fix manually (audited).
    ManuallyVerified,
    Superseded,
    Withdrawn,
}

/// Full lifecycle snapshot of a solution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolutionStatus {
    pub state: SolutionState,
    pub merge_state: MergeState,
    pub deployment_state: DeploymentState,
    pub needs_revalidation: bool,
    pub revalidation_reason: Option<String>,
}

impl SolutionStatus {
    pub fn new_candidate() -> Self {
        Self {
            state: SolutionState::Candidate,
            merge_state: MergeState::Unknown,
            deployment_state: DeploymentState::Unknown,
            needs_revalidation: false,
            revalidation_reason: None,
        }
    }

    pub fn is_suppressed(&self) -> bool {
        matches!(
            self.state,
            SolutionState::Withdrawn | SolutionState::Superseded
        )
    }
}

fn rank(s: SolutionState) -> u8 {
    match s {
        SolutionState::Candidate => 0,
        SolutionState::Corroborated => 1,
        SolutionState::Verified => 2,
        SolutionState::Superseded => 3,
        SolutionState::Withdrawn => 4,
    }
}

/// Apply an event, returning the new status. Events that do not change the
/// verification state still update merge/deployment/revalidation fields.
pub fn apply(
    current: &SolutionStatus,
    event: &SolutionEvent,
) -> Result<SolutionStatus, DomainError> {
    let mut next = current.clone();
    let terminal = current.state == SolutionState::Withdrawn;
    let live = !current.is_suppressed();

    match event {
        SolutionEvent::Withdrawn => {
            if terminal {
                return Err(invalid(current.state, "withdrawn"));
            }
            next.state = SolutionState::Withdrawn;
        }
        SolutionEvent::Superseded => {
            if current.is_suppressed() {
                return Err(invalid(current.state, "superseded"));
            }
            next.state = SolutionState::Superseded;
        }
        SolutionEvent::Corroborated => {
            if live && rank(current.state) < rank(SolutionState::Corroborated) {
                next.state = SolutionState::Corroborated;
            }
        }
        SolutionEvent::Merged => {
            // Merge is corroborating evidence only: never verification, never deployment.
            next.merge_state = MergeState::Merged;
            if live && current.state == SolutionState::Candidate {
                next.state = SolutionState::Corroborated;
            }
            if current.merge_state == MergeState::Reverted {
                next.needs_revalidation = true;
                next.revalidation_reason = Some("fix was re-merged after a revert".into());
            }
        }
        SolutionEvent::Reverted => {
            next.merge_state = MergeState::Reverted;
            next.needs_revalidation = true;
            next.revalidation_reason = Some("fix commit was reverted".into());
        }
        SolutionEvent::Deployed => {
            next.deployment_state = DeploymentState::Deployed;
        }
        SolutionEvent::TrustedCheckPassed | SolutionEvent::ManuallyVerified => {
            if !live {
                return Ok(next);
            }
            next.state = SolutionState::Verified;
            if current.merge_state != MergeState::Reverted {
                next.needs_revalidation = false;
                next.revalidation_reason = None;
            }
        }
        SolutionEvent::TrustedCheckFailed => {
            if live && current.state == SolutionState::Verified {
                next.needs_revalidation = true;
                next.revalidation_reason =
                    Some("a trusted check failed on a revision containing the fix".into());
            }
        }
        SolutionEvent::AgentClaimedPass => {
            // Recorded as evidence elsewhere; deliberately no state change.
        }
        SolutionEvent::DependencyChanged => {
            if live {
                next.needs_revalidation = true;
                next.revalidation_reason =
                    Some("repository dependencies changed since the fix was verified".into());
            }
        }
    }
    Ok(next)
}

fn invalid(from: SolutionState, to: &str) -> DomainError {
    DomainError::new(
        ErrorCode::InvalidStateTransition,
        format!("cannot transition solution from {from} to {to}"),
    )
}

/// Requirements checked before a workflow improvement may be promoted.
#[derive(Debug, Clone, Default)]
pub struct PromotionGate {
    pub owner_approved: bool,
    pub evaluation_reproducible: bool,
    pub has_negative_cases: bool,
    pub preserves_checks: bool,
}

pub fn workflow_transition(
    from: WorkflowState,
    to: WorkflowState,
    gate: &PromotionGate,
) -> Result<WorkflowState, DomainError> {
    use WorkflowState::*;
    let allowed = matches!(
        (from, to),
        (Candidate, Evaluated)
            | (Evaluated, Available)
            | (Available, Promoted)
            | (Candidate, Retired)
            | (Evaluated, Retired)
            | (Available, Retired)
            | (Promoted, Retired)
    );
    if !allowed {
        return Err(DomainError::new(
            ErrorCode::InvalidStateTransition,
            format!("cannot transition workflow improvement from {from} to {to}"),
        ));
    }
    if to == Promoted {
        let mut missing = Vec::new();
        if !gate.owner_approved {
            missing.push("owner approval");
        }
        if !gate.evaluation_reproducible {
            missing.push("reproducible evaluation");
        }
        if !gate.has_negative_cases {
            missing.push("negative cases");
        }
        if !gate.preserves_checks {
            missing.push("preservation of existing checks");
        }
        if !missing.is_empty() {
            return Err(DomainError::new(
                ErrorCode::InvalidStateTransition,
                format!("promotion requires: {}", missing.join(", ")),
            ));
        }
    }
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> SolutionStatus {
        SolutionStatus::new_candidate()
    }

    #[test]
    fn merge_is_not_verification_or_deployment() {
        let after = apply(&s(), &SolutionEvent::Merged).unwrap();
        assert_eq!(after.state, SolutionState::Corroborated);
        assert_eq!(after.merge_state, MergeState::Merged);
        assert_eq!(after.deployment_state, DeploymentState::Unknown);
        assert_ne!(after.state, SolutionState::Verified);
    }

    #[test]
    fn deployment_is_not_verification() {
        let after = apply(&s(), &SolutionEvent::Deployed).unwrap();
        assert_eq!(after.state, SolutionState::Candidate);
        assert_eq!(after.deployment_state, DeploymentState::Deployed);
    }

    #[test]
    fn agent_claim_is_not_verification() {
        let after = apply(&s(), &SolutionEvent::AgentClaimedPass).unwrap();
        assert_eq!(after.state, SolutionState::Candidate);
    }

    #[test]
    fn trusted_check_verifies() {
        let after = apply(&s(), &SolutionEvent::TrustedCheckPassed).unwrap();
        assert_eq!(after.state, SolutionState::Verified);
        assert_eq!(
            after.merge_state,
            MergeState::Unknown,
            "verification does not imply merge"
        );
    }

    #[test]
    fn revert_flags_revalidation_without_changing_state() {
        let v = apply(&s(), &SolutionEvent::TrustedCheckPassed).unwrap();
        let r = apply(&v, &SolutionEvent::Reverted).unwrap();
        assert_eq!(r.state, SolutionState::Verified);
        assert!(r.needs_revalidation);
        assert_eq!(r.merge_state, MergeState::Reverted);
        // A later passing check does not clear a revert flag on its own.
        let again = apply(&r, &SolutionEvent::TrustedCheckPassed).unwrap();
        assert!(again.needs_revalidation);
    }

    #[test]
    fn dependency_change_flags_revalidation_and_check_clears_it() {
        let v = apply(&s(), &SolutionEvent::TrustedCheckPassed).unwrap();
        let d = apply(&v, &SolutionEvent::DependencyChanged).unwrap();
        assert!(d.needs_revalidation);
        let c = apply(&d, &SolutionEvent::TrustedCheckPassed).unwrap();
        assert!(!c.needs_revalidation);
    }

    #[test]
    fn withdrawn_is_terminal() {
        let w = apply(&s(), &SolutionEvent::Withdrawn).unwrap();
        assert!(apply(&w, &SolutionEvent::Withdrawn).is_err());
        assert!(apply(&w, &SolutionEvent::Superseded).is_err());
        let still = apply(&w, &SolutionEvent::TrustedCheckPassed).unwrap();
        assert_eq!(still.state, SolutionState::Withdrawn);
        let c = apply(&w, &SolutionEvent::Corroborated).unwrap();
        assert_eq!(c.state, SolutionState::Withdrawn);
    }

    #[test]
    fn superseded_cannot_be_reverified() {
        let sup = apply(&s(), &SolutionEvent::Superseded).unwrap();
        let after = apply(&sup, &SolutionEvent::TrustedCheckPassed).unwrap();
        assert_eq!(after.state, SolutionState::Superseded);
        assert!(apply(&sup, &SolutionEvent::Withdrawn).is_ok());
    }

    #[test]
    fn corroboration_never_downgrades() {
        let v = apply(&s(), &SolutionEvent::TrustedCheckPassed).unwrap();
        let c = apply(&v, &SolutionEvent::Corroborated).unwrap();
        assert_eq!(c.state, SolutionState::Verified);
    }

    #[test]
    fn workflow_promotion_requires_gate() {
        let gate = PromotionGate::default();
        assert!(
            workflow_transition(WorkflowState::Available, WorkflowState::Promoted, &gate).is_err()
        );
        let ok = PromotionGate {
            owner_approved: true,
            evaluation_reproducible: true,
            has_negative_cases: true,
            preserves_checks: true,
        };
        assert_eq!(
            workflow_transition(WorkflowState::Available, WorkflowState::Promoted, &ok).unwrap(),
            WorkflowState::Promoted
        );
        assert!(
            workflow_transition(WorkflowState::Candidate, WorkflowState::Promoted, &ok).is_err()
        );
        assert!(
            workflow_transition(WorkflowState::Retired, WorkflowState::Available, &ok).is_err()
        );
    }
}
