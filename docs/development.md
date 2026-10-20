# Development

This guide shows how to build, run, and test Antibody on a local machine.

## Prerequisites

- Rust toolchain `1.97.0` (installed automatically via `rust-toolchain.toml`)
- Docker with Compose 2.24.4 or newer (for PostgreSQL 18 with `pgvector` and the smoke test)
- Node.js 26 and `pnpm` (for the web dashboard in `apps/web`)

## Git Hooks

Install the versioned hooks in `.husky/` once per clone:

```sh
scripts/install-git-hooks.sh
```

- **pre-commit:** Rejects staged files that hold secrets, `.env` files, submodules, files over 10 MiB, merge conflict markers, or whitespace errors. Hand-written source files may not exceed 500 lines; split a file before it reaches the limit.
- **commit-msg:** Rejects Claude/Anthropic co-author credits, generated-by footers, and Claude session links. Human co-authors and required DCO sign-offs remain allowed. Repository Claude settings disable attribution in commits and pull requests.
- **pre-push:** Runs the same checks on every outgoing commit and blocks deleting `main`. It then checks out each pushed commit into a temporary worktree and runs the CI checks that match the changed paths: `rustfmt`, `clippy`, and contract drift for Rust changes, and the generated-client check, `tsc`, and Vitest for web changes. The Postgres test suites and the smoke test run only in CI.

The pre-push hook also rejects forbidden commit attribution. CI independently checks commit history and runs the hook regression tests. Run them locally with `node --test scripts/git-hooks/*.test.mjs`.

The hook logic is in `scripts/git-hooks/run-git-hook.mjs` and `scripts/git-hooks/check-commit-message.mjs`. Keep the hooks enabled; do not bypass the attribution policy.

## 1. Start the Local Database

Start the development PostgreSQL container on port `55432`:

```sh
docker compose -f deploy/compose.dev.yaml up -d
export DATABASE_URL=postgres://antibody:antibody@localhost:55432/antibody
```

## 2. Run Rust Checks and Tests

Format, lint, and test the Rust workspace:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

`cargo test` requires `DATABASE_URL`.

## 3. Run the API and Worker Locally

Set the required environment variables and start the API server:

```sh
export ANTIBODY_SECRET_KEY=$(openssl rand -hex 32)
export ANTIBODY_BOOTSTRAP_ORG_SLUG=dev
export ANTIBODY_BOOTSTRAP_ADMIN_PASSWORD=dev-password-123
export ANTIBODY_BIND=127.0.0.1:18080
export ANTIBODY_PUBLIC_URL=http://localhost:5173
cargo run -p antibody-api -- serve
```

The API listens on `http://127.0.0.1:18080`, matching Vite's default proxy target.
`ANTIBODY_PUBLIC_URL` must match the browser origin (`http://localhost:5173`)
so cookie login and mutations pass the API's Origin checks. By default, the API
accepts plain HTTP only from loopback addresses (`127.0.0.1` and `::1`).

Open a second terminal and start the background worker:

```sh
export DATABASE_URL=postgres://antibody:antibody@localhost:55432/antibody
cargo run -p antibody-worker
```

## 4. Run the Web Dashboard Locally

Open a separate terminal and start the Vite development server:

```sh
cd apps/web
pnpm install --frozen-lockfile
pnpm dev
```

Open `http://localhost:5173`.

Run the frontend type checks, unit tests, and production build:

```sh
cd apps/web
pnpm run check:api
pnpm exec tsc --noEmit
pnpm test
pnpm build
```

## 5. Update API and MCP Contracts

The Rust code owns the OpenAPI and MCP schemas. If you change an HTTP endpoint or an MCP tool, regenerate the schema files and the TypeScript client:

```sh
cargo run -q -p antibody-api -- openapi > openapi/openapi.json
cargo run -q -p antibody-api -- mcp-schema > openapi/mcp-tools.json
(cd apps/web && pnpm run gen:api)
```

Commit the updated files in `openapi/` and `apps/web/src/api/`. CI fails if the committed files do not match the Rust types.

## Continuous Integration

The CI workflow (`.github/workflows/ci.yml`) runs these jobs:

- **Rust:** Runs `rustfmt`, `clippy`, and `cargo test` against a `pgvector` container.
- **Contract Drift:** Verifies that `openapi/openapi.json` and `openapi/mcp-tools.json` match the Rust code.
- **Web:** Verifies the generated TypeScript client, runs `tsc`, runs Vitest unit tests, and builds the static bundle.
- **E2E:** Runs Playwright browser tests against the compiled Rust API and worker.
- **Compose Smoke:** Runs `scripts/smoke.sh` on `main` and on manual workflow runs.
