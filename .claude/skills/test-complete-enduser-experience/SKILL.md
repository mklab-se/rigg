---
name: test-complete-enduser-experience
description: End-to-end test of the rigg CLI against live Azure (mklabsrch + mklabaifndr/proj-default) using the samples workspace. Run before releases and after registry/API-version changes.
---

# End-to-end test: complete rigg user experience

Tests the released user journey against live Azure. Uses ONLY `mklabsrch`
(Search) and `mklabaifndr`/`proj-default` (Foundry); creates resources inside
them and deletes everything afterwards. Keep SKUs minimal (capacity 1).

## Setup

1. Build: `cargo build` — use `target/debug/rigg`.
2. Copy `samples/` to a temp dir. In the copy:
   - `rigg.yaml`: point the `demo` env at `mklabsrch` and `mklabaifndr/proj-default`.
   - `projects/agentic-stack/search/data-sources/docs-ds.json`: set the real
     storage ResourceId (`az storage account list`) and container `rigg-e2e-docs`.
   - Rename the deployment to `rigg-e2e-model` (avoid colliding with real
     deployments) and point the agent's `model` at it. Pick a currently
     deployable model version (`az cognitiveservices account deployment list`).
3. Create the container + upload 2 small text blobs
   (`az storage container create` / `az storage blob upload-batch --auth-mode key`).

## Test sequence (all from the temp workspace)

1. `rigg validate` → exit 0.
2. `rigg push agentic-stack --dry-run` → plan lists ~9 resources in dependency
   order (data source before indexer, KS before KB, guardrail before deployment).
3. `rigg push agentic-stack --yes` → all ✓ across Search + Foundry v1 + ARM.
4. `rigg status agentic-stack` → everything "in sync" (canonicalization check:
   the data source connection string must STILL be in the local file).
5. `rigg auth doctor --fix` → storage/KB/grounding roles verified or fixed.
6. Drift cycle: mutate the remote index via `az rest`/curl (add a field) →
   `rigg status` shows "remote ahead" → `rigg diff --exit-code` exits 5 →
   `rigg pull --yes` reconciles → "in sync".
7. Ingestion: detach the skillset (`skillsetName: null` — the sample's custom
   Web API is intentionally unimplemented), `rigg push --yes`, run the indexer
   (POST `/indexers/docs-indexer/run` with `Content-Length: 0`), poll status →
   `itemsProcessed` == blob count, `itemsFailed` == 0.
8. Agent round-trip: `rigg pull --yes` → instructions stay in the `.md`
   sidecar, `x-rigg-ref` annotation survives, `server_url` contains the KB MCP
   endpoint.
9. `rigg delete agentic-stack --remote --yes` → 9 deletions in reverse order.

## Teardown checklist (MUST complete)

- [ ] `rigg delete agentic-stack --remote --yes` succeeded
- [ ] blob container `rigg-e2e-docs` deleted
- [ ] any role assignments created by `--fix` during the test removed
      (`az role assignment delete ...`) unless the user wants to keep them
- [ ] `rigg status` in the temp workspace shows only "local only" rows
