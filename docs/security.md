# Security

This document describes the Antibody threat model, secret redaction rules, data retention controls, and incident response steps.

## Threat Model

**Protected assets:**

- Source code context and discussions inside coding agent transcripts
- Credentials that appear accidentally in transcripts or CI logs
- Extracted solutions and embeddings
- API tokens, administrator accounts, and the webhook master key (`ANTIBODY_SECRET_KEY`)

### Trust Boundaries

| Input or Component | Security Treatment |
| --- | --- |
| Agent transcripts and hints | Untrusted. Size-limited, validated, redacted before storage, and parsed strictly as data. |
| Model (LLM) output | Untrusted. Parsed against a strict schema, bounded in length, and redacted again. |
| Retrieved solutions and CI excerpts | Untrusted data. Responses carry an untrusted-data notice, and the web UI renders text without HTML or Markdown execution. |
| Webhook payloads | Untrusted until HMAC-SHA256 verification succeeds on the raw bytes. Replay deliveries are rejected. |
| API and MCP callers | Authenticated on every request. Repository access is checked before search and ranking. |
| Network transport | Plain HTTP is allowed only from `ANTIBODY_HTTP_TRUSTED_NETWORKS`. All other traffic requires HTTPS. |
| Host, PostgreSQL, Caddy | Trusted infrastructure. Encrypt the host disk or storage volume to protect data at rest. |

**Out of scope:** A compromised host OS, a malicious organization administrator, or a compromised developer workstation.

## No Execution of Derived Content

Antibody never executes commands, scripts, or patches from transcripts, model output, repositories, or webhooks. The MCP server exposes only three tools (`find_relevant_context`, `get_solution`, `report_outcome`). Antibody never modifies repository files.

## Secret Redaction (Best Effort)

Antibody redacts recognizable secrets from every string before database storage and redacts extracted solutions, CI excerpts, and outcome notes again.

Redaction uses pattern matching (`crates/antibody-core/src/redact.rs`) and replaces matches with `[REDACTED:<kind>]`:

- `private_key` (PEM blocks)
- `antibody_token` (`abt_...`)
- `aws_access_key`
- `github_token` (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`)
- `gitlab_token`, `slack_token`, `anthropic_key`, `openai_key`, `google_api_key`, `stripe_key`, `npm_token`
- `jwt`, `bearer_token`, `basic_auth_header`, `url_credentials` (`scheme://user:pass@`)
- `secret_assignment` (keys such as `password`, `secret`, `api_key`, `access_token`, `client_secret`, `private_key`, or `signing_key` followed by `=` or `:` and a value of 6 or more characters)

**Important limitation:** Pattern matching cannot detect unstructured passwords, secrets split across lines, or proprietary source code. Treat stored transcripts as sensitive data.

## Capture Modes, Retention, and Deletion

- **Structured-only mode (default):** The worker deletes the redacted transcript in the same transaction that completes extraction. A 7-day cleanup rule deletes payloads if extraction never completes.
- **Raw capture mode:** Requires an administrator to set `raw_capture_enabled = true` and `raw_retention_days > 0` (default `30`, maximum `3650`). The worker retention sweep deletes raw payloads after `raw_retention_days`.
- **Audited transcript access:** Only administrators can read retained raw payloads (`GET /v1/sessions/{id}/transcript`). Each read writes a `session.transcript_viewed` audit event.

### Delete a Session (`DELETE /v1/sessions/{id}`)

This admin endpoint runs in a single transaction and deletes:

- The session record, its raw payload, and its source artifact.
- Every solution supported *only* by that session (including its embeddings, evidence, and source links).
- Pending, running, and dead jobs for that session.

If other sessions also support a solution, the solution stays in the database (`solutions_detached` in the response). To delete a specific solution and all of its derived records regardless of sessions, call `DELETE /v1/solutions/{id}`.

## Authorization and Isolation

- Every database row belongs to an organization.
- Search computes the caller's accessible repositories before querying and filters candidates again before returning results.
- Missing repositories and unauthorized repositories return identical `repository_access_denied` errors.
- Disconnected repositories drop out of all search scopes immediately.

## Browser Sessions and CSRF

Dashboard logins create server-side sessions stored as SHA-256 hashes (default TTL: 12 hours via `ANTIBODY_SESSION_TTL_HOURS`). Session cookies set `HttpOnly`, `SameSite=Strict`, and (on HTTPS) `Secure` with the `__Host-` prefix. Mutating cookie requests require a valid `X-CSRF-Token` header and a same-origin `Origin` header. Passwords use Argon2id hashing.

Every request consumes a client-IP rate-limit budget; callers behind the same NAT or proxy share that budget. Requests carrying a bearer token or the configured session cookie also consume a separate budget for that credential, independent of header whitespace or unrelated cookies. Both budgets use `ANTIBODY_RATE_LIMIT_PER_MINUTE`; login attempts also use `ANTIBODY_LOGIN_RATE_LIMIT_PER_MINUTE`. Fully replenished IP and credential buckets are removed during periodic request-driven cleanup.

## External Model Providers

Antibody classifies model provider URLs as `local` (loopback, RFC 1918 private networks, link-local, `.local`, `.internal`, or single-label hostnames) or `external`. Antibody sends transcripts or queries to an `external` provider only when an administrator sets `external_providers_approved = true`.

## Incident Response Runbook

Always revoke or rotate the leaked credential at its source first, then clean up Antibody records.

### Scenario A: A secret was uploaded in a session or CI log

1. **Revoke the credential immediately** in the target system (cloud provider, GitHub, database).
2. **Locate the session:** Query `GET /v1/sessions` and `GET /v1/admin/audit` for uploads from that repository and time window.
3. **Delete the session:** Call `DELETE /v1/sessions/{id}`.
4. **Check detached solutions:** If `solutions_detached` is greater than `0`, search with `GET /v1/solutions?q=...` and delete affected solutions with `DELETE /v1/solutions/{id}`.
5. **Check CI events:** If a `ci_events` excerpt contains the secret, delete the row in PostgreSQL (`DELETE FROM ci_events WHERE id = ...`).
6. **Rotate backups and exports:** Delete or restrict database backups and `GET /v1/admin/export` files created after the leak.
7. **Reduce retention:** Lower `raw_retention_days` (set `0` for `structured_only`) and trigger `POST /v1/admin/retention/sweep`.

### Scenario B: An Antibody API token leaked

1. Revoke the token with `DELETE /v1/admin/tokens/{id}`.
2. Check `last_used_at` and `GET /v1/admin/audit` for activity from that principal.
3. Issue a new token with minimal scopes and a repository allow-list.

### Scenario C: An administrator password leaked

1. Ensure another administrator in the affected organization can sign in. If necessary, create a replacement user with `POST /v1/admin/principals` using a new username and a unique password, and verify the new login before continuing. Keep that administrator's session available.
2. Revoke the compromised principal in PostgreSQL using the transaction below. This disables its old username/password, API tokens, and browser sessions. Token revocation and session deletion alone do **not** disable a leaked password. Replace the two example values with the affected organization slug and username; the transaction refuses to proceed unless exactly one account matches and another active administrator user remains.

   ```sql
   \set ON_ERROR_STOP on
   \set organization_slug 'acme'
   \set compromised_username 'old-admin'

   BEGIN;
   CREATE TEMP TABLE recovery_target ON COMMIT DROP AS
     SELECT p.id AS principal_id, p.organization_id
       FROM principals p
       JOIN users u ON u.principal_id = p.id AND u.organization_id = p.organization_id
       JOIN organizations o ON o.id = p.organization_id
      WHERE o.slug = :'organization_slug' AND u.username = :'compromised_username';

   DO $$
   BEGIN
     IF (SELECT count(*) FROM recovery_target) <> 1 THEN
       RAISE EXCEPTION 'Expected exactly one compromised account; check slug and username';
     END IF;
     IF NOT EXISTS (
       SELECT 1 FROM principals p
       JOIN users u ON u.principal_id = p.id AND u.organization_id = p.organization_id
       JOIN recovery_target t ON t.organization_id = p.organization_id
       WHERE p.id <> t.principal_id AND p.role = 'admin' AND p.revoked_at IS NULL
     ) THEN
       RAISE EXCEPTION 'Create and verify a replacement administrator first';
     END IF;
   END $$;

   UPDATE principals p SET revoked_at = COALESCE(p.revoked_at, now())
     FROM recovery_target t
    WHERE p.id = t.principal_id AND p.organization_id = t.organization_id;
   UPDATE api_tokens a SET revoked_at = COALESCE(a.revoked_at, now())
     FROM recovery_target t
    WHERE a.principal_id = t.principal_id AND a.organization_id = t.organization_id;
   DELETE FROM web_sessions s USING recovery_target t
    WHERE s.principal_id = t.principal_id AND s.organization_id = t.organization_id;
   INSERT INTO audit_events
     (organization_id, actor_label, action, resource_type, resource_id, metadata)
     SELECT organization_id, 'operator:password-compromise', 'principal.revoked',
            'principal', principal_id::text, '{}'::jsonb FROM recovery_target;
   COMMIT;
   ```

   If a guard aborts the transaction in an interactive PostgreSQL session, run `ROLLBACK;` before correcting the inputs or creating the replacement account.

3. Confirm the compromised password, old token, and old browser session are rejected, and the replacement administrator still works. Keep the compromised principal revoked; use the replacement account going forward.
4. Audit recent events (`auth.login`, `auth.login_failed`, `webhook_secret.revealed`, `session.transcript_viewed`, `organization.exported`). Rotate any other credentials exposed during the compromise.

### Scenario D: `ANTIBODY_SECRET_KEY` or a webhook secret leaked

1. If a single organization's webhook secret leaked, call `POST /v1/admin/webhook-secrets` with `{"rotate": true}` and update GitHub and CI.
2. If `ANTIBODY_SECRET_KEY` leaked, generate a new key in `deploy/.env`, restart `api`, fetch the new webhook secrets via `POST /v1/admin/webhook-secrets`, and update GitHub and CI.
3. Review recent `solution.verified` audit events and withdraw or revalidate any suspect solutions.
