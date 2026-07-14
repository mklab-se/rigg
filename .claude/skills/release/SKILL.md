---
name: release
description: "Release a new version: bump version, update docs, commit, push, and tag"
argument-hint: "<major|minor|patch>"
---

Release a new version of rigg.

## Input

$ARGUMENTS must be one of: `major`, `minor`, `patch`. If empty or invalid, stop and ask.

## Steps

### 1. Determine the new version

- Read the current version from the `version` field in the workspace `Cargo.toml`
- Apply the semver bump based on $ARGUMENTS:
  - `patch`: 0.10.1 -> 0.10.2
  - `minor`: 0.10.1 -> 0.11.0
  - `major`: 0.10.1 -> 1.0.0
- Show the user: "Releasing rigg v{OLD} -> v{NEW}"

### 2. Update toolchain and dependencies

- Run `rustup update stable` — CI runs the LATEST stable Rust, and newer clippy
  versions ship new lints. Running the pre-flight checks on an older local
  toolchain lets warnings through that then fail the release workflow
  (this happened for v1.1.0 and v1.2.0). After updating, confirm with
  `rustc --version`.
- Run `cargo update` to update all dependencies to the latest compatible versions
- This ensures the release ships with up-to-date dependencies

### 3. Pre-flight checks

- Run `cargo fmt --all -- --check` — abort if formatting issues
- Run `cargo clippy --workspace --all-targets -- -D warnings` — abort if warnings
  (`--all-targets` matches CI: it also lints tests and benches)
- Run `cargo test --workspace` — abort if any test fails
- Run `git status` — abort if there are uncommitted changes that are NOT documentation, version, or dependency files

### 4. Bump version numbers

- Update `version` in the root `Cargo.toml` `[workspace.package]` section
- Update internal crate dependency versions (`rigg-core`, `rigg-client`, `rigg-diff`) in the root `Cargo.toml` `[workspace.dependencies]` section — they use `version = "X.Y.Z"` format (no `=` prefix)

### 5. Update documentation

- **CHANGELOG.md**: Rename the `[Unreleased]` section to `[{NEW_VERSION}] - {TODAY}` (YYYY-MM-DD format). If there is no `[Unreleased]` section, create a new dated entry summarizing changes since the last release
- **README.md**: Review for accuracy — update any version references if present
- **CLAUDE.md**: Review for accuracy — no version references to update typically

### 6. Verify the build

- Run `cargo build --workspace` to ensure everything compiles with the new version
- Run `cargo test --workspace` once more after version bump

### 7. Commit, push, and tag

- Stage all changed files: `Cargo.toml`, `CHANGELOG.md`, and any updated docs
  (`Cargo.lock` is gitignored in this repo — do not force-add it)
- Commit with message: `Release v{NEW_VERSION}`
- Push to main: `git push`
- Create and push tag: `git tag v{NEW_VERSION} && git push origin v{NEW_VERSION}`

### 8. Verify the release workflow

- The push triggers the Release workflow on GitHub Actions. Do NOT declare
  success yet — watch it: `gh run list --repo mklab-se/rigg --limit 3`, then
  `gh run watch <id> --repo mklab-se/rigg` (or poll `gh run view <id>`)
  until it completes
- If it fails, inspect with `gh run view <id> --log-failed`, fix the cause,
  and re-release as a patch

### 9. Confirm

- Tell the user the release is tagged, pushed, and the workflow is green —
  binaries are built, crates.io is published, and the Homebrew tap is updated
