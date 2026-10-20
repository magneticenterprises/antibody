//! OpenAPI document generated from the Rust endpoint types. The committed
//! copy in `openapi/openapi.json` is checked for drift in tests and CI.

use crate::routes::*;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

struct Security;

/// Document the error responses every operation can produce so generated
/// clients see them: 401/403 on authenticated operations, 429 (rate limit)
/// and 500 everywhere. Operation-specific errors are declared per route.
struct StandardErrors;

impl Modify for StandardErrors {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::{ContentBuilder, Ref, ResponseBuilder};
        let error = |description: &str| {
            ResponseBuilder::new()
                .description(description)
                .content(
                    "application/json",
                    ContentBuilder::new()
                        .schema(Some(Ref::from_schema_name("ErrorBody")))
                        .build(),
                )
                .build()
        };
        for item in openapi.paths.paths.values_mut() {
            let ops = [
                &mut item.get,
                &mut item.put,
                &mut item.post,
                &mut item.delete,
                &mut item.patch,
            ];
            for op in ops.into_iter().flatten() {
                let authenticated = op.security.as_ref().is_some_and(|s| !s.is_empty());
                let responses = &mut op.responses.responses;
                let mut add = |code: &str, description: &str| {
                    responses
                        .entry(code.to_string())
                        .or_insert_with(|| error(description).into());
                };
                if authenticated {
                    add(
                        "401",
                        "Missing, invalid, expired or revoked credentials (`unauthenticated`)",
                    );
                    add(
                        "403",
                        "Insufficient scope or role, CSRF failure, or repository not accessible",
                    );
                }
                add(
                    "429",
                    "Rate limited (`rate_limited`); retry after the Retry-After interval",
                );
                add(
                    "500",
                    "Internal error (`internal`); details are logged server-side only",
                );
            }
        }
    }
}

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("abt_ token")
                    .description(Some(
                        "API token issued by an administrator (scoped, revocable).",
                    ))
                    .build(),
            ),
        );
        components.add_security_scheme(
            "cookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "antibody_session",
                "Browser session cookie (named __Host-antibody_session over HTTPS). Mutating requests also need X-CSRF-Token.",
            ))),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Antibody API",
        description = "Self-hostable collective memory for coding agents. All errors use the ErrorBody shape with a typed code. \
Request bodies are JSON (`Content-Type: application/json`) and size-limited (sessions: ANTIBODY_MAX_SESSION_BYTES, webhooks: ANTIBODY_MAX_WEBHOOK_BYTES, other: ANTIBODY_MAX_JSON_BYTES).",
        license(name = "Apache-2.0"),
    ),
    paths(
        health::live, health::ready,
        sessions::ingest, sessions::status, sessions::list, sessions::delete, sessions::transcript,
        context::search, context::get_solution, context::delete_solution, context::list_solutions, context::change_state, context::propagation, context::report_outcome,
        repositories::list, repositories::create, repositories::add_component, repositories::patch,
        webhooks::github, webhooks::ci_failure,
        hooks::user_prompt_submit,
        auth_routes::login, auth_routes::logout, auth_routes::session,
        admin::overview, admin::ci_events, admin::retrievals,
        admin::list_jobs, admin::retry_job,
        admin::list_principals, admin::create_principal, admin::list_grants, admin::put_grant, admin::delete_grant,
        admin::list_tokens, admin::create_token, admin::revoke_token,
        admin::get_settings, admin::patch_settings, admin::webhook_secrets, admin::sweep,
        admin::audit_log, admin::export, admin::list_workflow, admin::create_workflow, admin::transition_workflow,
    ),
    components(schemas(antibody_core::ErrorCode, antibody_core::api::ErrorBody)),
    modifiers(&Security, &StandardErrors),
    tags(
        (name = "sessions", description = "Flow A: session ingestion"),
        (name = "retrieval", description = "Flow B: authorized hybrid search, solutions and outcomes"),
        (name = "webhooks", description = "Flows C/D: signed Git and CI events"),
        (name = "hooks", description = "Native agent HTTP hooks"),
        (name = "repositories"), (name = "auth"), (name = "dashboard"), (name = "admin"), (name = "health"),
    )
)]
pub struct ApiDoc;

pub fn json() -> String {
    ApiDoc::openapi()
        .to_pretty_json()
        .expect("OpenAPI serializes")
}

pub fn mcp_tools_json() -> String {
    let tools = crate::mcp::tool_definitions();
    serde_json::to_string_pretty(&serde_json::json!({ "tools": tools })).expect("tools serialize")
}
