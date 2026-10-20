#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export function validateCommitMessage(message) {
  const credit = /^\s*(?:co-authored-by|authored-by|generated-by|assisted-by)\s*:\s*.*(?:\bclaude\b|\banthropic\b)/im;
  const byline = /(?:generated|powered|written|authored|assisted)\s+(?:with|by|using)\s+(?:\[[^\]\r\n]*\bclaude\b|claude\b|anthropic\b)/i;
  const session = /^\s*claude-session\s*:|https?:\/\/(?:www\.)?claude\.(?:ai|com)\/(?:code|chat)\//im;
  if (credit.test(message) || byline.test(message) || session.test(message)) {
    throw new Error(
      "Claude/Anthropic attribution is forbidden in this repository. Remove the tool credit or session link; preserve the human author and Signed-off-by trailer."
    );
  }
}

export function validateCommitHistory(revisions, cwd = process.cwd()) {
  const git = (args) => execFileSync("git", args, { cwd, encoding: "utf8" });
  const commits = git(["rev-list", ...revisions]).trim().split("\n").filter(Boolean);
  for (const commit of commits) {
    try {
      validateCommitMessage(git(["show", "-s", "--format=%B", commit]));
    } catch (error) {
      throw new Error(`${commit}: ${error.message}`);
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv[2] === "--history") {
      validateCommitHistory(process.argv.slice(3).length ? process.argv.slice(3) : ["HEAD"]);
    } else {
      validateCommitMessage(readFileSync(process.argv[2], "utf8"));
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
