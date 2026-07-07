---
name: api-watchdog
description: Check whether rigg's pinned Azure API versions are still the newest available. Use at the START of every coding session in the rigg repository, and whenever the user asks about Azure API versions, api-version updates, or whether rigg is up to date with Azure AI Search / Microsoft Foundry APIs.
---

# Azure API version watchdog

rigg pins Azure api-versions as constants in `crates/rigg-core/src/registry.rs`
(search stable/preview, foundry data plane, CognitiveServices ARM). Azure ships
new versions regularly; rigg should be updated promptly when they do.

## Check (run this first)

```bash
cargo run -q --bin rigg -- dev api-check
```

- **Exit 0, all ✓** — rigg is current. Say so briefly and move on.
- **Exit 1, any ✗ BEHIND** — Azure has newer versions. Tell the user which API
  is behind, then offer to start the upgrade:
  1. Research the new version's changelog (learn.microsoft.com; the release
     notes for Azure AI Search, and the azure-rest-api-specs folder diff).
  2. Update the version constants and any capability/volatile-field changes in
     `crates/rigg-core/src/registry.rs`.
  3. Cross-check section 2 of `docs/superpowers/specs/2026-07-07-rigg-1.0-redesign-design.md`
     and update it if resource shapes changed.
  4. Run the full gate (`cargo fmt --all -- --check && cargo clippy --workspace
     --all-targets -- -D warnings && cargo test --workspace`) and the live smoke
     flow against mklabsrch/mklabaifndr before releasing.
- **? lookup failed** — network problem; not an error. Mention it and continue.

The GitHub Action `.github/workflows/api-watchdog.yml` runs the same check
weekly and opens an issue labeled `azure-api-versions` when rigg falls behind.
