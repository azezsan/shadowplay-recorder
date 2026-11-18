// Merges a pull request the safe way: `pnpm merge <pr>`.
// Waits for the PR's checks, squash-merges, then waits for CI on the merge
// commit on main and fails if it does not pass.

import { execFileSync, spawnSync } from "node:child_process";
import process from "node:process";
import { setTimeout as sleep } from "node:timers/promises";

const pr = process.argv[2];
if (!pr) {
  console.error("usage: pnpm merge <pr>");
  process.exit(1);
}

function run(...args: string[]): void {
  console.log(`$ gh ${args.join(" ")}`);
  const result = spawnSync("gh", args, { stdio: "inherit" });
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function read(...args: string[]): string {
  return execFileSync("gh", args, { encoding: "utf8" }).trim();
}

run("pr", "checks", pr, "--watch", "--fail-fast");
run("pr", "merge", pr, "--squash");

const commit = read("pr", "view", pr, "--json", "mergeCommit", "--jq", ".mergeCommit.oid");
console.log(`merged as ${commit}; waiting for CI on main`);

let runId = "";
for (let attempt = 0; attempt < 30 && !runId; attempt++) {
  runId = read(
    "run",
    "list",
    "--commit",
    commit,
    "--json",
    "databaseId",
    "--jq",
    ".[0].databaseId",
  );
  if (!runId) await sleep(5000);
}
if (!runId) {
  console.error(`no CI run appeared for ${commit}`);
  process.exit(1);
}
run("run", "watch", runId, "--exit-status");
console.log("✓ main is green");
