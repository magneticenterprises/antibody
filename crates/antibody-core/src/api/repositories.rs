//! Repository and component contracts.

use crate::model::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ComponentView {
    pub id: Uuid,
    pub name: String,
    pub path_glob: String,
    pub package_name: Option<String>,
    pub ecosystem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RepositoryView {
    pub id: Uuid,
    pub provider: RepositoryProvider,
    pub full_name: String,
    pub url: Option<String>,
    pub default_branch: String,
    pub status: String,
    pub permission: Permission,
    pub components: Vec<ComponentView>,
    pub last_event_at: Option<DateTime<Utc>>,
    pub session_count: i64,
    pub solution_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RepositoryList {
    pub items: Vec<RepositoryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRepository {
    pub provider: RepositoryProvider,
    pub full_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_repo_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateComponent {
    pub name: String,
    pub path_glob: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
}
