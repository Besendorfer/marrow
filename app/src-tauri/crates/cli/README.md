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
# Homebrew (macOS / Linux):
brew install besendorfer/marrow/marrow

# Cargo (from crates.io):
cargo install marrow-cli
```

Or download a prebuilt binary from the `cli-v*` GitHub releases. The crate is
published as `marrow-cli` (the bare `marrow` name is taken on crates.io), but it
installs a command named **`marrow`**.

## Setup

```bash
marrow init          # scaffold ~/.config/marrow/config
marrow settings      # check resolved config (token source is masked)
```

You'll need a GitHub token (in the config, or `GH_TOKEN` / `GITHUB_TOKEN`) and an
AI model + key. The **provider is auto-detected from the model name** — set the
matching API key (config field or env var):

| Model name | Provider | Key |
| --- | --- | --- |
| `claude*` | Anthropic | `anthropic_api_key` / `ANTHROPIC_API_KEY` (or the `claude` CLI) |
| `gpt*`, `o3*` | OpenAI | `openai_api_key` / `OPENAI_API_KEY` |
| `gemini*` | Gemini | `gemini_api_key` / `GEMINI_API_KEY` |
| `arn:aws:bedrock:…` | AWS Bedrock | AWS credentials + `aws_profile` |

For **OpenRouter, a local server, or any OpenAI-compatible endpoint**, set
`provider=openai-compatible`, `openai_base_url`, and `openai_api_key`. To
override auto-detect, set `provider` explicitly. `marrow settings` shows which
backend your config resolves to.

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
