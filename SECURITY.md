# Security Policy

## Supported versions

Antibody has not had a stable release yet. Security fixes land on `main`. Once releases begin, this table will list which versions receive fixes.

| Version | Supported |
|---|---|
| `main` | Yes |

## Reporting a vulnerability

**Do not open a public issue, discussion, or pull request for a security problem.**

Report it privately through GitHub: open the repository's **Security** tab and choose **Report a vulnerability**. Include:

- what is affected (component, endpoint, MCP tool, or deployment file) and the commit you tested;
- steps to reproduce, or a proof of concept;
- the impact you expect, for example a cross-organization data leak, authentication bypass, or secret exposure.

Use synthetic data in the report. Do not send real transcripts, tokens, or customer data.

## What to expect

- We acknowledge the report within 3 business days.
- We send an initial assessment within 10 business days.
- We agree on a disclosure date with you. The default is 90 days after the report, or sooner once a fix is released.
- If you want credit, we name you in the advisory.

## Scope

In scope:

- Authorization and isolation between organizations and repositories (see acceptance criterion A09 in [spec.md](spec.md)).
- Authentication, API tokens, browser sessions, CSRF, and webhook signature checks.
- Secret redaction, retention, and deletion of raw and derived data.
- Any path where transcripts, model output, or retrieved text could lead to command execution.
- Private content reaching an external model provider without administrator approval.
- The Docker Compose deployment in `deploy/`.

Out of scope:

- Deployments that ignore the documented configuration, for example plain HTTP exposed to untrusted networks.
- Vulnerabilities in model providers, agent clients, or GitHub themselves. Report those to the vendor.
- Denial of service that needs valid administrator credentials.

## Running Antibody securely

[docs/security.md](docs/security.md) describes the threat model, secret redaction, and the incident response runbook for leaked secrets and tokens.
