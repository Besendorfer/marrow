# Marrow (repo: relevant-reviews)

AI-powered GitHub PR review app — surfaces only the diffs that matter. Tauri v2 desktop app (macOS-only), plus a Rust CLI/TUI (`marrow`), a browser extension, and a legacy bash script (`rr`). GitHub: `Besendorfer/marrow`.

## Commands

All app commands run from `app/`. Use **Bun**, not npm.

- `bun install` — install frontend deps (`bun.lock` is the tracked lockfile; `bun.lockb` is gitignored)
- `bun run tauri dev` — full app in dev (starts Vite on :1420 via `beforeDevCommand`, rebuilds Rust on change)
- `bun run dev` — Vite only. The UI loads but every `invoke()` fails outside the Tauri shell — use for CSS/layout work only.
- `bun run build` — `tsc && vite build`. **This is the TypeScript typecheck** (no separate lint/typecheck script).
- `bun test` — frontend unit tests (chat protocol invariants in `src/components/chatProtocol.test.ts`); Rust tests via `cargo test -p marrow-core`.
- `bun run tauri build` — full signed macOS bundle → `app/src-tauri/target/release/bundle/macos/Marrow.app`. Slow; needs the signing identity — don't run to "verify" a change, use `cargo check` + `bun run build`.
- `cargo check` (from `app/src-tauri/`) — Rust typecheck for the whole workspace (desktop crate + `crates/core` + `crates/cli`)
- `cargo run -p marrow-cli -- review <pr>` (from `app/src-tauri/`) — run the terminal app

## Architecture

- `app/src/` — React 19 + TypeScript + Vite 6 frontend. Two entry points (see `vite.config.ts`): `index.html`/`main.tsx` (main window) and `widget.html`/`widget.tsx` (floating activity mini-player window).
- `app/src-tauri/` — Rust workspace root (Tauri desktop crate; `target/` and `Cargo.lock` live here on purpose):
  - `src/commands.rs` — all `#[command]` Tauri commands (thin wrappers)
  - `src/lib.rs` — app setup, deep-link handling, mini-player NSPanel logic, and the `generate_handler![...]` registration list (~line 218)
  - `crates/core` (`marrow-core`) — ALL business logic: GitHub API, AI providers (Anthropic/OpenAI/Gemini/Bedrock/claude CLI), config, caches. Shared by desktop and CLI. Put new logic here, not in `commands.rs`.
  - `crates/cli` (`marrow-cli`) — terminal frontend (binary named `marrow`), published to crates.io
- `browser-extension/` — MV3 content script for `github.com/*/*/pull/*` that injects an "Open in Relevant Reviews" button firing the `relevantreviews://` deep link. No build system: `build.sh` just zips Chrome/Firefox variants into `browser-extension/dist/`. It does not talk to the app directly — only via the deep link.
- `rr` (repo root) — standalone bash predecessor using `gh` + `claude` CLIs; independent of the app.
- `scripts/resolve-highlights.mjs` — dismiss AI highlights from outside the app (writes the same `~/.config/marrow` state files the app reads).

## Rust ↔ frontend boundary (Tauri IPC)

Adding a command takes THREE steps or it silently doesn't exist:
1. Write `#[command] pub fn my_cmd(...)` in `app/src-tauri/src/commands.rs` (delegate real work to `marrow-core`)
2. Add `commands::my_cmd` to `tauri::generate_handler![...]` in `app/src-tauri/src/lib.rs`
3. Call it from TS: `import { invoke } from "@tauri-apps/api/core"; await invoke("my_cmd", { prRef })`

Notes:
- Arg names: snake_case in Rust ⇄ camelCase in JS (Tauri converts automatically). Shared types live in `app/src/types.ts` and `crates/core/src/types.rs` — keep them in sync by hand; there is no codegen.
- Rust→frontend push uses events (`app.emit` / `listen` from `@tauri-apps/api/event`), e.g. deep-link opens and activity updates.
- Per-window permissions live in `app/src-tauri/capabilities/` (`default.json` for `main`, `mini-player.json` for `activity-widget`). A new plugin API used from a window fails at runtime until its permission is added there.

## Release process

- **Desktop app**: push a `v*` tag → `.github/workflows/release.yml` builds on `macos-14` (aarch64 only), signs, notarizes, and creates a **draft** GitHub release with updater artifacts (`latest.json`). Publish the draft manually.
- **ALWAYS write release notes before publishing** — the draft's body is empty (no `releaseBody` in the workflow). Write them from `git log <prev-tag>..<tag>` in the house style of past releases (one-line tagline, then `##` sections with bold-led bullets, user-facing language). Applies to `cli-v*` releases too.
- **Versions in-repo are stale on purpose**: CI stamps the version from the tag into `package.json`/`tauri.conf.json` via `sed`. Repo says `0.3.1`; latest real release is the highest `v*` tag (`git tag --sort=-creatordate`). Do NOT hand-bump these files for an app release.
- **CLI**: separate `cli-v*` tags → `cli-release.yml` (macOS/Linux/Windows binaries + Homebrew formula → `besendorfer/tap`). crates.io publish is manual, always `marrow-core` before `marrow-cli` — see `PUBLISHING.md`. CLI version is `[workspace.package]` in `app/src-tauri/Cargo.toml`.
- Auto-updates require `TAURI_SIGNING_PRIVATE_KEY` secrets in CI; the matching pubkey is in `tauri.conf.json`. Never rebuild/replace release assets after a run (hashes in `latest.json`/`marrow.rb` won't match).

## Gotchas

- **macOS-only by design**: `macOSPrivateApi`, `objc2`/`tauri-nspanel` deps under `cfg(target_os = "macos")`, Developer ID signing in `tauri.conf.json`. Don't gate desktop features on cross-platform support; the CLI is the cross-platform surface.
- **Mini-player is fragile**: the floating window is a non-activating NSPanel driven by polling `NSApplication.isActive` (no webview focus event exists for app reactivation). Read the comments in `lib.rs` and `docs/mini-player.md` before touching show/hide logic — many past regressions here (see git log).
- **Generated / ignored**: `app/dist/`, `app/src-tauri/target/`, `app/src-tauri/gen/`, `browser-extension/dist/`. Never edit these. `Cargo.lock` IS tracked (binary crates) — commit its changes.
- **Chat action protocol is triplicated**: `CHAT_UI_ACTIONS` (`crates/core/src/chat.rs`), the `ChatAction` union (`app/src/types.ts`), and `isChatAction` (`app/src/components/RichText.tsx`) must stay in sync by hand — sync markers at each site. The `marrow-card` answer-card protocol (issue #166) is triplicated the same way, across its own three sites: `CHAT_ANSWER_CARDS` (`crates/core/src/chat.rs`), the `ChatCard` union (`app/src/types.ts`), and `isChatCard` (`app/src/components/ChatCards.tsx`).
- **PR-ref regex is quadruplicated**: `browser-extension/content.js`, the bookmarklet in `app/src/components/SettingsModal.tsx`, `crates/core/src/pr_parser.rs`, and `app/src/utils.ts` must stay in sync — a comment in `content.js` marks this.
- **User state lives in `~/.config/marrow/`** (config with plaintext API keys, manifest cache, viewed/dismissed state). Treat it as real user data — never clear it in tests or "cleanup".
- Vite ignores `**/src-tauri/**` in its watcher; Rust edits are picked up by the Tauri CLI, not Vite.
- `.env` at repo root exists locally and is gitignored — don't commit or overwrite it.

## Commits / PRs

- Conventional commits with scopes, e.g. `feat(gui): ...`, `fix(activity): ...`, `refactor(core): ...`, `docs: ...`
- Work branches off `main`, merged via PR (`Merge pull request #N` in history). Before opening a PR: `bun run build` and `cargo check` both clean.
