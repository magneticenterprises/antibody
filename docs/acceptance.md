# Acceptance status (spec section 13)

How each criterion is verified and the result of the most recent local run
(2026-10-20, macOS, PostgreSQL 18 + pgvector in Docker). "Pending" means the
check exists but has not passed yet, for the reason given.

Run the Rust checks with:

```bash
docker compose -f deploy/compose.dev.yaml up -d --wait
DATABASE_URL=postgres://antibody:antibody@localhost:55432/antibody cargo test --workspace
```

| ID | Requirement | Verification | Status |
|---|---|---|---|
| A01 | Fresh self-hosted install works without proprietary accounts | `scripts/smoke.sh` (builds both images, bootstraps, ingests, extracts, searches over HTTP and MCP through Caddy) | Pass (full run with the real web image): readiness through Caddy, UI security headers, auth rejection, cookie login + CSRF rejection, repository/principal/grant/token creation, idempotent ingestion, extraction, search, MCP `find_relevant_context` through the proxy, Postgres not published. Dashboard also checked by hand in a browser against this stack. |
| A02 | Session ingestion is durable and idempotent | `a02_ingestion_is_idempotent`, `a02_worker_crash_is_recovered_by_lease_expiry`, `a02_failures_retry_with_backoff_then_dead_letter` | Pass |
| A03 | Invalid/unauthorized/oversized payloads fail safely | `a03_negative_ingestion_cases`, `a03_browser_session_requires_csrf_and_same_origin`, `a03_members_cannot_use_admin_endpoints` | Pass |
| A04 | Extracted knowledge retains source references | `a04_extraction_keeps_sources_and_redacts`, `a04_sessions_without_a_solved_problem_create_no_knowledge`, `a04_duplicates_corroborate_and_distinct_applicability_versions`, worker unit tests in `extract.rs` | Pass |
| A05 | Merge is distinct from verification and deployment | `a05_merge_verification_and_deployment_are_separate`, `antibody-core` `state::tests` | Pass |
| A06 | Supported task-start hook actually invokes retrieval | `tests/agents/real_agent_hooks.sh` (real CLIs), `a06_user_prompt_submit_hook_injects_context_once` (server side) | Claude Code 2.1.302: hook invocation **proven** (real `claude -p` run recorded a `hook` retrieval with a result). Whether the model used the context was not observed because the local CLI login had expired. Codex 0.163.0: **pending**; Codex only runs trusted hooks, and the hook was not yet trusted (see `docs/integrations/codex.md`). |
| A07 | Known exact error finds prior verified fix | `a07_exact_error_finds_verified_fix_across_repositories` | Pass |
| A08 | Similar but incompatible fix is rejected or qualified | `a08_incompatible_fix_is_suppressed_or_qualified`, `applicability::tests` | Pass |
| A09 | Authorized cross-repo retrieval works; unauthorized retrieval leaks nothing | `a09_isolation_between_repositories_and_organizations`, `a09_mcp_shares_retrieval_and_permissions`, `flow_d_propagation_lists_siblings_within_scope` | Pass |
| A10 | CI failure finds prior diagnosis | `a10_signed_ci_failure_finds_prior_diagnosis` (generic signed CI webhook and GitHub `check_run`) | Pass |
| A11 | Reverts and stale dependencies invalidate applicability | `a11_revert_and_dependency_changes_flag_revalidation` | Pass |
| A12 | UI shows solutions, ingestion, evidence, and failures | Playwright suite in `apps/web/e2e` (real API and worker) | Pass (4/4): admin path overview → knowledge → evidence → activity → settings; member forbidden/hidden admin areas; bad login error with request id; preview search not recorded. Vitest also covers delayed navigation, authentication, and access-form responses. |
| A13 | Retention/deletion removes raw and derived data | `a13_retention_and_deletion`, `a13_admin_can_hard_delete_a_solution` | Pass |
| A14 | Missing model provider does not break ingestion | `a14_no_provider_mode_ingests_and_retrieves`, `a14_failing_providers_degrade_gracefully`, `a14_external_providers_need_admin_approval` | Pass |
| A15 | Outcome tracking never fabricates savings | `a15_metrics_count_only_reported_and_confirmed_outcomes` | Pass |
| A16 | HTTP API, MCP, search, extraction and jobs run in Rust without a Node backend | Cargo build/tests; `scripts/smoke.sh` process inspection | Pass: the running stack had exactly `api`, `worker`, `web`, `postgres`, with processes `antibody-api`, `antibody-worker`, `caddy` and `postgres`, and no node/npm/pnpm process. |
| A17 | Web UI is React + strict TypeScript, statically served, typed client | `pnpm exec tsc --noEmit`, `pnpm build`, `pnpm run check:api` in `apps/web` | Pass. Static Vite build; types generated from the Rust OpenAPI; no hand-written API models. |
| A18 | Rust OpenAPI and generated TypeScript client stay compatible | `crates/antibody-api/tests/contract.rs` (committed OpenAPI/MCP schemas match the code; unique operationIds; every enum schema equals its wire format; auth errors documented), `pnpm run check:api` | Pass |

## Retrieval latency

`crates/antibody-api/tests/load.rs` (opt-in) seeds 5,000 verified solutions
with embeddings across 5 repositories, then runs 400 searches through the HTTP
API at concurrency 8, mixing exact-error and free-text queries, with the
vector stage enabled (local hash embeddings) and no LLM inference.

```bash
DATABASE_URL=... cargo test -p antibody-api --release --test load -- --ignored --nocapture
```

Earlier local run (2026-10-19): p50 81 ms, p95 94 ms, p99 105 ms,
max 110 ms; target p95 ≤ 1.5 s. This load test was not part of the
2026-10-20 regression run.
This is a single developer machine with API and database on the same host, so
treat it as a sanity check, not a capacity plan.

## Release review regressions

The release review added these dedicated regression suites:

- `antibody-db/tests/integrity.rs`: concurrent extraction/deletion and version
  allocation, shortened retention, and upgrades from the original schema and
  applicability fingerprints.
- `antibody-api/tests/auth_hardening.rs`: atomic account creation, credential
  bounds, rate-limit bypasses, and the actual compromised-account recovery SQL.
- `antibody-api/tests/worker_evidence.rs` and `evidence_ordering.rs`: dirty
  worktree provenance, reruns and delayed CI, changed PR heads, and correlation
  retries after the transcript has been removed.
- `antibody-api/tests/webhook_safety.rs`: redaction before durable writes,
  authenticated-body replay protection, event-header confusion, and repository
  lifecycle changes.
- `antibody-api/tests/search_privacy.rs`, `outcome_provenance.rs`, and
  `outcome_visibility.rs`: query
  redaction before embedding, scoped activity and evidence, and attribution to
  the consuming repository.
- `antibody-core/tests/privacy_and_applicability.rs`: quoted credentials,
  component boundaries, and dependency ecosystems.
- Web deferred-response tests and CLI command-hook/slow-Git tests cover the
  corresponding dashboard and agent-integration races.
- `antibody-api/tests/legacy_evidence.rs` checks that the provenance migration
  flags ambiguous historical CI verification while preserving manual and valid
  committed-fix verification.

The provider-free command-hook check is `tests/agents/real_agent_hooks.sh cli`.
It exercises both command adapters against a real API and verifies one
retrieval per agent session; it does not prove a model used the returned context.

## Last run (2026-10-20)

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean.
- `cargo test --workspace --locked`: 175 passed, 1 ignored (the opt-in load test),
  against disposable PostgreSQL with `max_connections=200` and
  `RUST_TEST_THREADS=6`. The final `evidence_ordering` rerun passed 7/7, including
  one additional regression added after the full run: 176 distinct Rust tests
  passed across the final checks.
- Rust OpenAPI/MCP output matches the checked-in schemas.
- `apps/web`: `check:api`, `pnpm test` (57), `pnpm build` (includes
  `tsc --noEmit`), and real-backend Playwright (4/4): pass.
- `tests/agents/real_agent_hooks.sh cli`: pass for Claude Code and Codex command
  adapters, including empty output on the second prompt in each session.
  Earlier real-model hook evidence remains described in A06; no model provider
  was used in this run.
- `SMOKE_KEEP=1 scripts/smoke.sh` with both real release images: pass.
  The generated teardown script removed the private run directory, containers,
  and volumes; `deploy/.env` remained unchanged. Release builds plus Docker
  build cache need substantial free space.
