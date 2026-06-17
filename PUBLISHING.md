# Publishing the `marrow` CLI to crates.io

The terminal app is published as two crates from the workspace at
`app/src-tauri/`:

| Crate         | crates.io name | Notes                                            |
| ------------- | -------------- | ------------------------------------------------ |
| `crates/core` | `marrow-core`  | Library. The bare name was free.                 |
| `crates/cli`  | `marrow-cli`   | Binary is `marrow` (bare `marrow` crate is taken).|

`cargo install marrow-cli` installs a command named **`marrow`**.

## Current status: pre-release

The workspace version is **`0.1.0-alpha.1`** (set once in
`[workspace.package]` in `app/src-tauri/Cargo.toml`; both crates inherit it).

Publishing this pre-release **claims both names permanently** without burning a
stable `0.1.0`. Pre-releases are *not* selected by `cargo install marrow-cli`
or `^` version requirements unless a user explicitly asks for them:

```bash
cargo install marrow-cli --version 0.1.0-alpha.1
```

So a casual `cargo install marrow-cli` will report "no stable release" until a
real `0.1.0` is published — which is the point: the names are reserved, but the
not-yet-announce-ready build isn't handed to people by default.

> Reminder: crates.io versions are **immutable and permanent** — you can't
> overwrite or delete a published version, only `yank` it (which hides it from
> new resolution but leaves it downloadable forever). "Republishing" always
> means publishing a *higher* version.

## Publish order (always core → cli)

`marrow-cli` depends on `marrow-core`, so `marrow-core` must exist on crates.io
first. From `app/src-tauri/`:

```bash
# 0. dry-run first (no upload) — note: the marrow-cli dry-run errors with
#    "no matching marrow-core" until marrow-core is actually published; that's
#    expected, not a problem.
cargo publish --dry-run -p marrow-core

# 1. publish the library
cargo publish -p marrow-core

# 2. then the CLI (its marrow-core dep now resolves on crates.io)
cargo publish -p marrow-cli
```

## Cutting a later version

1. Bump `version` in `[workspace.package]` (`app/src-tauri/Cargo.toml`).
2. Bump the `marrow-core` dependency version in `crates/cli/Cargo.toml` to match
   (keep them in lockstep).
3. Re-run the publish order above.

The stable announce version is just the first non-pre-release publish (e.g.
`0.1.0`). Do that once the AI-backend first-run friction is resolved.

## Prebuilt binaries + Homebrew

Prebuilt `marrow` binaries are built by `.github/workflows/cli-release.yml`,
which is independent of the desktop app's `release.yml` (that uses `v*` tags).
The CLI uses its own **`cli-v*`** tags so the two version independently.

To cut a CLI binary release:

```bash
git tag cli-v0.1.0-alpha.1
git push origin cli-v0.1.0-alpha.1
```

The workflow builds for macOS (arm64 + Intel), Linux (x86_64), and Windows
(x86_64), then attaches to a **draft** GitHub release:

- `marrow-<target>.tar.gz` / `.zip`
- `SHA256SUMS`
- `marrow.rb` — a ready-to-use Homebrew formula with the real checksums

Review and **publish the draft release**, then the tarball download links work.

### Homebrew tap (one-time setup)

The tap lives in a repo named **`homebrew-marrow`** (Homebrew maps the tap
`besendorfer/marrow` → the repo `homebrew-marrow`):

1. Create the repo `Besendorfer/homebrew-marrow`.
2. After each CLI release, copy the generated `marrow.rb` from the release into
   `Formula/marrow.rb` in that repo and push.

Users then install with the short name after a one-time tap:

```bash
brew tap besendorfer/marrow
brew install marrow
# (or, without tapping first: brew install besendorfer/marrow/marrow)
```

> Future automation: have `cli-release.yml` push `marrow.rb` to the tap repo
> directly (needs a `HOMEBREW_TAP_TOKEN` secret with write access to the tap).
> For now the formula is generated and attached to the release for a manual
> copy — no secret required.

### Bare `brew install marrow` (homebrew-core, later)

To drop the tap entirely and get `brew install marrow`, submit the formula to
[homebrew-core](https://github.com/Homebrew/homebrew-core). That requires a
**stable** release (not a pre-release), enough **notability** (real usage —
core rejects brand-new projects), the bare name `marrow` being free in core, and
a **build-from-source** formula (`depends_on "rust"`; core doesn't ship our
prebuilt binaries). Worth doing post-`0.1.0` once Marrow has some traction;
until then the tap is the path.
