// Cuts a release of what is on main: `pnpm release`.
// Reads the app version from origin/main, tags that commit `v<version>`, pushes
// the tag, waits for the release workflow, and checks the published assets.
// Bump the version in a PR first; this script never edits files.

import { execFileSync, spawnSync } from "node:child_process";
import process from "node:process";
import { setTimeout as sleep } from "node:timers/promises";
import { checkReleaseTag, workspaceVersion } from "./release-tag.ts";

/** The asset names a release must carry for friends to install and copies to update. */
export function missingAssets(names: string[]): string[] {
  const missing: string[] = [];
  if (!names.some((n) => n.endsWith("-setup.exe")))
    missing.push("the NSIS installer (*-setup.exe)");
  if (!names.some((n) => n.endsWith("-setup.exe.sig")))
    missing.push("its signature (*-setup.exe.sig)");
  if (!names.includes("latest.json")) missing.push("the updater file (latest.json)");
  return missing;
}

function fail(message: string): never {
  console.error(`✗ ${message}`);
  process.exit(1);
}

function run(command: string, ...args: string[]): void {
  console.log(`$ ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function read(command: string, ...args: string[]): string {
  return execFileSync(command, args, { encoding: "utf8" }).trim();
}

if (import.meta.main) {
  run("git", "fetch", "--tags", "origin", "main");
  const commit = read("git", "rev-parse", "origin/main");
  const show = (path: string) => read("git", "show", `${commit}:${path}`);
  const files = {
    cargoToml: show("Cargo.toml"),
    tauriConf: show("src-tauri/tauri.conf.json"),
    packageJson: show("package.json"),
  };
  const version = workspaceVersion(files.cargoToml);
  if (!version) fail("origin/main's Cargo.toml has no [workspace.package] version");
  const tag = `v${version}`;
  const check = checkReleaseTag(tag, files);
  if (!check.ok) fail(check.error);
  if (read("git", "tag", "--list", tag)) {
    fail(`${tag} already exists; bump the version in Cargo.toml in a PR first`);
  }

  run("git", "tag", tag, commit);
  run("git", "push", "origin", `refs/tags/${tag}`);

  let runId = "";
  for (let attempt = 0; attempt < 30 && !runId; attempt++) {
    runId = read(
      "gh",
      "run",
      "list",
      "--workflow",
      "release.yml",
      "--branch",
      tag,
      "--json",
      "databaseId",
      "--jq",
      ".[0].databaseId // empty",
    );
    if (!runId) await sleep(5000);
  }
  if (!runId) fail(`no release workflow run appeared for ${tag}`);
  run("gh", "run", "watch", runId, "--exit-status");

  const assets = read("gh", "release", "view", tag, "--json", "assets", "--jq", ".assets[].name")
    .split("\n")
    .filter(Boolean);
  const missing = missingAssets(assets);
  if (missing.length) fail(`release ${tag} is missing ${missing.join(", ")}`);
  console.log(`✓ released ${tag}: ${assets.join(", ")}`);
}
