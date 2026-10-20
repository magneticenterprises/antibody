use crate::DbResult;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

/// Fingerprint authenticated bytes, independent of unsigned delivery/event headers.
/// Retained fingerprints make GitHub's non-expiring signatures replay resistant.
pub fn payload_hash(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// Record the payload and diagnostic delivery atomically in the caller's
/// transaction. A false result requires rollback (no side effects may commit).
pub async fn record_delivery(
    conn: &mut PgConnection,
    provider: &str,
    delivery_id: &str,
    org_id: Uuid,
    event: &str,
    payload_hash: &str,
) -> DbResult<bool> {
    let fresh = sqlx::query(
        "INSERT INTO webhook_payloads (organization_id, provider, payload_hash) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(org_id)
    .bind(provider)
    .bind(payload_hash)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if !fresh {
        return Ok(false);
    }
    let res = sqlx::query(
        "INSERT INTO webhook_deliveries (provider, delivery_id, organization_id, event) VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(provider)
    .bind(antibody_core::redact::redact(delivery_id).text)
    .bind(org_id)
    .bind(antibody_core::redact::redact(event).text)
    .execute(conn)
    .await?;
    Ok(res.rows_affected() == 1)
}
