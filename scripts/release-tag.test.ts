import assert from "node:assert/strict";
import { test } from "node:test";
import { checkReleaseTag } from "./release-tag.ts";

const cargoToml = `[workspace]
resolver = "3"
members = ["crates/core", "src-tauri"]

[workspace.package]
version = "0.1.0"
edition = "2024"
`;

const files = { cargoToml, tauriConf: "{}", packageJson: "{}" };

test("a tag naming the workspace version passes", () => {
  assert.deepEqual(checkReleaseTag("v0.1.0", files), { ok: true, version: "0.1.0" });
});

test("a tag naming another version fails and names both", () => {
  const check = checkReleaseTag("v0.2.0", files);
  assert.equal(check.ok, false);
  assert.match(!check.ok ? check.error : "", /v0\.2\.0.*0\.1\.0/);
});

test("a version in tauri.conf.json is a second source and fails", () => {
  const check = checkReleaseTag("v0.1.0", { ...files, tauriConf: '{ "version": "0.1.0" }' });
  assert.equal(check.ok, false);
  assert.match(!check.ok ? check.error : "", /tauri\.conf\.json/);
});

test("a version in package.json is a second source and fails", () => {
  const check = checkReleaseTag("v0.1.0", { ...files, packageJson: '{ "version": "0.1.0" }' });
  assert.equal(check.ok, false);
  assert.match(!check.ok ? check.error : "", /package\.json/);
});

test("the version is found after array values in the same table", () => {
  const withArray = cargoToml.replace(
    "[workspace.package]\n",
    '[workspace.package]\nauthors = ["azez"]\n',
  );
  assert.deepEqual(checkReleaseTag("v0.1.0", { ...files, cargoToml: withArray }), {
    ok: true,
    version: "0.1.0",
  });
});

test("Cargo.toml without a workspace version fails", () => {
  const check = checkReleaseTag("v0.1.0", { ...files, cargoToml: "[workspace]\n" });
  assert.equal(check.ok, false);
  assert.match(!check.ok ? check.error : "", /workspace\.package/);
});
