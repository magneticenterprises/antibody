# CI Integration

CI events verify candidate solutions and match failing builds against known fixes. CI lookups never fail or block a build.

## Options

- **GitHub Webhook (`POST /v1/webhooks/github`):** Recommended for GitHub repositories. Requires no CLI installation.
- **`antibody ci report` CLI command:** Run as a post-failure step in any CI system. Always exits with code `0`.
- **`integrations/ci/generic.sh` script:** Uses `curl` and `openssl` when you do not want to install the CLI binary.

## Use `antibody ci report`

Set these environment variables in your CI job:

- `ANTIBODY_URL`
- `ANTIBODY_TOKEN`
- `ANTIBODY_ORG`
- `ANTIBODY_CI_SECRET`
- `ANTIBODY_REPOSITORY` (optional if running inside a Git checkout with an `origin` remote)

Run the command when a test or build step fails:

```sh
antibody ci report --check test --revision "$SHA" --excerpt-file test.log --url "$RUN_URL" || true
```

The CLI sends the last 16 KB of the log file (the server redacts and truncates it to 4,000 characters) and signs the request with HMAC-SHA256 (`X-Antibody-Signature`). See `integrations/ci/github-actions.yml` for a complete GitHub Actions example.

## Troubleshooting

- The CLI writes errors to `stderr` only and exits with `0`.
- `401` or `403` indicates an invalid organization slug or webhook secret.
- `400` with a timestamp error means the CI runner clock differs from the server clock by more than 300 seconds.
- Run `antibody doctor` to verify server connectivity.
