# Antibody

[![CI](https://github.com/magneticenterprises/antibody/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/magneticenterprises/antibody/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Antibody is a self-hosted shared memory service for coding agents. It records how agents fix problems and shares verified solutions with your team. This stops engineers and agents from investigating the same problem twice.

## How It Works

1. Engineer A fixes a build error with a coding agent.
2. When the session ends, a hook sends the redacted session to Antibody.
3. A worker extracts the symptom, root cause, remedy, and applicability rules.
4. The solution starts in the `candidate` state.
5. A merged pull request moves the solution to the `corroborated` state.
6. A signed CI check on the fix moves the solution to the `verified` state.
7. Later, Engineer B or a CI job encounters the same error.
8. Antibody returns up to three matching solutions with source links and verification status.
9. The agent uses, adapts, or rejects the solution and reports the outcome.

### Scope Limits

- Antibody does not build a coding agent.
- Antibody does not replace Git or CI.
- Antibody never changes repository code or applies patches automatically.
- Antibody does not run a background daemon on developer machines.

## Quick Start

**Requirements:** Docker with Compose 2.24.4 or newer. You do not need third-party accounts.

### 1. Create the configuration file

Copy the example environment file:

```sh
cp deploy/env.example deploy/.env
```

Open `deploy/.env` and set these three variables:

- `POSTGRES_PASSWORD`: Set a password for the PostgreSQL database.
- `ANTIBODY_SECRET_KEY`: Run `openssl rand -hex 32` and use the output.
- `ANTIBODY_BOOTSTRAP_ADMIN_PASSWORD`: Set an administrator password (minimum 12 characters).

### 2. Start the services

Build and start the four containers:

```sh
docker compose -f deploy/compose.yaml up -d --build
```

The first build compiles the Rust services and takes several minutes. Only the web container publishes a port (`127.0.0.1:8080` by default).

### 3. Sign in and create an API token

1. Open <http://localhost:8080> in your browser.
2. Sign in as `admin` with the password from `deploy/.env`.
3. Register your repositories in the dashboard.
4. Create an API token for your coding agents.

### 4. Verify the installation (optional)

Run the system test script to verify the full stack:

```sh
scripts/smoke.sh
```

This script builds the images, ingests a session, runs a search, and checks the container processes.

### Model Providers (optional)

Antibody works without an external model provider. By default, Antibody uses rule-based extraction and runs exact-signature, symbol, path, and full-text searches.

- To enable vector search, configure an embedding provider in `deploy/.env`.
- To improve solution extraction, configure an LLM provider in `deploy/.env`.

You can use self-hosted model providers (any OpenAI-compatible server, such as Ollama or vLLM) or the hosted presets: `anthropic`, `openai`, `gemini`, and `deepseek` for extraction, and `openai` or `gemini` for embeddings. Set the vendor's API key in `deploy/.env`; see `deploy/env.example`. Antibody sends private content to a hosted provider only after an administrator approves external providers in Settings.

## Connect Agents and CI

Copy-ready configuration files are in [`integrations/`](integrations/).

| Integration | Setup Guide |
| --- | --- |
| Claude Code (HTTP hook, session upload, MCP) | [`docs/integrations/claude-code.md`](docs/integrations/claude-code.md) |
| Codex CLI (hooks, MCP) | [`docs/integrations/codex.md`](docs/integrations/codex.md) |
| CI Failure Lookup (GitHub Actions, generic CI) | [`docs/integrations/ci.md`](docs/integrations/ci.md) |

## Documentation

| Document | Topic |
| --- | --- |
| [`docs/architecture.md`](docs/architecture.md) | System components, repository layout, data model, and workflows |
| [`docs/development.md`](docs/development.md) | Local development, test commands, and schema generation |
| [`docs/operations.md`](docs/operations.md) | Deployment, HTTPS, backups, upgrades, and monitoring |
| [`docs/api.md`](docs/api.md) | HTTP API, MCP tools, webhooks, and error codes |
| [`docs/security.md`](docs/security.md) | Threat model, secret redaction, data retention, and incident response |
| [`docs/acceptance.md`](docs/acceptance.md) | Acceptance criteria and verification status |
| [`spec.md`](spec.md) | Full v1 implementation specification |

## License

Apache-2.0. See [`LICENSE`](LICENSE). Contributions use the Developer Certificate of Origin; see [`CONTRIBUTING.md`](CONTRIBUTING.md). Report security problems as described in [`SECURITY.md`](SECURITY.md).

[![GitHub stars](https://img.shields.io/github/stars/magneticenterprises/antibody?style=flat&logo=github)](https://github.com/magneticenterprises/antibody/stargazers)