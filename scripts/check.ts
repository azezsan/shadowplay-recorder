// Runs every check the project has, in order, and stops at the first failure.
// This is the one command that says whether a change is good: `pnpm check`.

import { spawnSync } from "node:child_process";
import process from "node:process";

const onWindows = process.platform === "win32";

const steps: [name: string, command: string][] = [
  // Generators first: the UI bundle is embedded by the Rust build, and a
  // debug build of the app regenerates the TypeScript bindings.
  ["build the UI", "pnpm ui:build"],
  ["generate the bindings", "cargo run -p shadowplay-recorder -- --export-bindings"],
  ["bindings are up to date", "git diff --exit-code -- src/lib/bindings.ts"],
  ["Rust tests", "cargo test --workspace"],
  ["Prettier", "pnpm exec prettier --check ."],
  ["ESLint", "pnpm exec eslint ."],
  ["svelte-check", "pnpm exec svelte-check --tsgo --tsconfig ./tsconfig.json"],
  ["rustfmt", "cargo fmt --all --check"],
  ["Clippy", "cargo clippy --workspace --all-targets -- -D warnings"],
  // Off Windows, also lint the core for Windows, where its platform layer is
  // the real one. The shell crate cannot be cross-checked this way.
  ...(onWindows
    ? []
    : ([
        [
          "Clippy for Windows",
          "cargo clippy -p recorder-core --all-targets --target x86_64-pc-windows-msvc -- -D warnings",
        ],
      ] satisfies [string, string][])),
  ["cargo-deny", "cargo deny check"],
  ["pnpm audit", "pnpm audit"],
  ["gitleaks", "gitleaks dir . --no-banner --redact"],
];

for (const [name, command] of steps) {
  console.log(`\n── ${name}: ${command}`);
  const result = spawnSync(command, { stdio: "inherit", shell: true });
  if (result.status !== 0) {
    console.error(`\n✗ ${name} failed`);
    process.exit(result.status ?? 1);
  }
}
console.log("\n✓ all checks passed");
