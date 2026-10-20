# Operations

This guide covers deployment, network configuration, backups, upgrades, and monitoring for the Docker Compose stack in `deploy/`. Run all commands from the repository root. See `deploy/env.example` for a full list of environment variables.

## Deploy

1. Copy the example environment file and set `POSTGRES_PASSWORD`, `ANTIBODY_SECRET_KEY`, and `ANTIBODY_BOOTSTRAP_ADMIN_PASSWORD`:

   ```sh
   cp deploy/env.example deploy/.env
   ```

2. Build and start the containers:

   ```sh
   docker compose -f deploy/compose.yaml up -d --build
   docker compose -f deploy/compose.yaml ps
   ```

The stack runs four containers:

- `postgres`: `pgvector/pgvector:pg18-trixie` with data in the `antibody-pg` volume at `/var/lib/postgresql`.
- `api`: Non-root `debian:trixie-slim` container running `antibody-api`.
- `worker`: Non-root `debian:trixie-slim` container running `antibody-worker`.
- `web`: Caddy serving the static UI and proxying `/v1/*` and `/mcp*` to `api`.

Only the `web` container publishes a host port.

### Logs

Containers output JSON logs by default. Set `ANTIBODY_LOG_FORMAT=text` for human-readable output. Logs never include transcripts, credentials, request bodies, or query strings.

View API and worker logs:

```sh
docker compose -f deploy/compose.yaml logs -f api worker
```

The default log filter is `info,sqlx=warn,tower_http=info`. Set `RUST_LOG=info,tower_http=debug` to log HTTP status codes and latency for each request.

### First Administrator

On an empty database, the API creates the initial organization and administrator from the `ANTIBODY_BOOTSTRAP_*` variables in `deploy/.env`. After the first start, remove `ANTIBODY_BOOTSTRAP_ADMIN_PASSWORD` from `deploy/.env`.

To create the organization and an API token manually from the CLI instead:

```sh
docker compose -f deploy/compose.yaml run --rm -e ADMIN_PW='a-long-password' api \
  antibody-api bootstrap --org-slug acme --org-name "Acme" --admin-username admin --admin-password-env ADMIN_PW

docker compose -f deploy/compose.yaml run --rm api \
  antibody-api create-token --org-slug acme --name ci-bot --scopes sessions:write,context:read,outcomes:write
```

Manage tokens, principals, and repositories in the web dashboard or through the admin API ([`api.md`](api.md)). Manage webhook secrets at `POST /v1/admin/webhook-secrets`.

### Network Exposure and HTTPS

- **Local machine only (default):** Keep `ANTIBODY_HTTP_BIND=127.0.0.1`, `ANTIBODY_SITE_ADDRESS=:8080`, and `ANTIBODY_PUBLIC_URL=http://localhost:8080`.
- **Trusted LAN over HTTP:** Set `ANTIBODY_HTTP_BIND=0.0.0.0` and set `ANTIBODY_PUBLIC_URL` to the LAN URL. The API accepts plain HTTP only from CIDR ranges in `ANTIBODY_HTTP_TRUSTED_NETWORKS`. Restrict this variable to your trusted LAN subnet.
- **Public or remote access (HTTPS required):**
  1. Point a DNS record at your host and open ports `80` and `443`.
  2. In `deploy/.env`, set `ANTIBODY_SITE_ADDRESS=antibody.example.com` and `ANTIBODY_PUBLIC_URL=https://antibody.example.com`.
  3. Remove non-loopback subnets from `ANTIBODY_HTTP_TRUSTED_NETWORKS`.
  4. Start the stack with the TLS override file:

     ```sh
     docker compose -f deploy/compose.yaml -f deploy/compose.tls.yaml up -d --build
     ```

  Caddy obtains and renews TLS certificates automatically in the `antibody-caddy-data` volume.
- **Behind an external load balancer:** Add `servers { trusted_proxies static <load-balancer-cidr> }` to the global block in `deploy/Caddyfile` and set `ANTIBODY_PUBLIC_URL` to the external `https://` URL.

Always set `ANTIBODY_PUBLIC_URL` to the exact URL that clients use. The API uses this value for browser `Origin` checks, cookie security flags, the MCP `Host` allow-list (extendable via `ANTIBODY_ALLOWED_HOSTS`), and result links.

## Select the Deployment for Maintenance

Use the same Compose files for migrations, backup, restore, upgrades, and scaling
that you used to deploy. Define one of these helpers in your current shell before
running the commands below.

For a local or LAN HTTP deployment:

```sh
compose() { docker compose -f deploy/compose.yaml "$@"; }
```

For a public HTTPS deployment, keep the TLS override on every command:

```sh
compose() { docker compose -f deploy/compose.yaml -f deploy/compose.tls.yaml "$@"; }
```

Keep any additional deployment override files in this helper too. Using only the
base file during restore or upgrade would replace the HTTPS port configuration.

## Database Migrations

The binaries embed the SQL migrations from `crates/antibody-db/migrations`. By default (`ANTIBODY_MIGRATE=true`), the API and worker apply pending migrations on startup using a database lock.

To run migrations as a standalone command:

```sh
compose run --rm api antibody-api migrate
```

Migrations move forward only. To roll back an upgrade, restore a database backup taken before the upgrade.

## Backup and Restore

All application state lives in PostgreSQL (`antibody-pg` volume). TLS certificates live in `antibody-caddy-data`.

### Create a Backup

Run `pg_dump` in custom format while the stack is running:

```sh
compose exec -T postgres \
  pg_dump -U antibody -d antibody --format=custom --no-owner > antibody-$(date +%F).dump
```

Backups contain redacted transcripts (when raw capture is enabled), extracted solutions, and token hashes. Encrypt backup files and delete old backups according to your retention policy.

### Restore a Backup

1. Stop the application containers:

   ```sh
   compose stop api worker web
   ```

2. Recreate the database and restore the dump:

   ```sh
   compose exec -T postgres psql -U antibody -d postgres \
     -c 'DROP DATABASE IF EXISTS antibody' -c 'CREATE DATABASE antibody OWNER antibody'
   compose exec -T postgres \
     pg_restore -U antibody -d antibody --no-owner --exit-on-error < antibody-2026-10-19.dump
   ```

3. Start the containers:

   ```sh
   compose up -d
   ```

4. Re-apply any session or solution deletions that occurred after the backup timestamp.

## Upgrades

1. Create a database backup.
2. Pull the new release and rebuild the containers:

   ```sh
   git pull
   compose up -d --build
   ```

3. Check container status (`compose ps`) and readiness (`GET /v1/health/ready`).
4. If the upgrade fails, restore the database backup and deploy the previous version.

Keep `ANTIBODY_SECRET_KEY` unchanged across upgrades so webhook secrets stay valid. If you change the embedding provider or model, restart the worker so it backfills embeddings for the new model.

Migration `0003_worker_evidence.sql` marks older CI-verified solutions for
revalidation when their committed fix provenance cannot be established. It
preserves historical evidence and independent manual verification. Review the
flagged solution's fix and current evidence before recording a new verification.

## Data Retention

The worker checks for maintenance once per minute and schedules at most one
retention sweep per hour. The sweep deletes:

- Raw session payloads past their retention window (`raw_retention_days`, default `30`, `0` for `structured_only`).
- Expired browser sessions.
- Webhook delivery diagnostics older than 30 days. Permanent SHA-256 payload
  fingerprints retain replay protection after those rows expire; fingerprints
  contain no stored webhook payload.
- Completed jobs older than 14 days.

Administrators can trigger an immediate sweep with `POST /v1/admin/retention/sweep`, delete a session with `DELETE /v1/sessions/{id}`, or export organization metadata with `GET /v1/admin/export`.

## Monitoring

Antibody reports operational metrics at `GET /v1/overview` and on the dashboard Overview page.

| Metric | Source | Action Required |
| --- | --- | --- |
| Ingestion errors | API logs (`/v1/sessions`), `stats.extraction_failed` | For `401`/`403`, check tokens and grants. For `413`, increase `ANTIBODY_MAX_SESSION_BYTES`. |
| Queue age | `queue.oldest_queued_age_seconds`, `queue.queued` | If age grows continuously, start more workers or check model provider latency. |
| Dead jobs | `queue.dead`, `GET /v1/admin/jobs?state=dead` | Inspect `last_error`, fix the underlying issue, and retry the job. |
| Search latency | `stats.retrieval_p95_latency_ms_7d` (target <= 1500 ms) | Check PostgreSQL load, `ANTIBODY_SEARCH_TIMEOUT_MS`, and embedding provider latency. |
| Rejected outcomes | `outcomes_rejected`, `outcomes_inapplicable` | Review applicability rules and withdraw inaccurate solutions. |
| Confirmed reuse | `stats.confirmed_reuse` | Counts only `used` outcomes confirmed by a signed passing CI check. |
| Stale solutions | `solutions_needing_revalidation` | Review flagged solutions and clear the flag or withdraw them. |

Check the job queue directly in PostgreSQL:

```sh
compose exec -T postgres psql -U antibody -d antibody -c \
  "SELECT state, kind, count(*), now() - min(run_at) FILTER (WHERE state='queued') AS oldest FROM jobs GROUP BY 1,2 ORDER BY 1,2"
```

Health endpoints:

- `GET /v1/health/live`: Returns `200` when the API process is running.
- `GET /v1/health/ready`: Returns `200` when PostgreSQL, `pgvector`, and all SQL migrations are ready; otherwise returns `503`.

## Dead-Letter Jobs

Failed jobs retry up to 8 times with exponential backoff and then move to `state = dead`. To recover a dead job:

1. Call `GET /v1/admin/jobs?state=dead` and inspect `last_error`.
2. Fix the root cause (for example, restore connectivity to the model provider).
3. Requeue the job with `POST /v1/admin/jobs/{id}/retry`.

If `last_error` is `payload no longer available`, the retention sweep already deleted the raw payload and the session cannot be re-extracted.

## Scaling

- **Scale workers:** Run `compose up -d --scale worker=3`. Adjust per-worker concurrency with `ANTIBODY_WORKER_CONCURRENCY` (default `4`, max `64`) and lease duration with `ANTIBODY_WORKER_LEASE_SECS` (default `120`).
- **Database connections:** Each worker uses `concurrency + 4` (minimum 8) connections. The API uses up to `ANTIBODY_DB_MAX_CONNECTIONS` (default `20`). Keep the total below PostgreSQL `max_connections` (default `100`).
