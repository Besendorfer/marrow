# Marrow

[![GitHub Release](https://img.shields.io/github/v/release/Besendorfer/marrow?label=release)](https://github.com/Besendorfer/marrow/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/Besendorfer/marrow/total)](https://github.com/Besendorfer/marrow/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20(Apple%20Silicon)-lightgrey)](https://github.com/Besendorfer/marrow/releases/latest)

A desktop app that uses AI to surface only the parts of a GitHub pull request that matter -- business logic, infrastructure, API changes -- so you can focus your review where it counts.

<p align="center">
  <img src="assets/screenshot.png" alt="Marrow reviewing a merged PR: AI change groups in the sidebar, a split diff, and the comments panel with a review thread beside the code" width="900" />
</p>

## Why?

A typical pull request touches 30-50 files. Most of them are UI components, tests, lock files, and config -- noise that buries the changes that actually matter. Reviewers either spend time scrolling past irrelevant diffs or start skimming and miss critical logic changes.

Marrow loads a PR, uses AI to classify every file by what it contains, and surfaces only the business logic, infrastructure, and API changes. A second AI pass highlights the specific lines that deserve attention -- auth changes, removed safety checks, breaking API contracts -- so you know exactly where to focus.

**In a typical 40-file PR, only 8-12 files contain reviewable business logic. Marrow finds them in seconds.**

## Download

**[Download for macOS (Apple Silicon)](https://github.com/Besendorfer/marrow/releases/latest/download/Marrow_aarch64.dmg)** -- or install with Homebrew:

```bash
brew install --cask besendorfer/tap/marrow
```

Or browse [all releases](https://github.com/Besendorfer/marrow/releases).

> The app is signed and notarized by Apple. After downloading the `.dmg`, open it and drag "Marrow" to your Applications folder. Auto-updates are built in -- you'll be notified when new versions are available (the Homebrew cask defers to the built-in updater, so `brew upgrade` won't fight it).

## Terminal (CLI/TUI)

Marrow also ships as a terminal app -- the same fetch/classify/review workflow in an interactive TUI, built on the same core (no webview). It installs a command named **`marrow`**.

<p align="center">
  <img src="assets/tui.png" alt="The marrow TUI reviewing a PR: change groups and files in the left sidebar, a syntax-colored diff on the right, keybindings along the bottom" width="850" />
</p>

```bash
# Homebrew (macOS / Linux):
brew tap besendorfer/tap
brew install marrow
# (if Homebrew refuses with an "untrusted tap" error: brew trust besendorfer/tap)

# Prebuilt binary: download a tarball for your platform from the CLI releases
# (tags starting `cli-v`) and put `marrow` on your PATH.

# Cargo (builds from source):
cargo install marrow-cli            # crates.io  (add --version 0.1.0-alpha.1 while pre-release)
cargo install --path app/src-tauri/crates/cli   # from a clone

marrow init        # scaffold ~/.config/marrow/config
marrow review <pr> # fetch + classify, then open the TUI
```

`<pr>` is a URL, `owner/repo/pull/N`, or `owner/repo#N`. See [the CLI readme](app/src-tauri/crates/cli/README.md) for the full command list. Homebrew needs the tap to exist -- see [PUBLISHING.md](PUBLISHING.md).

## How it works

1. **Fetch** -- pulls PR metadata, file list, and diff via the GitHub REST API
2. **Classify** -- sends the file list and diff to Claude, which labels each file as RELEVANT or NOT_RELEVANT based on what it contains
3. **Highlight** -- a second AI pass identifies specific lines in relevant files that deserve human attention (security changes, behavior changes, removed safety checks, etc.)
4. **Summarize** -- generates a high-level summary of the PR's changes
5. **Review** -- lands you on a summary-first overview (description, CI, labels, mergeable state), then guides you file by file through split, unified, or full-file diffs with syntax highlighting and AI-annotated risk indicators; resolve highlights with a reason, ask the AI questions about the diff, post AI notes as editable comment drafts, and work review threads in a panel beside the code -- and keep an eye on every PR you care about from the activity mini-player

## Features

### A guided path through every PR

- **Review Queue home** -- open the app to the PRs that need you: review requests with author avatars, approval counts, and reason lines, plus a drafts toggle and recently analyzed PRs. The omnibox opens any PR ref or filters the queue as you type.
- **Overview first** -- every PR starts at a summary page: the AI summary, the PR description rendered as markdown, and meta chips for CI status, mergeable/conflict state, labels, draft state, author, branch, and total size -- then a guided next-file bar walks you through the relevant files in order, tracking progress as you go.
- **⌘K command palette** -- every action searchable from the keyboard.
- **Guided first run** -- a two-step welcome validates your GitHub token and AI provider before you ever see an empty screen.

### Stay on top of PR activity

<p align="center">
  <img src="assets/mini-player.png" alt="The floating Marrow Activity mini-player listing your PRs with unseen-activity indicators (two PRs blurred)" width="480" />
</p>

- **Activity mini-player** -- a compact, always-current view of new comments, status changes, new commits, and review requests across the PRs you care about. It runs as an in-app dock *and* an optional floating, always-on-top window that appears when you switch away from Marrow and tucks itself away when you return (resizable; remembers its size and position).
- **Watches** -- follow any org or repo via saved GitHub searches, including PRs that don't request you as a reviewer. A configurable per-watch cap keeps busy queries tidy, with search + source filtering to narrow the feed.
- **Focused on what needs you** -- PRs you've approved drop out of the feed (optional setting), and your own comments don't get flagged as new activity.
- **Review request list** -- incoming review requests from GitHub, in one place.

### Find what matters in a diff

- **AI classification** -- files automatically categorized and scored by risk level (critical / high / medium / low)
- **AI highlights** -- specific lines annotated with severity (critical / warning / info) and explanatory comments -- **resolve the ones you've handled with a reason** (fixed / intentional / noise), remembered per PR and fed back to the AI so re-analysis doesn't re-flag what you've already triaged; an indicator counts new notes whenever the PR updates
- **AI summaries** -- high-level overview of what the PR changes and why it matters
- **Ask AI** -- a streaming chat about the diff itself ("why did this signature change?", "what calls this?"), with per-PR history
- **Change groups** -- AI-generated logical grouping of related file changes
- **Split, unified, and full-file diff views** -- toggle side-by-side or unified, with syntax highlighting; expand any file to its full contents when you need surrounding context
- **File sidebar** -- files grouped by category with risk indicators and viewed-progress tracking; **show or hide the "not relevant" files** on demand
- **Search** -- full-text search across all diffs with result navigation
- **Keyboard-driven** -- jump between hunks and findings, fold sections, and navigate without leaving the keyboard

### Review and collaborate

- **Comments panel** -- review threads live in a slide-in panel beside the diff, so the code never leaves the screen: grouped by file with an unresolved/all filter, markdown bodies (including GitHub `suggestion` blocks), and jump-to-thread that scrolls the diff to the exact line. Reply, resolve, edit, and react without switching context.
- **AI notes → comment drafts** -- turn an AI highlight into a comment, pre-filled and editable, before it ever reaches GitHub
- **PR checks** -- monitor CI/CD check status with blocking-check alerts
- **Merged & approved at a glance** -- a "Merged" badge when a PR lands (even while you're viewing it) and an "Approved by you" badge once you've signed off
- **Viewed file tracking** -- persistent progress across sessions, with stale-file detection
- **PR update detection** -- detects new commits and flags files changed since your last review

### Workflow

- **PR opener** -- paste a PR URL or short ref (`owner/repo#123`) directly in the app
- **Multi-tab** -- open multiple PRs simultaneously in separate tabs
- **Bring your own AI** -- Anthropic, OpenAI, Gemini, AWS Bedrock, the Claude CLI, or any OpenAI-compatible endpoint (OpenRouter, a local server)
- **Auto-update** -- background update checks with one-click download and relaunch
- **Drag-and-drop** -- drop a manifest JSON file onto the app to load a review
- **Settings** -- configure your model/provider, API keys, and GitHub token from within the app

## Prerequisites

You'll need:

- An **AI model + key**. The provider is auto-detected from the model name; set the matching API key (config setting or env var):
  - `claude*` -> **Anthropic** (`anthropic_api_key` / `ANTHROPIC_API_KEY`), or just a model name with the [Claude CLI](https://docs.anthropic.com/en/docs/claude-cli) installed
  - `gpt*` / `o3*` -> **OpenAI** (`openai_api_key` / `OPENAI_API_KEY`)
  - `gemini*` -> **Gemini** (`gemini_api_key` / `GEMINI_API_KEY`)
  - an AWS **Bedrock** model ARN with AWS credentials configured (env vars, `~/.aws/credentials`, or SSO)
  - **OpenRouter / a local server / any OpenAI-compatible endpoint** -- set `provider=openai-compatible` + `openai_base_url` + `openai_api_key`
- A **GitHub personal access token** (or `GH_TOKEN` / `GITHUB_TOKEN` environment variable)

## Configuration

Settings are stored in `~/.config/marrow/config` (run `marrow init` to scaffold it) and can be edited from the app's Settings modal.

| Setting | Description |
|---|---|
| `model` | Model name (`claude*` / `gpt*` / `gemini*`) or AWS Bedrock ARN |
| `github_token` | GitHub personal access token (optional if `GH_TOKEN` / `GITHUB_TOKEN` env var is set) |
| `anthropic_api_key` | Anthropic API key for `claude*` models (or `ANTHROPIC_API_KEY`) |
| `openai_api_key` | OpenAI / OpenAI-compatible key for `gpt*`/`o3*` (or `OPENAI_API_KEY`) |
| `gemini_api_key` | Gemini key for `gemini*` models (or `GEMINI_API_KEY`) |
| `provider` | Optional override (`openai` / `gemini` / `openai-compatible` / …); blank = auto-detect |
| `openai_base_url` | OpenAI-compatible base URL for OpenRouter / a local server (or `OPENAI_BASE_URL`) |
| `aws_profile` | AWS profile name (only used with Bedrock ARNs; uses default credential chain if empty) |

GitHub token resolution order: config file > `GH_TOKEN` env > `GITHUB_TOKEN` env.

> **Secret storage.** The token and API keys are written to this config file in
> **plaintext** (`0600` / owner-only on macOS & Linux; user-profile ACLs on
> Windows). Keys are never logged or printed (`marrow settings` shows only
> `set` / `not set`). To keep secrets off disk, leave the fields blank and use
> the environment variables instead. Note that `~/.config` can be picked up by
> backups / dotfile sync. See [SECURITY.md](SECURITY.md); OS-keychain storage
> is a tracked future enhancement.

When using a Bedrock ARN, the AWS region is extracted automatically from the ARN. When using a model name, the app invokes the `claude` CLI.

## Building from source

### Prerequisites

- [Rust](https://rustup.rs/)
- [Bun](https://bun.sh/)
- Tauri v2 prerequisites: see [Tauri Getting Started](https://v2.tauri.app/start/prerequisites/)

### Setup and build

```bash
cd app
bun install
bun run tauri build
```

The built app will be at `app/src-tauri/target/release/bundle/macos/Marrow.app`.

### Development

```bash
cd app
bun install
bun run tauri dev
```

## How files are classified

**RELEVANT** (shown for review):
- Backend business logic (services, handlers, controllers, routers, middleware)
- Infrastructure-as-code (CDK, SST, Terraform, CI/CD workflows)
- API routes, tRPC routers, REST endpoints
- Database schemas, migrations, seeds
- Auth/authz logic
- Domain types (DTOs, entities, models)
- Configuration that affects runtime behavior (feature flags, environment configs)
- Shared libraries and utilities used by business logic

**NOT RELEVANT** (skipped):
- UI components (React JSX, CSS, layouts)
- Tests (any file matching `*.test.*`, `*.spec.*`, `__tests__/`, etc.)
- Documentation
- IDE/editor config
- Package manager lock files
- Build/tooling config
- Type declaration files that only re-export or define UI prop types
- Static assets and auto-generated files

## Legacy: the `rr` script

Marrow began as [`rr`](rr), a standalone Bash script driving the same fetch/classify/highlight workflow through the `gh` and `claude` CLIs. It still works (`rr <pr-url-or-ref> --help`), but the [`marrow` CLI/TUI](#terminal-clitui) above supersedes it -- same idea, real core, no Bash.

## License

MIT

<!-- annotation fixture line A (#61 live test) -->
<!-- annotation fixture line B -->
