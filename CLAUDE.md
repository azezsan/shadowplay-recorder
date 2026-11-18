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

## Rules that hold everywhere

- The app only works on Windows with an NVIDIA GPU, and agents work on macOS. Code that calls Windows goes in `recorder-core`'s `platform` module and nowhere else. Everything else must build and be tested on macOS.
- On macOS `pnpm check` also lints `recorder-core` for the Windows target. The shell crate cannot be cross-checked, so keep Windows-only code out of it.
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

Not built yet. The release pipeline (a version tag builds the installer and publishes it to GitHub Releases) and self-update are tracked as issues.

## Git flow

Each task is done on its own branch in its own git worktree. When the task is finished, run `pnpm check`, commit, push, and open a PR with `gh pr create`. Then stop: the user reviews the PR and says when to merge. Merge only when they say so, with `pnpm merge <pr>`. Never push to `main` directly.

## Agent skills

### Issue tracker

Issues and specs live in GitHub Issues on `azezsan/shadowplay-recorder`. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: one `GLOSSARY.md` at the repo root and ADRs in `docs/adr/`. See `docs/agents/domain.md`.
