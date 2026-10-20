# Contributing to Antibody

Thanks for helping. This guide covers how to propose changes and what a pull
request needs before it can merge.

## Ground rules

- Be respectful. Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).
- Report security problems privately as described in [SECURITY.md](SECURITY.md), never in a public issue.
- Never paste real session transcripts, CI logs, tokens, or customer data into issues, pull requests, or test fixtures. Use synthetic data.

## Before you start

- For a bug, open an issue with steps to reproduce, the expected and actual behavior, and versions (Antibody commit, PostgreSQL, agent client).
- For a feature or a change to an API, MCP tool, or database schema, open an issue first so the design can be agreed before you write code.
- Small fixes (typos, docs, obvious bugs) can go straight to a pull request.

## Development setup

[docs/development.md](docs/development.md) covers the toolchain, the local database, and how to run the API, worker, and web app.

## Pull request checklist

- [ ] Every commit is signed off (see [Developer Certificate of Origin](#developer-certificate-of-origin)).
- [ ] `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings` pass.
- [ ] `cargo test --workspace --locked` passes against a local PostgreSQL with pgvector.
- [ ] For web changes: `pnpm run check:api`, `pnpm exec tsc --noEmit`, `pnpm test`, and `pnpm build` pass in `apps/web`.
- [ ] If you changed an HTTP endpoint or MCP tool, the committed OpenAPI and MCP schemas in `openapi/` are regenerated and the contract tests pass.
- [ ] New behavior has tests. If it maps to an acceptance criterion in [spec.md](spec.md), name the ID (for example A07) in the test or the PR.
- [ ] User-facing changes are documented in `docs/` or `README.md`.

Keep pull requests focused. A reviewer should be able to understand one change at a time.

## Design constraints

These come from the spec and are not up for debate in an individual PR:

- The backend, workers, search, and MCP server are Rust. No Node.js backend.
- PostgreSQL (with pgvector) is the only required datastore.
- Model providers stay behind the traits in `crates/antibody-providers`. Private content goes to an external provider only after an administrator approves it.
- Transcripts, model output, and retrieved text are untrusted data. Never execute anything derived from them.

## Developer Certificate of Origin

Antibody uses the [Developer Certificate of Origin 1.1](https://developercertificate.org/) (DCO) instead of a contributor license agreement. By signing off a commit you certify that you wrote the change, or otherwise have the right to submit it under the project's license (Apache-2.0).

Sign off each commit with `-s`:

```sh
git commit -s -m "Fix applicability ranking for monorepo components"
```

This adds a line with your real name and email:

```text
Signed-off-by: Jane Doe <jane@example.com>
```

To sign off commits you already made on your branch:

```sh
git rebase --signoff main
```

CI rejects pull requests that contain commits without a sign-off.

## License

By contributing, you agree that your contributions are licensed under the [Apache License 2.0](LICENSE).
