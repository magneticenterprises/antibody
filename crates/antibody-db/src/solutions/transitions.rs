//! State-machine transitions and administrative state changes.

use crate::{DbResult, parse_enum};
use antibody_core::error::{DomainError, ErrorCode};
use antibody_core::model::SolutionState;
use antibody_core::state::{self, SolutionEvent, SolutionStatus};
use sqlx::PgConnection;
use uuid::Uuid;

/// Apply a state-machine event and persist the resulting status.
pub async fn apply_event(
    conn: &mut PgConnection,
    id: Uuid,
    event: &SolutionEvent,
) -> DbResult<SolutionStatus> {
    let row: Option<(String, String, String, bool, Option<String>)> = sqlx::query_as(
        "SELECT state, merge_state, deployment_state, needs_revalidation, revalidation_reason
           FROM solutions WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((st, ms, ds, nr, rr)) = row else {
        return Err(DomainError::not_found("solution").into());
    };
    let current = SolutionStatus {
        state: parse_enum(&st)?,
        merge_state: parse_enum(&ms)?,
        deployment_state: parse_enum(&ds)?,
        needs_revalidation: nr,
        revalidation_reason: rr,
    };
    let next = state::apply(&current, event)?;
    let verified_now = next.state == SolutionState::Verified
        && matches!(
            event,
            SolutionEvent::TrustedCheckPassed | SolutionEvent::ManuallyVerified
        );
    sqlx::query(
        "UPDATE solutions SET state = $2, merge_state = $3, deployment_state = $4, needs_revalidation = $5,
                revalidation_reason = $6, last_verified_at = CASE WHEN $7 THEN now() ELSE last_verified_at END,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(next.state.as_str())
    .bind(next.merge_state.as_str())
    .bind(next.deployment_state.as_str())
    .bind(next.needs_revalidation)
    .bind(&next.revalidation_reason)
    .bind(verified_now)
    .execute(&mut *conn)
    .await?;
    Ok(next)
}

pub async fn set_superseded_by(
    conn: &mut PgConnection,
    org_id: Uuid,
    id: Uuid,
    by: Uuid,
) -> DbResult<()> {
    if id == by {
        return Err(DomainError::new(
            ErrorCode::InvalidRequest,
            "a solution cannot supersede itself",
        )
        .into());
    }
    let res = sqlx::query(
        "UPDATE solutions SET superseded_by = $3, updated_at = now()
          WHERE id = $2 AND organization_id = $1
            AND EXISTS (SELECT 1 FROM solutions b WHERE b.id = $3 AND b.organization_id = $1)",
    )
    .bind(org_id)
    .bind(id)
    .bind(by)
    .execute(&mut *conn)
    .await?;
    if res.rows_affected() == 0 {
        return Err(DomainError::not_found("superseding solution").into());
    }
    Ok(())
}

/// Clear the revalidation flag (admin review).
pub async fn clear_revalidation(conn: &mut PgConnection, id: Uuid) -> DbResult<()> {
    sqlx::query("UPDATE solutions SET needs_revalidation = false, revalidation_reason = NULL, updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
