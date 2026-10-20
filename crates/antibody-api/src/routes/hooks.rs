//! Task-start hook endpoint for agents with native HTTP hooks (Claude Code
//! `type: "http"` hooks). Fails open: internal errors return an empty hook
//! response so the agent proceeds normally.

use crate::auth::Auth;
use crate::error::ApiResult;
use crate::extract::ApiJson;
use crate::routes::context::external_approved;
use crate::state::AppState;
use antibody_core::api::ContextSearchRequest;
use antibody_core::model::{RetrievalChannel, Scope};
use antibody_db::retrievals;
use antibody_integrations::hooks::{HookInput, HookOutput, render_context};
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use std::time::Duration;

const HOOK_BUDGET: Duration = Duration::from_millis(2500);

#[utoipa::path(
    post, path = "/v1/hooks/user-prompt-submit", operation_id = "user_prompt_submit_hook", tag = "hooks",
    params(
        ("X-Antibody-Repository" = String, Header, description = "Repository full name, e.g. acme/api"),
        ("X-Antibody-Component" = Option<String>, Header, description = "Optional component path"),
        ("X-Antibody-Every-Prompt" = Option<bool>, Header, description = "Search on every prompt instead of only the first one per session")),
    request_body = HookInput,
    responses((status = 200, description = "Claude Code hook output (empty object when there is nothing to add)", body = HookOutput)),
    security(("bearer" = []))
)]
pub async fn user_prompt_submit(
    State(state): State<AppState>,
    auth: Auth,
    headers: HeaderMap,
    ApiJson(input): ApiJson<HookInput>,
) -> ApiResult<Json<HookOutput>> {
    auth.require(Scope::ContextRead)?;
    let event = input
        .hook_event_name
        .clone()
        .unwrap_or_else(|| "UserPromptSubmit".into());
    let prompt = input.prompt.clone().unwrap_or_default();
    if prompt.trim().chars().count() < 12 || event != "UserPromptSubmit" {
        return Ok(Json(HookOutput::empty()));
    }
    let every = headers
        .get("x-antibody-every-prompt")
        .and_then(|v| v.to_str().ok())
        == Some("true");
    let hdr = |n: &str| {
        headers
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let work = async {
        if let (false, Some(sid)) = (every, input.session_id.as_deref())
            && retrievals::seen_agent_session(
                &state.pool,
                auth.0.org_id(),
                auth.0.principal.id,
                sid,
            )
            .await?
        {
            return Ok::<_, crate::error::ApiError>(HookOutput::empty());
        }
        let scope = auth.scope(&state).await?;
        let approved = external_approved(&state, auth.0.org_id()).await?;
        let req = ContextSearchRequest {
            task: antibody_core::text::truncate_chars(&prompt, 8000),
            repository: hdr("x-antibody-repository"),
            component_path: hdr("x-antibody-component"),
            agent_session_id: input.session_id.clone(),
            ..Default::default()
        };
        let resp = state
            .search
            .search(&scope, &req, RetrievalChannel::Hook, approved)
            .await?;
        Ok(match render_context(&resp) {
            Some(text) => HookOutput::context(&event, text),
            None => HookOutput::empty(),
        })
    };
    match tokio::time::timeout(HOOK_BUDGET, work).await {
        Ok(Ok(out)) => Ok(Json(out)),
        Ok(Err(e)) if e.code == antibody_core::ErrorCode::Forbidden => Err(e),
        Ok(Err(e)) => {
            tracing::warn!(code = ?e.code, "hook search failed; failing open");
            Ok(Json(HookOutput::empty()))
        }
        Err(_) => {
            tracing::warn!("hook search exceeded budget; failing open");
            Ok(Json(HookOutput::empty()))
        }
    }
}
