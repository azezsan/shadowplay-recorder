# ShadowPlay Recorder

A Windows tray app that records each Deadlock match with NVIDIA ShadowPlay, names the recording after the match id, and keeps the recordings folder under a size cap. It replaces a Python script that needed a terminal and hand-edited config files to set up.

## Brief

- **Problem**: the script needed `uv`, an admin hand-edit of the NVIDIA overlay config, a hand-edited `config.toml` and a startup shortcut. A friend could not install it.
- **Users**: the owner and a few friends.
- **Surface**: one Windows desktop app: a tray icon and one window with a setup checklist, live status, settings and activity.
- **Data**: a JSON settings file and a rotating log file in the app's data folders. Nothing sensitive, no database.
- **Integrations**: the NVIDIA overlay's debug port (`docs/apis/nvidia-overlay.md`), Deadlock's console log, the Steam install, and GitHub Releases.
- **Load**: a background loop that polls a process, a log file and a folder once a second.
- **Language**: English, left to right.
- **Tier**: tool.

## Stack

- **Shell**: Tauri 2, Windows only, NSIS installer installed per user.
- **Backend**: Rust. All logic lives in `recorder-core`, which has no Tauri dependency.
- **UI**: Svelte 5 in runes mode, Vite, Tailwind 4, shadcn-svelte.
- **Boundary**: tauri-specta generates `src/lib/bindings.ts` from the Rust commands and events. The UI imports from it and never calls `invoke` by name.
- **Storage**: one JSON settings file. No database.
- **Quality**: rustfmt, Clippy with warnings as errors, cargo-deny, Rust tests, Prettier, ESLint, svelte-check, `pnpm audit`, gitleaks.
- **CI**: GitHub Actions runs `pnpm check` on a Windows runner for every PR and for `main`.

Nothing is hosted. The repo is `azezsan/shadowplay-recorder` on GitHub, owned by the `azezsan` account.

## Commands

- `pnpm check` verifies a change. It runs every generator, formatter, linter and test, and is what CI runs.
- `pnpm dev` runs the app with live reload.
- `pnpm format` formats everything.
- `pnpm merge <pr>` merges a PR once its checks pass and waits for `main` to go green.
- `pnpm release` tags `main` with its app version, waits for the release build, and checks the published assets.

## Rules that hold everywhere

- The app only works on Windows with an NVIDIA GPU, and agents work on macOS. Code that calls Windows goes in `recorder-core`'s `platform` module and nowhere else. Everything else must build and be tested on macOS.
- On macOS `pnpm check` also lints `recorder-core` for the Windows target. The shell crate cannot be cross-checked, so keep Windows-only code out of it.
- The shell crate has no tests, because its test binary cannot load on Windows. Put logic in `recorder-core`.
- Behaviour is tested through the engine step with fakes, not through the UI. Recording a real match is verified by the owner by hand.
- After changing a command, an event or a type that crosses to the UI, run `pnpm check` and commit the regenerated `src/lib/bindings.ts`.
- Timings, NVIDIA command names and the recordable-map list are constants, not settings.
- Use the words in `GLOSSARY.md`.

## Layout

One line per area. Details live in each module's own doc comment.

- `crates/core/` is `recorder-core`: match detection, the match session, the overlay client, recordings, setup checks. Start at `crates/core/src/lib.rs`.
- `src-tauri/` is the desktop shell: the engine thread, commands and events. Start at `src-tauri/src/lib.rs`.
- `src/` is the Svelte UI. `src/lib/components/` has one component per section of the window, and `src/lib/components/ui/` is shadcn-svelte's generated code.
- `scripts/` holds the multi-step pnpm scripts, in TypeScript.
- `docs/apis/` records what the project relies on from outside systems.

## Releasing

Pushing a tag `vX.Y.Z` runs `.github/workflows/release.yml`: it checks the tag against the app version, builds the NSIS installer, signs the update, and publishes a GitHub Release with the installer, its `.sig` and `latest.json`. Installed copies read `latest.json` from the latest release (`plugins.updater` in `src-tauri/tauri.conf.json`).

The app version lives only in `version` under `[workspace.package]` in the root `Cargo.toml`. Tauri reads it from there; `scripts/release-tag.ts` fails the release if `tauri.conf.json` or `package.json` sets one.

A local `pnpm tauri build` needs `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` set, because the bundle signs its update.

### Cutting a release

An agent cuts a release when the owner asks for one.

1. On a branch, bump `version` in the root `Cargo.toml`, run `cargo check` to refresh `Cargo.lock`, and merge the PR with `pnpm merge <pr>`. Skip this for a version that has never been tagged.
2. Run `pnpm release`. It tags the tip of `origin/main` as `v<version>`, pushes the tag, watches `.github/workflows/release.yml`, and fails unless the release has the installer, its `.sig` and `latest.json`.

The script refuses a version that is already tagged. The workflow refuses a tag that differs from the version, a commit that is not on `main`, and the placeholder updater key.

If the build fails before anything is published, fix it in a PR, then move the tag with `git push origin :refs/tags/v<version> && git tag -d v<version>` and run `pnpm release` again. Once a release is published, never move its tag: fix forward as below.

### Undoing a bad release

Fix forward: merge the fix, bump the patch version, and tag it. The updater only moves to a newer version, so the new tag is what replaces the bad one on installed copies. A published tag stays where it is: copies may already have installed it.

To stop a bad release spreading before the fix is ready, mark it a pre-release. `latest.json` then comes from the previous release, which is older than the bad one, so nothing installs it:

```sh
gh release edit v0.2.0 --prerelease
```

### One-time setup

Already done; kept for a new repo or a lost key. The steps run in this order, because branch protection needs the repo public on a free plan.

1. Scan the full history for secrets: `gitleaks git . --no-banner --redact`. It must report no leaks.
2. Make the repo public: `gh repo edit azezsan/shadowplay-recorder --visibility public --accept-visibility-change-consequences`.
3. Protect `main`, requiring the CI job `check` on an up-to-date branch:
   ```sh
   gh api -X PUT repos/azezsan/shadowplay-recorder/branches/main/protection --input - <<'EOF'
   {"required_status_checks": {"strict": true, "contexts": ["check"]},
    "enforce_admins": false, "required_pull_request_reviews": null, "restrictions": null}
   EOF
   ```
4. Generate the update-signing key outside the repo, store the private key and its password in the macOS Keychain and in GitHub Actions secrets, put the public key in `plugins.updater.pubkey`, then delete the files. Losing the private key means installed copies can never update again; the Keychain copy is the backup.
   ```sh
   dir=$(mktemp -d) && pass=$(openssl rand -base64 32)
   pnpm tauri signer generate --ci -p "$pass" -w "$dir/updater.key"
   security add-generic-password -U -a azezsan -s shadowplay-recorder-updater-private-key -w "$(cat "$dir/updater.key")"
   security add-generic-password -U -a azezsan -s shadowplay-recorder-updater-key-password -w "$pass"
   gh secret set TAURI_SIGNING_PRIVATE_KEY --repo azezsan/shadowplay-recorder < "$dir/updater.key"
   printf %s "$pass" | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo azezsan/shadowplay-recorder
   cat "$dir/updater.key.pub"   # goes into plugins.updater.pubkey, via a PR
   rm -rf "$dir"; unset pass
   ```
   This was done on 2026-11-18. To read the key and password back: `security find-generic-password -a azezsan -s shadowplay-recorder-updater-private-key -w` and `... -s shadowplay-recorder-updater-key-password -w`.

## Git flow

When a task's changes are done, follow the `ship` skill (branch, checks, PR, when to merge). Here the check is `pnpm check` and the merge is `pnpm merge <pr>`. Without that skill: open a PR from the task's own branch, then stop and merge only when the user says so.

## Agent skills

### Issue tracker

Issues and specs live in GitHub Issues on `azezsan/shadowplay-recorder`. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: one `GLOSSARY.md` at the repo root and ADRs in `docs/adr/`. See `docs/agents/domain.md`.
