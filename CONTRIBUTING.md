# Contributing to Relevant Reviews

Thanks for your interest in contributing! This guide covers how to get started.

## Getting started

### Prerequisites

- [Rust](https://rustup.rs/)
- [Bun](https://bun.sh/)
- Tauri v2 prerequisites: see [Tauri Getting Started](https://v2.tauri.app/start/prerequisites/)

### Development setup

```bash
cd app
bun install
bun run tauri dev
```

### Building

```bash
cd app
bun install
bun run tauri build
```

## Reporting bugs

Open a [bug report](https://github.com/Besendorfer/relevant-reviews/issues/new?template=bug_report.yml). Include:

- Steps to reproduce
- Expected vs actual behavior
- App version and OS version
- Screenshots if applicable

## Suggesting features

Open a [feature request](https://github.com/Besendorfer/relevant-reviews/issues/new?template=feature_request.yml) describing the problem you're trying to solve and your proposed solution.

## Pull requests

1. Fork the repo and create a branch from `main`
2. Make your changes
3. Test that the app builds and runs (`bun run tauri dev`)
4. Open a PR against `main`

Keep PRs focused on a single change. If you're fixing a bug and adding a feature, open separate PRs.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By participating, you agree to uphold it.
