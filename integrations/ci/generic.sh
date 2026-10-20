#!/usr/bin/env bash
# Report a failed check to Antibody with a signed webhook, using only curl + openssl.
# Required env: ANTIBODY_URL ANTIBODY_ORG ANTIBODY_CI_SECRET REPOSITORY (owner/name) REVISION CHECK
# Optional: EXCERPT_FILE (last lines of the failing log). Never blocks the build.
set -u
body=$(
  tail -c 16000 "${EXCERPT_FILE:-/dev/null}" 2>/dev/null | python3 -c '
import json, os, sys
e = {"repository": os.environ["REPOSITORY"], "revision": os.environ["REVISION"], "check_name": os.environ["CHECK"]}
x = sys.stdin.read()
if x.strip():
    e["excerpt"] = x
print(json.dumps(e), end="")'
) || exit 0
ts=$(date +%s)
delivery=$(openssl rand -hex 16)
# Signature is HMAC-SHA256 over "<timestamp>.<raw body>".
sig=$(printf '%s.%s' "$ts" "$body" | openssl dgst -sha256 -hmac "$ANTIBODY_CI_SECRET" -hex | sed 's/^.* //')
curl -sS --max-time 20 -X POST "${ANTIBODY_URL%/}/v1/webhooks/ci" \
  -H "Content-Type: application/json" \
  -H "X-Antibody-Org: $ANTIBODY_ORG" \
  -H "X-Antibody-Delivery: $delivery" \
  -H "X-Antibody-Timestamp: $ts" \
  -H "X-Antibody-Signature: sha256=$sig" \
  --data-binary "$body" || true
exit 0
