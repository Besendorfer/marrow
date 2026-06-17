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
