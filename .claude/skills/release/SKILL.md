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

- Run `cargo fmt --all -- --check` — abort if formatting issues. If you fix formatting with
  `cargo fmt --all`, re-run clippy afterwards: reformatting can change what clippy flags
- Run `cargo clippy --workspace --all-targets -- -D warnings` — abort if warnings
  (`--all-targets` matches CI: it also lints tests and benches)
- Run `cargo test --workspace` — abort if any test fails
- Run `grep -rn "include_str!" crates/*/src` and verify every path stays inside
  its crate directory (no `../` escaping above the crate root) — `cargo publish`
  packages only the crate directory, so an `include_str!` reaching into the repo
  root builds fine locally but fails tarball verification during publish
  (this broke the v1.2.1 publish via CONCEPTS.md)
- Run `git status` — abort if there are uncommitted changes that are NOT documentation, version, or dependency files

### 4. Bump version numbers

- Update `version` in the root `Cargo.toml` `[workspace.package]` section
- Update internal crate dependency versions (`rigg-core`, `rigg-client`, `rigg-diff`) in the root `Cargo.toml` `[workspace.dependencies]` section — they use `version = "X.Y.Z"` format (no `=` prefix)

### 5. Update documentation

- **CHANGELOG.md**: Rename the `[Unreleased]` section to `[{NEW_VERSION}] - {TODAY}` (YYYY-MM-DD format). If there is no `[Unreleased]` section, create a new dated entry summarizing changes since the last release
- **README.md**: Review for accuracy — update any version references if present
- **CLAUDE.md**: Review for accuracy — no version references to update typically
- **INSTALL.md**: Review for accuracy — no version references to update typically

### 6. Verify the build

- Run `cargo build --workspace` to ensure everything compiles with the new version
- Run `cargo test --workspace` once more after version bump

### 7. Commit, push, and tag

- Stage all changed files: `Cargo.toml`, `CHANGELOG.md`, and any updated docs
  (`Cargo.lock` is gitignored in this repo — do not force-add it)
- Commit with message: `Release v{NEW_VERSION}`
- Push to main: `git push`
- Create and push tag: `git tag v{NEW_VERSION} && git push origin v{NEW_VERSION}`

### 8. Watch and verify

- The tag push triggers the Release workflow. Do NOT declare success yet — watch it:
  `gh run list --repo mklab-se/rigg --workflow release.yml --limit 1`, then
  `gh run watch <id> --repo mklab-se/rigg --exit-status` until it completes
- If it fails, inspect with `gh run view <id> --log-failed`, fix the cause, and re-release as a patch
- When it is green, confirm the outputs:
  - `gh release view v{NEW_VERSION} --repo mklab-se/rigg` lists 4 archives
    (3 × `.tar.gz`, 1 × `.zip`) plus 4 matching `.cdx.json` SBOMs
  - `cargo search rigg --limit 1` shows the new version on crates.io
  - `Formula/rigg.rb` in `mklab-se/homebrew-tap` carries the new version

### 9. Confirm

- Tell the user the release is tagged, pushed, and the workflow is green — auditable binaries and
  SBOMs are attached to the GitHub Release, crates.io is published, and the Homebrew tap is updated
- The publish jobs require the `CARGO_REGISTRY_TOKEN` (in the `crates-io` environment) and
  `HOMEBREW_TAP_TOKEN` (repo secret) to be configured — see README.md
