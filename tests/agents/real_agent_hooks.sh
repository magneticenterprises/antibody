#!/usr/bin/env bash
# A06: prove that real agent task-start hooks invoke Antibody retrieval.
#
# Starts an isolated API on 127.0.0.1:18090 with its own database, seeds a
# known fix, then runs each requested agent headless with a task-start hook
# and asserts that the server recorded a `hook` retrieval with a result for
# that agent run. Spends a small amount of model usage per agent.
#
# Usage: tests/agents/real_agent_hooks.sh [claude] [codex] [cli]
# `cli` exercises both command-hook contracts without running an agent or model.
# Needs: a running Postgres+pgvector (deploy/compose.dev.yaml), cargo, psql,
#        python3, and logged-in `claude` / `codex` CLIs.
#
# Codex only runs hooks you have trusted. Trust the hook once interactively
# (Codex prompts for new hooks), or pass CODEX_BYPASS_HOOK_TRUST=1 to use
# `--dangerously-bypass-hook-trust` for this throwaway run.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PGURL_ADMIN="${PGURL_ADMIN:-postgres://antibody:antibody@localhost:55432/postgres}"
DB="antibody_agent_hooks"
DBURL="${PGURL_ADMIN%/*}/$DB"
PORT=18090
BASE="http://127.0.0.1:$PORT"
WORK="$(mktemp -d)"
AGENTS=("${@:-claude}")
API_PID=""

cleanup() {
  [[ -n "$API_PID" ]] && kill "$API_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

cd "$ROOT"
cargo build -q -p antibody-api -p antibody-worker -p antibody-cli
psql "$PGURL_ADMIN" -qc "DROP DATABASE IF EXISTS $DB" -c "CREATE DATABASE $DB" >/dev/null

export DATABASE_URL="$DBURL" AGENT_TEST_PW="agent-hooks-password-123"
export ANTIBODY_LLM_PROVIDER=none ANTIBODY_EMBEDDING_PROVIDER=none
./target/debug/antibody-api bootstrap --org-slug acme --org-name Acme --admin-username agenthooks \
  --admin-password-env AGENT_TEST_PW > "$WORK/boot.json" 2>/dev/null
TOKEN="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['admin_token'])" "$WORK/boot.json")"

ANTIBODY_SECRET_KEY="agent-hooks-secret-key-0123456789abcdef" ANTIBODY_PUBLIC_URL="$BASE" \
  ANTIBODY_BIND="127.0.0.1:$PORT" ANTIBODY_LOG_FORMAT=text ./target/debug/antibody-api serve > "$WORK/api.log" 2>&1 &
API_PID=$!
for _ in $(seq 1 60); do curl -sf "$BASE/v1/health/ready" >/dev/null && break; sleep 0.5; done

curl -sf -o /dev/null -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"provider":"github","full_name":"acme/api"}' "$BASE/v1/repositories"
curl -sf -o /dev/null -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -H 'Idempotency-Key: agent-hooks-seed-1' \
  -d '{"agent_type":"claude_code","repository":"acme/api","messages":[
        {"role":"user","content":"cargo build fails in the user handler after the sqlx upgrade"},
        {"role":"tool","content":"error[E0308]: mismatched types\n  --> src/handlers/user.rs:42:17"},
        {"role":"assistant","content":"Root cause: sqlx 0.8 infers nullable columns as Option. Cast the column as non-null (id AS \"id!\") in src/handlers/user.rs."}]}' \
  "$BASE/v1/sessions"
./target/debug/antibody-worker --once >/dev/null 2>&1
SOLUTION="$(psql "$DBURL" -Atc "select id from solutions limit 1")"
[[ -n "$SOLUTION" ]] || { echo "seeding failed"; exit 1; }

PROMPT="Our build fails with error[E0308]: mismatched types in src/handlers/user.rs after upgrading sqlx. Do not use any tools. If you were given prior-solution context, reply with the solution_id it mentions; otherwise reply NO_CONTEXT."
mkdir -p "$WORK/repo"
fail=0

hook_count() { psql "$DBURL" -Atc "select count(*) from retrievals where channel = 'hook' and result_count > 0"; }

for agent in "${AGENTS[@]}"; do
  before="$(hook_count)"
  case "$agent" in
    cli)
      for command_agent in claude-code codex; do
        input="$(python3 -I -c 'import json,sys; print(json.dumps({"session_id":sys.argv[1], "hook_event_name":"UserPromptSubmit", "prompt":sys.argv[2]}))' "fixture-$command_agent-$$" "$PROMPT")"
        run_command_hook() {
          printf '%s' "$input" | ANTIBODY_TOKEN="$TOKEN" ANTIBODY_URL="$BASE" ANTIBODY_REPOSITORY=acme/api \
            ./target/debug/antibody hook user-prompt --agent "$command_agent"
        }
        answer="$(run_command_hook)"
        repeat="$(run_command_hook)"
        [[ "$answer" == *"$SOLUTION"* && -z "$repeat" ]] || {
          echo "FAIL [$command_agent] expected context only on the first prompt"; fail=1;
        }
      done
      after="$(hook_count)"
      [[ "$((after - before))" == 2 ]] || {
        echo "FAIL [cli] expected exactly one hook retrieval per agent session"; fail=1;
      }
      ;;
    claude)
      cat > "$WORK/claude-settings.json" <<EOF
{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"http","url":"$BASE/v1/hooks/user-prompt-submit","timeout":5,
  "headers":{"Authorization":"Bearer \$ANTIBODY_TOKEN","X-Antibody-Repository":"acme/api"},"allowedEnvVars":["ANTIBODY_TOKEN"]}]}]}}
EOF
      (cd "$WORK/repo" && ANTIBODY_TOKEN="$TOKEN" claude -p "$PROMPT" --settings "$WORK/claude-settings.json" \
        --model haiku --max-turns 1 --output-format json > "$WORK/claude.json" 2>&1) || true
      answer="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('result',''))" "$WORK/claude.json" 2>/dev/null || cat "$WORK/claude.json")"
      ;;
    codex)
      bypass=()
      [[ "${CODEX_BYPASS_HOOK_TRUST:-0}" == "1" ]] && bypass=(--dangerously-bypass-hook-trust)
      answer="$(cd "$WORK/repo" && ANTIBODY_TOKEN="$TOKEN" ANTIBODY_URL="$BASE" ANTIBODY_REPOSITORY=acme/api ANTIBODY_HOME="$WORK/home" \
        codex exec --ephemeral --skip-git-repo-check -s read-only "${bypass[@]}" -c 'features.hooks=true' \
        -c "hooks.UserPromptSubmit=[{hooks=[{type=\"command\",command=\"$ROOT/target/debug/antibody hook user-prompt --agent codex\",timeout=5}]}]" \
        "$PROMPT" 2>/dev/null | tail -3 || true)"
      ;;
    *) echo "unknown agent $agent"; exit 2 ;;
  esac
  after="$(hook_count)"
  if (( after > before )); then
    echo "PASS [$agent] hook invoked retrieval ($((after - before)) hook retrieval with results)"
  else
    echo "FAIL [$agent] no hook retrieval recorded"; fail=1
  fi
  if grep -q "$SOLUTION" <<<"$answer"; then
    if [[ "$agent" == cli ]]; then
      echo "PASS [cli] command hook output contains the solution_id"
    else
      echo "PASS [$agent] model answer cites the injected solution_id"
    fi
  else
    echo "INFO [$agent] model answer did not cite the solution_id: ${answer:0:200}"
  fi
done
exit "$fail"
