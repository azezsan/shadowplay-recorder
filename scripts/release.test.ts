import assert from "node:assert/strict";
import { test } from "node:test";
import { missingAssets } from "./release.ts";

test("a complete release is missing nothing", () => {
  assert.deepEqual(
    missingAssets([
      "ShadowPlay.Recorder_0.1.0_x64-setup.exe",
      "ShadowPlay.Recorder_0.1.0_x64-setup.exe.sig",
      "latest.json",
    ]),
    [],
  );
});

test("an empty release is missing all three assets", () => {
  assert.equal(missingAssets([]).length, 3);
});

test("an unsigned release is missing the signature only", () => {
  assert.deepEqual(missingAssets(["App_0.1.0_x64-setup.exe", "latest.json"]), [
    "its signature (*-setup.exe.sig)",
  ]);
});
