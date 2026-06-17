# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |

## Reporting a vulnerability

If you discover a security vulnerability, please report it responsibly. **Do not open a public issue.**

Email [teancum@besendorfer.net](mailto:teancum@besendorfer.net) with:

- A description of the vulnerability
- Steps to reproduce
- Potential impact

You should receive a response within 72 hours. Once confirmed, a fix will be prioritized and you will be credited in the release notes (unless you prefer to remain anonymous).

## Scope

This policy covers:

- The Marrow desktop application
- The `marrow` CLI (and the legacy `rr` script)
- GitHub API token handling
- AI provider API keys (Anthropic, OpenAI, Gemini) and AWS credential handling

## Secret storage

Marrow stores its GitHub token and AI provider API keys (`anthropic_api_key`,
`openai_api_key`, `gemini_api_key`) in a **plaintext** config file at
`~/.config/marrow/config` (`%APPDATA%\marrow\config` on Windows).

- On macOS/Linux the file is written with `0600` permissions (owner read/write
  only). On Windows it inherits your user profile's default ACLs (no extra
  restriction is applied).
- Secrets are **never printed** — `marrow settings` reports only `set` /
  `not set`, and keys are not written to logs or error messages.

This matches common dev tooling (`gh`, `~/.aws/credentials`, `~/.netrc`,
`.env`). If you'd rather keep secrets off disk entirely, leave the config
fields blank and use environment variables instead: `GH_TOKEN` /
`GITHUB_TOKEN`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`.

Be aware that anything under `~/.config` can be swept into backups or dotfile
sync (Time Machine, cloud backup, a dotfiles repo) — where the key would travel
in plaintext. Storing secrets in the OS keychain is tracked as a future
enhancement.
