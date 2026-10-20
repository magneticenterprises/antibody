# Claude Code Integration

Antibody connects to Claude Code through hooks (context lookup at prompt submission and transcript upload at session end) and an MCP server (on-demand search and outcome reporting).

Verified with Claude Code `2.1.302`. Features not confirmed in live runs are marked **unverified** or **best effort**.

## Setup

1. Create an API token in Antibody and export it in your shell:

   ```sh
   export ANTIBODY_TOKEN=abt_...
   export ANTIBODY_URL=https://antibody.example.com
   ```

2. Install the Antibody CLI for command hooks and check your connection:

   ```sh
   cargo install --path crates/antibody-cli --locked
   antibody doctor
   ```

3. Copy `integrations/claude-code/settings.json` to `.claude/settings.json` (project) or `~/.claude/settings.json` (user). Replace the placeholder URL and the `X-Antibody-Repository` header value.
4. (Optional) Enable the MCP server by copying `integrations/claude-code/mcp.json` to `.mcp.json`, or run:

   ```sh
   claude mcp add --transport http antibody https://antibody.example.com/mcp --header "Authorization: Bearer ${ANTIBODY_TOKEN}"
   ```

## Components

| Component | Mechanism | Verification Status |
| --- | --- | --- |
| Prompt context (HTTP hook) | `UserPromptSubmit` `http` hook sends JSON to `/v1/hooks/user-prompt-submit` with `X-Antibody-Repository`. | Verified hook type and output contract |
| Prompt context (Command hook) | `UserPromptSubmit` `command` hook runs `antibody hook user-prompt --agent claude-code` (2.5 s timeout, fails open). | Verified output contract |
| Session upload | `SessionEnd` spools the hook input privately to `~/.antibody/spool` and detaches a child that reads the transcript, collects Git metadata, and uploads it. | CLI handoff regression tested; hook `timeout` set to 10 s |
| MCP server | Exposes `find_relevant_context`, `get_solution`, and `report_outcome`. | Verified configuration |
| `CLAUDE.md.snippet` | Instructs the agent to call MCP tools. | **Best effort** (the model may ignore instructions) |

Use either the `http` hook (`settings.json`) or the `command` hook (`settings.command.json`) for `UserPromptSubmit`, not both.

## Notes

- Environment variable interpolation in the `http` hook `url` is **unverified**. Use a literal URL in `settings.json`. Header values can use `$ANTIBODY_TOKEN` when listed in `allowedEnvVars`.
- The command hook detects the repository name from `git remote get-url origin` or `ANTIBODY_REPOSITORY`. Set `ANTIBODY_COMPONENT` for monorepo subdirectories.
- Both prompt hook variants use `/v1/hooks/user-prompt-submit`, record retrievals as `hook`, and search only once per principal and agent session. A repeated prompt in the same session produces no additional context.
- If the Git working tree has uncommitted changes, the CLI sets `worktree_dirty: true` so the current commit is not treated as the fix revision.
- Session-end preparation runs after handoff; leave the transcript file available until the upload completes. Git metadata describes the working tree observed by that background process.
- Upload results are logged to `~/.antibody/upload.log` (without transcript text).

## Verify Prompt Hook Execution

Run a prompt that mentions a known error:

```sh
claude -p "cargo test fails with: pool timed out while waiting for an open connection" \
  --settings integrations/claude-code/settings.json
curl -s -H "Authorization: Bearer $ANTIBODY_TOKEN" "$ANTIBODY_URL/v1/activity/retrievals" | head
```

A new retrieval record with `source: "hook"` confirms that the hook executed. You can also run the automated verification script:

```sh
tests/agents/real_agent_hooks.sh claude
```

## Troubleshooting

- Run `antibody doctor` to verify environment variables, `/v1/health/ready`, and your API token.
- Run `claude --debug` to inspect hook execution and HTTP status codes.
- The command hook skips prompts shorter than 12 characters.
- If uploads fail, check `~/.antibody/upload.log`.
