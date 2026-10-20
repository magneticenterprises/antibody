#!/usr/bin/env bash
# A01 / A16 Compose smoke test.
#
# Builds and starts the four-container stack with a throwaway configuration,
# then exercises the main path over HTTP exactly as a user would:
#   health -> admin login -> repository + service token -> idempotent session
#   ingest -> extraction -> context search -> process inspection -> teardown.
#
# Requirements: docker (with compose >= 2.24.4), curl, python3, openssl.
# Usage: scripts/smoke.sh
#   SMOKE_PORT=18080            host port for the web container
#   SMOKE_KEEP=1                leave the stack running afterwards
#   SMOKE_COMPOSE_OVERRIDE=f    extra compose file merged after deploy/compose.yaml
#
# Each run owns a private temporary environment file and unique Compose project.
# deploy/.env is never read or changed. SMOKE_KEEP retains the private run
# directory, including a teardown script, until that script removes the stack.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROJECT="antibody-smoke-$$-${RANDOM}"
PORT="${SMOKE_PORT:-18080}"
BASE="http://127.0.0.1:${PORT}"
ORIGIN="http://localhost:${PORT}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/antibody-smoke.XXXXXX")"
ENV_FILE="$WORK/config.env"
# Shell variables override --env-file; exclude deployment values from this run.
COMPOSE=(env -u POSTGRES_PASSWORD -u ANTIBODY_SITE_ADDRESS -u ANTIBODY_HTTP_BIND -u ANTIBODY_HTTP_PORT
  docker compose --env-file "$ENV_FILE" -f "$ROOT/deploy/compose.yaml")
# Optional extra compose file (for example to stub a service while debugging).
[ -n "${SMOKE_COMPOSE_OVERRIDE:-}" ] && COMPOSE+=(-f "$SMOKE_COMPOSE_OVERRIDE")
COMPOSE+=(-f "$WORK/environment.yaml" -p "$PROJECT")
STARTED=0

log()  { printf '\n==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

for tool in docker curl python3 openssl; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done

cleanup() {
  local rc=$?
  set +e
  if [ "$rc" -ne 0 ] && [ "$STARTED" = 1 ]; then
    echo "--- compose ps ---" >&2
    "${COMPOSE[@]}" ps -a >&2 2>/dev/null
    echo "--- last logs (api, worker, web) ---" >&2
    "${COMPOSE[@]}" logs --tail 40 api worker web >&2 2>/dev/null
  fi
  if [ "${SMOKE_KEEP:-0}" != "1" ]; then
    if [ "$STARTED" = 0 ] || "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1; then
      rm -rf -- "$WORK"
    else
      echo "Cleanup failed; retry with: $WORK/teardown.sh" >&2
      rc=1
    fi
  else
    echo "Stack left running; private configuration retained in $WORK" >&2
    printf 'Tear it down with: %q\n' "$WORK/teardown.sh" >&2
  fi
  [ "$rc" -eq 0 ] && echo && echo "SMOKE TEST PASSED" || echo "SMOKE TEST FAILED (exit $rc)" >&2
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# JSON helper: json_get '<json>' 'a.b.0.c' prints the value (strings raw, others as JSON).
json_get() {
  python3 -I -c '
import json, sys
data = json.loads(sys.argv[1])
for part in sys.argv[2].split("."):
    data = data[int(part)] if isinstance(data, list) else data[part]
print(data if isinstance(data, str) else json.dumps(data))
' "$1" "$2"
}

# ---------------------------------------------------------------------------
log "Preparing throwaway configuration"
DB_PASSWORD="$(openssl rand -hex 16)"
SECRET_KEY="$(openssl rand -hex 32)"
ADMIN_PASSWORD="smoke-$(openssl rand -hex 12)"
umask 077
cat > "$ENV_FILE" <<ENV
POSTGRES_PASSWORD=${DB_PASSWORD}
ANTIBODY_SECRET_KEY=${SECRET_KEY}
ANTIBODY_SITE_ADDRESS=:8080
ANTIBODY_HTTP_BIND=127.0.0.1
ANTIBODY_HTTP_PORT=${PORT}
ANTIBODY_PUBLIC_URL=${ORIGIN}
ANTIBODY_MIGRATE=true
ANTIBODY_LOG_FORMAT=json
ANTIBODY_HTTP_TRUSTED_NETWORKS=127.0.0.0/8,::1/128,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16
ANTIBODY_EMBEDDING_PROVIDER=none
ANTIBODY_LLM_PROVIDER=none
ANTIBODY_BOOTSTRAP_ORG_SLUG=smoke
ANTIBODY_BOOTSTRAP_ORG_NAME=Smoke
ANTIBODY_BOOTSTRAP_ADMIN_USERNAME=admin
ANTIBODY_BOOTSTRAP_ADMIN_PASSWORD=${ADMIN_PASSWORD}
ENV

# Replace the service env_file entries as well as Compose interpolation. Keep
# images unique too, so concurrent builds do not retag another run's images.
python3 -I - "$ENV_FILE" "$PROJECT" > "$WORK/environment.yaml" <<'PY'
import json, sys
env_file, project = sys.argv[1:]
print("services:")
for service in ("api", "worker", "web"):
    print(f"  {service}:")
    image = "api" if service == "worker" else service
    print(f"    image: {project}-{image}:local")
    if service != "web":
        print(f"    env_file: !override [{json.dumps(env_file)}]")
PY
{
  printf '#!/usr/bin/env bash\nset -euo pipefail\n'
  printf 'cd %q\n' "$ROOT"
  printf '%q ' "${COMPOSE[@]}"
  printf 'down -v --remove-orphans\n'
  printf 'rm -rf -- %q\n' "$WORK"
} > "$WORK/teardown.sh"
chmod 700 "$WORK/teardown.sh"

log "Building and starting the stack (the first Rust build takes several minutes)"
STARTED=1
"${COMPOSE[@]}" up -d --build --wait --wait-timeout 300

# ---------------------------------------------------------------------------
log "Readiness (through the web container)"
READY="$(curl -fsS "$BASE/v1/health/ready")" || fail "readiness probe failed"
[ "$(json_get "$READY" status)" = "ready" ] || fail "not ready: $READY"
echo "$READY"

log "Static UI is served with security headers"
HEADERS="$(curl -sS -D - -o /dev/null "$BASE/")"
echo "$HEADERS" | grep -qi '^content-security-policy:.*frame-ancestors' || fail "CSP header missing on /"
echo "$HEADERS" | grep -qi '^cache-control: *no-cache' || fail "index.html is not no-cache"

log "Unauthenticated API access is rejected"
CODE="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/v1/repositories")"
[ "$CODE" = "401" ] || fail "expected 401 without credentials, got $CODE"

# ---------------------------------------------------------------------------
log "Admin login (cookie session)"
JAR="$WORK/cookies.txt"
LOGIN_BODY="$(python3 -I -c 'import json,sys; print(json.dumps({"username":"admin","password":sys.argv[1]}))' "$ADMIN_PASSWORD")"
LOGIN="$(curl -fsS -c "$JAR" -H "Origin: $ORIGIN" -H 'Content-Type: application/json' -d "$LOGIN_BODY" "$BASE/v1/auth/login")" \
  || fail "login failed"
CSRF="$(json_get "$LOGIN" csrf_token)"
ADMIN_PRINCIPAL="$(json_get "$LOGIN" principal_id)"
[ -n "$CSRF" ] || fail "login returned no CSRF token"

admin_post() { # path json
  curl -fsS -b "$JAR" -H "Origin: $ORIGIN" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' -d "$2" "$BASE$1"
}
admin_put() {
  curl -fsS -b "$JAR" -X PUT -H "Origin: $ORIGIN" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' -d "$2" "$BASE$1" -o /dev/null
}

log "Cookie mutation without CSRF token is rejected"
CODE="$(curl -s -o /dev/null -w '%{http_code}' -b "$JAR" -H "Origin: $ORIGIN" -H 'Content-Type: application/json' \
  -d '{"provider":"generic","full_name":"smoke/csrf"}' "$BASE/v1/repositories")"
[ "$CODE" = "403" ] || fail "expected 403 without X-CSRF-Token, got $CODE"

log "Create repository, service principal, grant and token"
REPO_NAME="smoke/app"
REPO="$(admin_post /v1/repositories "{\"provider\":\"generic\",\"full_name\":\"$REPO_NAME\"}")"
REPO_ID="$(json_get "$REPO" id)"
PRINCIPAL="$(admin_post /v1/admin/principals '{"kind":"service","display_name":"smoke agent","role":"member"}')"
PRINCIPAL_ID="$(json_get "$PRINCIPAL" id)"
admin_put /v1/admin/grants "{\"principal_id\":\"$PRINCIPAL_ID\",\"repository_id\":\"$REPO_ID\",\"permission\":\"write\"}"
TOKEN_RESP="$(admin_post /v1/admin/tokens "{\"principal_id\":\"$PRINCIPAL_ID\",\"name\":\"smoke token\",\"scopes\":[\"sessions:write\",\"context:read\",\"outcomes:write\"],\"expires_in_days\":1}")"
TOKEN="$(json_get "$TOKEN_RESP" token)"
[ -n "$TOKEN" ] || fail "no token issued"
: "$ADMIN_PRINCIPAL"

# ---------------------------------------------------------------------------
log "Ingest a session (Flow A)"
SIGNATURE="error[E0599]: no method named \`frobnicate_widget\` found for struct \`WidgetBox\`"
SESSION_BODY="$(python3 -I -c '
import json, sys
print(json.dumps({
  "agent_type": "claude_code",
  "external_id": "smoke-session-1",
  "repository": sys.argv[1],
  "branch": "fix/frobnicate",
  "messages": [
    {"role": "user", "content": "cargo build fails with " + sys.argv[2]},
    {"role": "assistant", "content": "The root cause is that frobnicate_widget lives in the WidgetExt trait, which was not imported. Add `use widgets::WidgetExt;` to the module. cargo test now passes."}
  ],
  "hints": {
    "title": "Missing WidgetExt import hides frobnicate_widget",
    "symptom": "cargo build fails: " + sys.argv[2],
    "root_cause": "frobnicate_widget is provided by the WidgetExt trait, which is not in scope.",
    "remedy": "Add `use widgets::WidgetExt;` to the module that calls frobnicate_widget.",
    "error_signatures": [sys.argv[2]],
    "files": ["src/widgets/mod.rs"]
  }
}))' "$REPO_NAME" "$SIGNATURE")"
IDEM="smoke-$(openssl rand -hex 8)"
ingest() {
  curl -sS -w '\n%{http_code}' -H "Authorization: Bearer $TOKEN" -H "Idempotency-Key: $1" \
    -H 'Content-Type: application/json' -d "$2" "$BASE/v1/sessions"
}
OUT="$(ingest "$IDEM" "$SESSION_BODY")"
CODE="${OUT##*$'\n'}"; BODY="${OUT%$'\n'*}"
[ "$CODE" = "202" ] || fail "ingest expected 202, got $CODE: $BODY"
SESSION_ID="$(json_get "$BODY" id)"

log "Identical retry returns the same ingestion; conflicting reuse fails"
OUT="$(ingest "$IDEM" "$SESSION_BODY")"
CODE="${OUT##*$'\n'}"; BODY="${OUT%$'\n'*}"
[ "$CODE" = "202" ] && [ "$(json_get "$BODY" id)" = "$SESSION_ID" ] && [ "$(json_get "$BODY" duplicate)" = "true" ] \
  || fail "retry did not replay the original ingestion: $CODE $BODY"
OTHER_BODY="${SESSION_BODY/smoke-session-1/smoke-session-2}"
OUT="$(ingest "$IDEM" "$OTHER_BODY")"
CODE="${OUT##*$'\n'}"
[ "$CODE" = "409" ] || fail "conflicting idempotency key expected 409, got $CODE"

log "Wait for extraction"
STATUS=""
for _ in $(seq 1 60); do
  S="$(curl -fsS -H "Authorization: Bearer $TOKEN" "$BASE/v1/sessions/$SESSION_ID")"
  STATUS="$(json_get "$S" extraction_status)"
  case "$STATUS" in succeeded|skipped|failed) break ;; esac
  sleep 1
done
[ "$STATUS" = "succeeded" ] || fail "extraction ended as '$STATUS' (expected succeeded): $S"
[ "$(json_get "$S" capture_mode)" = "structured_only" ] || fail "default capture mode should be structured_only"
[ "$(json_get "$S" raw_payload_available)" = "false" ] || fail "structured-only transcript should be discarded after extraction"

# ---------------------------------------------------------------------------
log "Search (Flow B): the exact error finds the solution"
SEARCH_BODY="$(python3 -I -c '
import json, sys
print(json.dumps({"task": "fix the failing build", "repository": sys.argv[1], "error": sys.argv[2]}))' "$REPO_NAME" "$SIGNATURE")"
RESULT="$(curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d "$SEARCH_BODY" "$BASE/v1/context/search")"
COUNT="$(python3 -I -c 'import json,sys; print(len(json.loads(sys.argv[1])["suggestions"]))' "$RESULT")"
[ "$COUNT" -ge 1 ] || fail "search returned no suggestion: $RESULT"
VSTATE="$(json_get "$RESULT" suggestions.0.verification.state)"
[ "$VSTATE" = "candidate" ] || fail "an agent-only claim must stay 'candidate', got '$VSTATE'"
echo "suggestion: $(json_get "$RESULT" suggestions.0.title) [$VSTATE]"

# ---------------------------------------------------------------------------
log "MCP through the reverse proxy: initialize, list tools, find_relevant_context"
mcp() {
  curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' -H 'MCP-Protocol-Version: 2025-06-18' -d "$1" "$BASE/mcp"
}
mcp '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' >/dev/null \
  || fail "MCP initialize failed through the proxy"
TOOLS="$(mcp '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}')"
python3 -I -c 'import json,sys; names=sorted(t["name"] for t in json.loads(sys.argv[1])["result"]["tools"]); assert names==["find_relevant_context","get_solution","report_outcome"], names' "$TOOLS" \
  || fail "unexpected MCP tools: $TOOLS"
CALL="$(python3 -I -c '
import json, sys
print(json.dumps({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"find_relevant_context","arguments":{"task":"fix the failing build","repository":sys.argv[1],"error":sys.argv[2]}}}))' "$REPO_NAME" "$SIGNATURE")"
MCP_RESULT="$(mcp "$CALL")"
python3 -I -c 'import json,sys; r=json.loads(sys.argv[1])["result"]; assert len(r["structuredContent"]["suggestions"])>=1, r' "$MCP_RESULT" \
  || fail "MCP find_relevant_context returned no suggestion: $MCP_RESULT"
echo "MCP find_relevant_context returned a suggestion"

# ---------------------------------------------------------------------------
log "Process inspection (A16): only Rust binaries, PostgreSQL and Caddy"
SERVICES="$("${COMPOSE[@]}" ps --services --status running | sort | tr '\n' ' ')"
[ "$SERVICES" = "api postgres web worker " ] || fail "expected exactly api postgres web worker running, got: $SERVICES"
check_proc() { # service pattern
  local top
  top="$("${COMPOSE[@]}" top "$1")"
  echo "$top" | grep -Eq "$2" || fail "$1: expected a process matching '$2':
$top"
  if echo "$top" | grep -Eiq '(^|[ /])(node|nodejs|npm|pnpm|yarn|deno|bun|nginx|redis-server)( |$)'; then
    fail "$1: unexpected non-Rust runtime process:
$top"
  fi
}
check_proc api    'antibody-api'
check_proc worker 'antibody-worker'
check_proc web    'caddy'
check_proc postgres 'postgres'
"${COMPOSE[@]}" top api worker web postgres

log "PostgreSQL is not published to the host"
PG_CONTAINER="$("${COMPOSE[@]}" ps -q postgres)"
if [ -n "$(docker port "$PG_CONTAINER" 2>/dev/null)" ]; then
  fail "postgres has published ports: $(docker port "$PG_CONTAINER")"
fi
