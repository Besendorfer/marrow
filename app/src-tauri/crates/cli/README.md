# marrow

Surface the parts of a GitHub pull request that matter — an AI-assisted PR
review tool for the terminal.

`marrow` loads a PR, uses AI to classify every file by what it contains, and
surfaces only the business-logic, infrastructure, and API changes — then opens
them in an interactive TUI with syntax-highlighted diffs, AI risk annotations,
inline review threads, and write actions (comment, reply, resolve, approve).

It's the terminal frontend for [Marrow](https://github.com/Besendorfer/marrow),
built on the shared `marrow-core` crate (no Tauri, no webview).

## Install

```bash
cargo install marrow-cli
```

The crate is published as `marrow-cli` (the bare `marrow` name is taken on
crates.io), but it installs a command named **`marrow`**.

## Setup

```bash
marrow init          # scaffold ~/.config/marrow/config
marrow settings      # check resolved config (token source is masked)
```

You'll need a GitHub token (in the config, or `GH_TOKEN` / `GITHUB_TOKEN`) and a
Claude model. Simplest setup — no AWS, no extra CLI:

- set `model` to a model name (e.g. `claude-sonnet-4-6`), and
- set an Anthropic API key via `anthropic_api_key` in the config or the
  `ANTHROPIC_API_KEY` env var ([console.anthropic.com](https://console.anthropic.com)).

Alternatively, use a model name with the [`claude` CLI](https://docs.anthropic.com/en/docs/claude-cli)
installed, or an AWS Bedrock model ARN with AWS credentials configured.
`marrow settings` shows which backend your config resolves to.

## Usage

```bash
marrow review <pr>     # fetch + classify, then open the interactive TUI
marrow diff <pr>       # raw relevance-ordered unified diff (pipe to nvim/delta)
marrow comments <pr>   # show review threads
marrow requests        # PRs awaiting your review
```

`<pr>` is a URL, `owner/repo/pull/N`, or `owner/repo#N`. Run `marrow --help` for
the full command list. Mutating commands are gated behind `--yes` when stdin
isn't a terminal.

## License

MIT
