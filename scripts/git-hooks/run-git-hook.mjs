#!/usr/bin/env node
// Git hook runner, ported from magnetic's tools/workspace/checks/run-git-hook.mjs.
//
//   pre-commit: validates every staged blob (secrets, size, line limit,
//               conflict markers, whitespace).
//   pre-push:   validates every outgoing blob the same way, then runs the
//               relevant CI checks against an exact worktree snapshot of
//               each pushed tip, so uncommitted local state cannot mask a
//               failure.
//
// Install with scripts/install-git-hooks.sh. Keep the hooks enabled;
// do not bypass the repository's attribution policy.

import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { validateCommitHistory } from "./check-commit-message.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

const maxLines = 500;
const maxBytes = 10 * 1024 * 1024;
const zeroOid = /^0+$/;
const secretName = /(^|\/)(\.env(?:\..+)?|terraform\.tfvars)$/;
const allowedExample = /\.env(?:\..+)?\.example$/;
const secretContent = [
  /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----\r?\n/,
  /AKIA[0-9A-Z]{16}/,
  /github_pat_[A-Za-z0-9_]{20,}/,
  /gh[pousr]_[A-Za-z0-9]{36,}/,
  /sk-ant-[A-Za-z0-9_-]{20,}/,
  /xox[baprs]-[A-Za-z0-9-]{10,}/
];
// Redaction code and its tests must contain synthetic secrets to exercise.
const secretFixtureFiles = new Set([
  "crates/antibody-core/src/redact.rs",
  "crates/antibody-worker/src/extract/tests.rs"
]);
const authoredSource =
  /^(?:apps|crates|integrations|scripts|tests|deploy)\/.*\.(?:css|rs|jsx?|mjs|py|scss|sh|sql|tsx?)$/;
// Fixtures in tests may hold fake passwords; hard secret patterns still apply.
const testSource = /(?:^|\/)(?:tests|e2e)\/|\.(?:test|spec)\.[cm]?[jt]sx?$/;
const generatedSource =
  /(?:^|\/)(?:__generated__|generated|migrations)(?:\/|$)|\.generated\.|^apps\/web\/src\/api\/schema\.d\.ts$/;

function git(args, options = {}) {
  return execFileSync("git", args, {
    encoding: options.encoding ?? "utf8",
    cwd: options.cwd,
    input: options.input,
    stdio: options.stdio,
    maxBuffer: 128 * 1024 * 1024
  });
}

export function parsePrePushInput(input) {
  return input
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const fields = line.split(" ");
      if (fields.length !== 4)
        throw new Error(`invalid pre-push update: ${line}`);
      const [localRef, localOid, remoteRef, remoteOid] = fields;
      if (
        !/^[0-9a-f]{40,64}$/.test(localOid) ||
        !/^[0-9a-f]{40,64}$/.test(remoteOid)
      ) {
        throw new Error(`invalid object ID in pre-push update: ${line}`);
      }
      return { localRef, localOid, remoteRef, remoteOid };
    });
}

function changedIndexEntries(cwd) {
  const fields = git(
    ["diff", "--cached", "--name-status", "-z", "--diff-filter=ACMR"],
    { cwd }
  ).split("\0");
  const entries = [];
  for (let index = 0; index < fields.length - 1;) {
    const status = fields[index++];
    if (status.startsWith("R")) index++;
    entries.push(fields[index++]);
  }
  return entries;
}

function indexBlob(cwd, file) {
  const record = git(["ls-files", "--stage", "-z", "--", file], { cwd }).split(
    "\0"
  )[0];
  const match = record.match(/^(\d+) ([0-9a-f]+) 0\t/);
  if (!match) throw new Error(`cannot resolve staged blob: ${file}`);
  return {
    mode: match[1],
    bytes: git(["cat-file", "blob", match[2]], { cwd, encoding: "buffer" })
  };
}

export function sourceLineCount(bytes) {
  const text = bytes.toString("utf8");
  return text.split("\n").length - (text.endsWith("\n") ? 1 : 0);
}

export function assertSafeBlob(file, { mode, bytes }) {
  if (mode === "160000") throw new Error(`submodules are not allowed: ${file}`);
  if (secretName.test(file) && !allowedExample.test(file)) {
    throw new Error(`secret-like filename is not allowed: ${file}`);
  }
  if (bytes.length > maxBytes)
    throw new Error(`staged file exceeds 10 MiB: ${file}`);
  if (mode === "120000") {
    const target = bytes.toString("utf8");
    if (path.isAbsolute(target) || target.split("/").includes(".."))
      throw new Error(`symlink escapes the source snapshot: ${file}`);
    return;
  }
  if (bytes.includes(0)) return;
  const text = bytes.toString("utf8");
  if (authoredSource.test(file) && !generatedSource.test(file)) {
    const lines = sourceLineCount(bytes);
    if (lines > maxLines)
      throw new Error(
        `authored source exceeds ${maxLines} lines (${lines}): ${file}`
      );
  }
  if (/^(?:<{7}|={7}|>{7})(?: |$)/m.test(text)) {
    throw new Error(`merge conflict marker found: ${file}`);
  }
  if (
    !secretFixtureFiles.has(file) &&
    secretContent.some((pattern) => pattern.test(text))
  ) {
    throw new Error(`secret-like content found: ${file}`);
  }
  const assignment = text.match(
    /(?:password|secret|api[_-]?key|access[_-]?token)\s*[:=]\s*(["'`])([^"'`\r\n]{12,})\1/i
  );
  if (
    assignment &&
    !testSource.test(file) &&
    !assignment[2].startsWith("$") &&
    !/^env\([A-Z0-9_]+\)$/i.test(assignment[2]) &&
    !(assignment[2].startsWith("{{") && assignment[2].endsWith("}}")) &&
    !(assignment[1] === "`" && assignment[2].includes("${")) &&
    !/(?:example|placeholder|changeme|replace|dummy|test|fixture|ci-only)/i.test(
      assignment[2]
    )
  ) {
    throw new Error(`secret-like assignment found: ${file}`);
  }
}

export function validateStagedIndex(cwd = process.cwd()) {
  for (const file of changedIndexEntries(cwd))
    assertSafeBlob(file, indexBlob(cwd, file));
  const whitespace = spawnSync("git", ["diff", "--cached", "--check"], {
    cwd,
    encoding: "utf8"
  });
  if (whitespace.status !== 0)
    throw new Error(whitespace.stdout || whitespace.stderr);
}

function outgoingCommits(cwd, update, remoteName) {
  if (zeroOid.test(update.localOid)) return [];
  const range = zeroOid.test(update.remoteOid)
    ? [update.localOid, "--not", `--remotes=${remoteName}`]
    : [update.localOid, `^${update.remoteOid}`];
  return git(["rev-list", ...range], { cwd })
    .trim()
    .split("\n")
    .filter(Boolean);
}

export function validateOutgoingObjects({
  cwd = process.cwd(),
  remoteName,
  input
}) {
  const updates = parsePrePushInput(input);
  if (
    updates.some(
      (update) =>
        zeroOid.test(update.localOid) && update.remoteRef === "refs/heads/main"
    )
  ) {
    throw new Error("deleting main is not allowed");
  }
  const commits = new Set(
    updates.flatMap((update) => outgoingCommits(cwd, update, remoteName))
  );
  if (commits.size) validateCommitHistory([...commits, "--no-walk"], cwd);
  const blobs = new Map();
  const paths = new Set();
  for (const commit of commits) {
    const fields = git(
      [
        "diff-tree",
        "--no-commit-id",
        "--no-renames",
        "--root",
        "-r",
        "-z",
        "--raw",
        commit
      ],
      { cwd }
    ).split("\0");
    for (let index = 0; index < fields.length - 1;) {
      const header = fields[index++];
      const file = fields[index++];
      const match = header.match(/^:\d+ (\d+) [0-9a-f]+ ([0-9a-f]+) ([A-Z])$/);
      if (!match) throw new Error(`cannot parse outgoing commit ${commit}`);
      paths.add(file);
      if (match[3] !== "D")
        blobs.set(`${match[2]}:${file}`, {
          oid: match[2],
          mode: match[1],
          file,
          commit
        });
    }
  }
  for (const entry of blobs.values()) {
    assertSafeBlob(
      entry.file,
      {
        mode: entry.mode,
        bytes: git(["cat-file", "blob", entry.oid], {
          cwd,
          encoding: "buffer"
        })
      }
    );
  }
  return {
    updates,
    commits: [...commits],
    blobs: blobs.size,
    paths: [...paths]
  };
}

// Mirrors .github/workflows/ci.yml, minus the Postgres-backed test suites and
// the Docker smoke test, which stay in CI.
export function selectValidationTargets(paths) {
  const rust = paths.some((file) =>
    /^(?:crates\/|Cargo\.(?:toml|lock)$|rust-toolchain\.toml$)/.test(file)
  );
  const contract = paths.some((file) =>
    /^(?:crates\/|openapi\/|Cargo\.lock$)/.test(file)
  );
  const web = paths.some((file) =>
    /^(?:apps\/web\/|openapi\/openapi\.json$)/.test(file)
  );
  const targets = [];
  if (rust) {
    targets.push(["cargo", ["fmt", "--all", "--check"]]);
    targets.push([
      "cargo",
      ["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]
    ]);
  }
  if (contract) targets.push(["contract-drift"]);
  if (web) {
    targets.push(["pnpm", ["install", "--frozen-lockfile", "--ignore-scripts"], "apps/web"]);
    targets.push(["pnpm", ["run", "check:api"], "apps/web"]);
    targets.push(["pnpm", ["exec", "tsc", "--noEmit"], "apps/web"]);
    targets.push(["pnpm", ["test"], "apps/web"]);
  }
  return targets;
}

function run(command, args, cwd, env) {
  const result = spawnSync(command, args, { cwd, env, stdio: "inherit" });
  if (result.status !== 0)
    throw new Error(`failed: ${command} ${args.join(" ")}`);
}

function checkContractDrift(snapshot, env) {
  run("cargo", ["build", "--locked", "-p", "antibody-api"], snapshot, env);
  const binary = path.join(env.CARGO_TARGET_DIR, "debug", "antibody-api");
  for (const [command, file] of [
    ["openapi", "openapi/openapi.json"],
    ["mcp-schema", "openapi/mcp-tools.json"]
  ]) {
    const generated = execFileSync(binary, [command], {
      cwd: snapshot,
      env,
      encoding: "utf8",
      maxBuffer: 128 * 1024 * 1024
    });
    if (generated !== readFileSync(path.join(snapshot, file), "utf8"))
      throw new Error(
        `${file} is stale; regenerate with: antibody-api ${command} > ${file}`
      );
  }
}

function validateExactSnapshot(cwd, oid, targets) {
  const snapshot = mkdtempSync(path.join(os.tmpdir(), "antibody-pre-push-"));
  try {
    git(["worktree", "add", "--detach", snapshot, oid], { cwd });
    const env = {
      ...process.env,
      CI: "1",
      // Reuse the main checkout's dependency builds instead of starting cold.
      CARGO_TARGET_DIR:
        process.env.CARGO_TARGET_DIR ?? path.join(cwd, "target", "pre-push")
    };
    for (const [command, args, dir] of targets) {
      const where = path.join(snapshot, dir ?? "");
      console.error(`[pre-push ${oid.slice(0, 8)}] ${[command, ...(args ?? [])].join(" ")}`);
      if (command === "contract-drift") checkContractDrift(snapshot, env);
      else run(command, args, where, env);
    }
  } finally {
    try {
      git(["worktree", "remove", "--force", snapshot], { cwd });
    } finally {
      rmSync(snapshot, { recursive: true, force: true });
    }
  }
}

function main() {
  const mode = process.argv[2];
  if (mode === "pre-commit") return validateStagedIndex();
  if (mode !== "pre-push")
    throw new Error("usage: run-git-hook.mjs pre-commit|pre-push");
  const input = readFileSync(0, "utf8");
  const result = validateOutgoingObjects({
    remoteName: process.argv[3],
    input
  });
  const tips = [
    ...new Set(
      result.updates
        .map((update) => update.localOid)
        .filter((oid) => !zeroOid.test(oid))
    )
  ];
  const targets = selectValidationTargets(result.paths);
  if (targets.length === 0) return;
  for (const tip of tips) validateExactSnapshot(process.cwd(), tip, targets);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
