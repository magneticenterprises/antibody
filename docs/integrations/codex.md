# Codex CLI Integration

Verified with Codex CLI `0.163.0`. Items not confirmed in live runs are marked **unverified** or **best effort**.

## Setup

1. Export the Antibody URL and API token in the shell where you start `codex`:

   ```sh
   export ANTIBODY_URL=https://antibody.example.com
   export ANTIBODY_TOKEN=abt_...
   ```

2. Install the Antibody CLI and check your connection:

   ```sh
   cargo install --path crates/antibody-cli --locked
   antibody doctor
   ```

3. Merge `integrations/codex/config.toml` into `~/.codex/config.toml` or `<repo>/.codex/config.toml`. This enables `[features] hooks = true`, configures the `UserPromptSubmit` and `SessionEnd` hooks, and registers the MCP server.

## Components

| Component | Mechanism | Verification Status |
| --- | --- | --- |
| Prompt context | `antibody hook user-prompt --agent codex` calls `/v1/hooks/user-prompt-submit` and prints its `hookSpecificOutput.additionalContext` JSON. | CLI contract regression tested |
| Session upload | `antibody hook session-end --agent codex` privately spools the hook input and detaches before Git and transcript preparation. The configured hook timeout is 3 s. | CLI handoff regression tested with slow Git |
| MCP server | Configured under `[mcp_servers.antibody]`. | Verified configuration |
| `AGENTS.md.snippet` | Instructs the agent to call MCP tools. | **Best effort** |

The prompt hook sends `ANTIBODY_REPOSITORY` (or the Git origin) and
`ANTIBODY_COMPONENT` as repository/component headers. Retrievals are recorded as
`hook`, once per principal and agent session. Later prompts in that session
receive no additional context.

Session-end preparation reads the transcript and Git metadata in the background.
Keep the transcript file available until upload completes; metadata reflects the
working tree when the background process reads it.

## Hook Trust

Codex runs only hooks that you explicitly approve. When you add a new hook, start an interactive `codex` session once and approve the hook prompt. Codex saves your approval under `[hooks.state."..."]` in `~/.codex/config.toml`.

For automated testing only, `codex exec --dangerously-bypass-hook-trust` skips the trust prompt for a single run.

## Verify Hook Execution

1. Start `codex` and submit a prompt that mentions a known error.
2. Check the retrieval activity log:

   ```sh
   curl -s -H "Authorization: Bearer $ANTIBODY_TOKEN" "$ANTIBODY_URL/v1/activity/retrievals"
   ```

3. Check `~/.antibody/upload.log` to confirm session uploads.

## Troubleshooting

- Confirm that `[features] hooks = true` is set in `config.toml` and that `ANTIBODY_URL` and `ANTIBODY_TOKEN` are exported.
- Test the prompt hook manually:

  ```sh
  echo '{"session_id":"s","cwd":"'$PWD'","prompt":"cargo test fails with pool timed out"}' \
    | antibody hook user-prompt --agent codex
  ```

- Hook errors never block Codex. The CLI logs at most one line to `stderr` and exits with code `0`.
