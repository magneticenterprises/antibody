//! Retention: purge raw payloads past their expiry and expire housekeeping
//! rows. Derived content is removed by explicit deletion (see sessions).

use crate::DbResult;
use sqlx::PgPool;

#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct SweepReport {
    pub payloads_purged: u64,
    pub web_sessions_expired: u64,
    pub deliveries_pruned: u64,
    pub jobs_pruned: u64,
}

pub async fn sweep(pool: &PgPool) -> DbResult<SweepReport> {
    // Raw payloads expire at purge_after, and never later than the
    // organization's current retention window (which may have been shortened).
    let payloads_purged = sqlx::query(
        "DELETE FROM session_payloads p
          USING sessions s, organizations o
          WHERE p.session_id = s.id AND o.id = p.organization_id
            AND (p.purge_after <= now()
                 OR (s.capture_mode = 'raw'
                     AND p.created_at + make_interval(days => o.raw_retention_days) <= now()))",
    )
    .execute(pool)
    .await?
    .rows_affected();
    let web_sessions_expired = sqlx::query("DELETE FROM web_sessions WHERE expires_at <= now()")
        .execute(pool)
        .await?
        .rows_affected();
    let deliveries_pruned = sqlx::query(
        "DELETE FROM webhook_deliveries WHERE received_at < now() - interval '30 days'",
    )
    .execute(pool)
    .await?
    .rows_affected();
    let jobs_pruned = sqlx::query(
        "DELETE FROM jobs WHERE state IN ('succeeded', 'cancelled') AND updated_at < now() - interval '14 days'",
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(SweepReport {
        payloads_purged,
        web_sessions_expired,
        deliveries_pruned,
        jobs_pruned,
    })
}
