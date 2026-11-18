// Checks that a release tag names the app version: `node scripts/release-tag.ts v1.2.3`.
// The version lives only in `[workspace.package]` of the root Cargo.toml.
// Tauri reads it from there, so tauri.conf.json and package.json must not set one.

import { readFileSync } from "node:fs";
import process from "node:process";

export type ReleaseFiles = { cargoToml: string; tauriConf: string; packageJson: string };
export type TagCheck = { ok: true; version: string } | { ok: false; error: string };

export function checkReleaseTag(tag: string, files: ReleaseFiles): TagCheck {
  const version = workspaceVersion(files.cargoToml);
  if (!version) {
    return { ok: false, error: "Cargo.toml has no version in [workspace.package]" };
  }
  for (const [name, text] of [
    ["src-tauri/tauri.conf.json", files.tauriConf],
    ["package.json", files.packageJson],
  ] as const) {
    if ("version" in JSON.parse(text)) {
      return { ok: false, error: `${name} sets a version; only Cargo.toml may` };
    }
  }
  if (tag !== `v${version}`) {
    return { ok: false, error: `tag ${tag} does not match the app version ${version}` };
  }
  return { ok: true, version };
}

function workspaceVersion(cargoToml: string): string | undefined {
  let inTable = false;
  for (const line of cargoToml.split("\n")) {
    const header = /^\s*\[([^\]]+)\]\s*$/.exec(line);
    if (header) inTable = header[1]?.trim() === "workspace.package";
    else if (inTable) {
      const version = /^\s*version\s*=\s*"([^"]+)"/.exec(line)?.[1];
      if (version) return version;
    }
  }
  return undefined;
}

if (import.meta.main) {
  const tag = process.argv[2];
  if (!tag) {
    console.error("usage: node scripts/release-tag.ts <tag>");
    process.exit(1);
  }
  const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
  const check = checkReleaseTag(tag, {
    cargoToml: read("Cargo.toml"),
    tauriConf: read("src-tauri/tauri.conf.json"),
    packageJson: read("package.json"),
  });
  if (!check.ok) {
    console.error(`✗ ${check.error}`);
    process.exit(1);
  }
  console.log(`✓ ${tag} matches the app version ${check.version}`);
}
