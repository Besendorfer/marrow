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

> **Checksum integrity.** `marrow.rb`'s `sha256`s are generated from the exact
> tarballs in the same workflow run, so they match by construction. Just don't
> rebuild or replace release assets after the run — Rust builds aren't
> byte-reproducible, so a rebuilt binary has a different hash and would no
> longer match the generated formula. Publish the draft as-is, and always take
> `marrow.rb` from the **same** release the binaries are on.

### Homebrew tap

The formula lives in the **umbrella tap** `besendorfer/tap` (repo
[`Besendorfer/homebrew-tap`](https://github.com/Besendorfer/homebrew-tap)) — one
tap for all of Besendorfer's tools, so users tap once and `brew install` any of
them. (The old per-project `homebrew-marrow` tap is deprecated and redirects
here.)

Updating the tap is **automated**: when a `cli-v*` release is *published*,
`.github/workflows/homebrew-tap.yml` copies that release's generated `marrow.rb`
into `Formula/marrow.rb` in the tap repo and pushes. (It runs on publish, not on
the tag push, because a draft release's asset URLs aren't reachable yet.) This
needs a repo secret **`HOMEBREW_TAP_TOKEN`** — a PAT with `contents:write` on
`Besendorfer/homebrew-tap` (the default `GITHUB_TOKEN` can't push to another
repo). If the secret is missing the job fails loudly; fall back to the manual
copy below.

Manual fallback — copy the generated `marrow.rb` from the release into
`Formula/marrow.rb` in the tap repo and push. Users install with the short name
after a one-time tap:

```bash
brew tap besendorfer/tap
brew install marrow
# (or, without tapping first: brew install besendorfer/tap/marrow)
```

> **Untrusted-tap error?** If a user's Homebrew has `HOMEBREW_REQUIRE_TAP_TRUST`
> set, it refuses non-official taps until trusted: `brew trust besendorfer/tap`
> (one-time, per machine). Only homebrew-core formulae are exempt.

> The formula is also attached to every CLI release, so the manual copy above
> always works if the automation is unavailable (e.g. the secret is missing).

### Desktop app cask (`brew install --cask`)

The same tap also carries a **cask** for the desktop app in `Casks/marrow.rb`
(same `marrow` token as the CLI formula — `brew install besendorfer/tap/marrow`
resolves the formula, `--cask` the app):

```bash
brew install --cask besendorfer/tap/marrow
```

Updating it is automated by the same `homebrew-tap.yml` workflow: when a
desktop `v*` release is *published*, the `update-cask` job downloads that
release's `Marrow_aarch64.dmg`, computes its sha256, regenerates
`Casks/marrow.rb`, and pushes to the tap (same `HOMEBREW_TAP_TOKEN` secret,
same publish-not-tag reasoning as the formula). Backfill or re-sync with a
manual run: Actions → "Update Homebrew Tap" → `workflow_dispatch` with the
desktop tag (e.g. `v0.24.0`).

The cask declares `auto_updates true` — the app updates itself via the Tauri
updater, so `brew upgrade` leaves it alone unless `--greedy`. Never rebuild a
release's DMG after publishing: the cask pins its sha256 (same rule as
`latest.json`/`marrow.rb`).

### Bare `brew install marrow` (homebrew-core, later)

To drop the tap entirely and get `brew install marrow`, submit the formula to
[homebrew-core](https://github.com/Homebrew/homebrew-core). That requires a
**stable** release (not a pre-release), enough **notability** (real usage —
core rejects brand-new projects), the bare name `marrow` being free in core, and
a **build-from-source** formula (`depends_on "rust"`; core doesn't ship our
prebuilt binaries). Worth doing post-`0.1.0` once Marrow has some traction;
until then the tap is the path.
