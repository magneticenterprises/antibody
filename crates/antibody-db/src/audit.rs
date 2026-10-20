//! Audit events. Metadata must never contain transcripts or credentials;
//! callers pass small, already-redacted fields and we redact again.

use crate::DbResult;
use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Actor {
    pub principal_id: Option<Uuid>,
    pub label: String,
}

impl Actor {
    pub fn system(label: &str) -> Self {
        Self {
            principal_id: None,
            label: label.to_string(),
        }
    }
    pub fn from_auth(auth: &crate::access::AuthContext) -> Self {
        Self {
            principal_id: Some(auth.principal.id),
            label: auth.actor_label(),
        }
    }
}

pub async fn record<'e>(
    exec: impl PgExecutor<'e>,
    org_id: Option<Uuid>,
    actor: &Actor,
    action: &str,
    resource_type: &str,
    resource_id: Option<String>,
    mut metadata: serde_json::Value,
) -> DbResult<()> {
    antibody_core::redact::redact_json(&mut metadata);
    sqlx::query(
        "INSERT INTO audit_events (organization_id, actor_principal_id, actor_label, action, resource_type, resource_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(org_id)
    .bind(actor.principal_id)
    .bind(&actor.label)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
    .bind(metadata)
    .execute(exec)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct AuditEventView {
    pub id: i64,
    pub actor_principal_id: Option<Uuid>,
    pub actor_label: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub async fn list(
    pool: &PgPool,
    org_id: Uuid,
    action_prefix: Option<&str>,
    limit: i64,
    before_id: Option<i64>,
) -> DbResult<Vec<AuditEventView>> {
    Ok(sqlx::query_as(
        "SELECT id, actor_principal_id, actor_label, action, resource_type, resource_id, metadata, created_at
           FROM audit_events
          WHERE organization_id = $1
            AND ($2::text IS NULL OR action LIKE $2 || '%')
            AND ($4::bigint IS NULL OR id < $4)
          ORDER BY id DESC LIMIT $3",
    )
    .bind(org_id)
    .bind(action_prefix)
    .bind(limit)
    .bind(before_id)
    .fetch_all(pool)
    .await?)
}
