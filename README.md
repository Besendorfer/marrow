# Relevant Reviews

A desktop app that uses AI to surface only the parts of a GitHub pull request that matter -- business logic, infrastructure, API changes -- so you can focus your review where it counts.

## Download

**[Download for macOS (Apple Silicon)](https://github.com/Besendorfer/relevant-reviews/releases/latest/download/Relevant.Reviews_aarch64.dmg)**

Or browse [all releases](https://github.com/Besendorfer/relevant-reviews/releases).

> The app is signed and notarized by Apple. After downloading the `.dmg`, open it and drag "Relevant Reviews" to your Applications folder. Auto-updates are built in -- you'll be notified when new versions are available.

## How it works

1. **Fetch** -- pulls PR metadata, file list, and diff via the GitHub REST API
2. **Classify** -- sends the file list and diff to Claude (via AWS Bedrock), which labels each file as RELEVANT or NOT_RELEVANT based on what it contains
3. **Highlight** -- a second AI pass identifies specific lines in relevant files that deserve human attention (security changes, behavior changes, removed safety checks, etc.)
4. **Summarize** -- generates a high-level summary of the PR's changes
5. **Review** -- displays the relevant diffs in a split or unified viewer with syntax highlighting, AI-annotated risk indicators, and inline comments

## Features

- **PR opener** -- paste a PR URL or short ref (`owner/repo#123`) directly in the app
- **AI classification** -- files automatically categorized and scored by risk level (critical / high / medium / low)
- **AI highlights** -- specific lines annotated with severity (critical / warning / info) and explanatory comments
- **AI summaries** -- high-level overview of what the PR changes and why it matters
- **Change groups** -- AI-generated logical grouping of related file changes
- **Split and unified diff views** -- toggle between side-by-side and unified diff display
- **File sidebar** -- files grouped by category with risk indicators; track which files you've reviewed
- **Review request list** -- see incoming review requests from GitHub
- **PR comments and threads** -- read, reply to, and resolve review threads; react with emoji
- **PR checks** -- monitor CI/CD check status with blocking-check alerts
- **Search** -- full-text search across all diffs with result navigation
- **Viewed file tracking** -- persistent progress tracking across sessions with stale-file detection
- **PR update detection** -- detects new commits and highlights files changed since your last review
- **Multi-tab** -- open multiple PRs simultaneously in separate tabs
- **Auto-update** -- background update checks with one-click download and relaunch
- **Drag-and-drop** -- drop a manifest JSON file onto the app to load a review
- **Settings** -- configure model ARN, GitHub token, and AWS profile from within the app

## Prerequisites

You'll need:

- An **AWS account** with access to [Amazon Bedrock](https://aws.amazon.com/bedrock/) and a Claude model enabled
- A **GitHub personal access token** (or `GH_TOKEN` / `GITHUB_TOKEN` environment variable)
- AWS credentials configured (env vars, `~/.aws/credentials`, or SSO)

## Configuration

Settings are stored in `~/.config/relevant-reviews/config` and can be edited from the app's Settings modal.

| Setting | Description |
|---|---|
| `model` | AWS Bedrock model ARN (e.g., `arn:aws:bedrock:us-east-2:123456789:application-inference-profile/...`) |
| `github_token` | GitHub personal access token (optional if `GH_TOKEN` or `GITHUB_TOKEN` env var is set) |
| `aws_profile` | AWS profile name (optional, uses default credential chain if empty) |

GitHub token resolution order: config file > `GH_TOKEN` env > `GITHUB_TOKEN` env.

AWS region is extracted automatically from the model ARN.

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

The built app will be at `app/src-tauri/target/release/bundle/macos/Relevant Reviews.app`.

### Development

```bash
cd app
bun install
bun run tauri dev
```

## How files are classified

**RELEVANT** (shown for review):
- Backend business logic (services, handlers, controllers, routers)
- Infrastructure-as-code (CDK, SST, Terraform, CI/CD workflows)
- API routes, tRPC routers, REST endpoints
- Database schemas, migrations
- Auth/authz logic
- Shared libraries used by business logic

**NOT RELEVANT** (skipped):
- UI components (React JSX, CSS, layouts)
- Tests
- Documentation
- IDE/editor config
- Package manager lock files
- Build/tooling config
- Static assets

## CLI (`rr`)

A standalone Bash script that performs the same fetch/classify/highlight workflow using the `gh` CLI and `claude` CLI. It can output results as a manifest JSON, print classification summaries, or open diffs in VSCode.

### Prerequisites

- [GitHub CLI (`gh`)](https://cli.github.com/) -- authenticated with access to your repos
- [Claude CLI (`claude`)](https://docs.anthropic.com/en/docs/claude-cli) -- for AI classification
- `jq`, `python3`

### Setup

```bash
chmod +x rr

# (Optional) Symlink into a directory on your PATH
ln -s "$(pwd)/rr" /usr/local/bin/rr

# Set the model ARN (env var or config file)
export RR_MODEL="arn:aws:bedrock:us-east-2:123456789:application-inference-profile/your-profile-id"
```

### Usage

```
rr <pr-url-or-ref> [options]
```

| Option | Description |
|---|---|
| `<pr-url-or-ref>` | GitHub PR URL, `owner/repo#number`, or just a number (if inside a repo) |
| `--list-only` | Only print the classification results, don't open any viewer |
| `--manifest-only` | Build the manifest JSON and print its path |
| `--vscode` | Open diffs in VSCode instead of the desktop app |
| `--help` | Show help |

### Examples

```bash
# Full PR URL
rr https://github.com/myorg/myrepo/pull/123

# Short form
rr myorg/myrepo#123

# Just a number (when inside a git repo with a GitHub remote)
rr 123

# List classification only
rr 123 --list-only

# Open in VSCode
rr 123 --vscode
```

## License

MIT
