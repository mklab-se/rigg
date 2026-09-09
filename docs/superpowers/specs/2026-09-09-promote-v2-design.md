# rigg 2.0 — Promote v2: translation between environments

**Date:** 2026-09-09
**Status:** Design, approved direction. Workstream 2 of
`2026-09-09-rigg-2.0-scope-and-principles-design.md`. Depends on
`2026-09-09-environment-bindings-design.md` (bindings, `InfraRef`) and
`2026-09-09-interaction-model-design.md` (question protocol).

## 1. Problem (verified in 1.7.0 by reading `commands/promote.rs`)

1. Resources **new in the target** are copied verbatim: the source
   environment's storage `ResourceId`, `resourceUri`, `subdomainUrl` and
   connection `target` land in the target environment's files. Only Web API
   URIs got a one-off interactive fix (1.6.3).
2. The pin list (`env_pinned_extra`) is hand-maintained and incomplete
   (`resourceUri`, `deploymentId`, identity objects, `keyVaultUri`,
   knowledge-store and cache connections are missing).
3. **References to renamed siblings are not rewritten**: with prod's index
   physically named `docs-index` and dev's `docs-index-dev`, promoting the
   indexer writes `targetIndexName: docs-index-dev` into prod.
4. Shared-vs-different is not expressible; non-interactive promote silently
   keeps source values (issue #6, gap 1).

## 2. Definition

`rigg promote [<project>] --from <A> --to <B>` produces, for every logical
resource in `A`, the document it should have in `B`, by **translation**:

1. **Infrastructure translation** — every infrastructure reference (registry
   `InfraRef`) is parsed to a physical resource, mapped to the binding name
   it has in `A`, and rendered from `B`'s binding of the same name. Same
   physical value in both = shared, no change, still listed.
2. **Sibling translation** — every registry reference field (indexer →
   data source/index/skillset, knowledge base → knowledge sources, agent →
   deployment/connection, `x-rigg-ref`, and the knowledge-base name inside a
   `SearchKbMcpUrl`) that names a sibling by `A`'s physical name is rewritten
   to that sibling's physical name in `B` (by logical id = file stem). If the
   sibling does not exist in `B` yet, it will be created under `A`'s physical
   name in this same promote, so the reference stays as is.
3. **Kept from the target** — the resource's own `name` (physical identity is
   never promoted), any path listed in the target file's `x-rigg-pin`
   annotation (with the 1.x array semantics: target-only array elements along
   a pinned path survive), and the target file's own `x-rigg-pin`.
4. **Re-derived from the target's infrastructure** — a WebApiSkill's auth
   carrier. After the URI is translated to `B`'s function app: if `B`'s
   skillset file already has an auth carrier for that skill (`authResourceId`,
   `x-rigg-auth`, or an `x-functions-key` header) it is kept; otherwise it is
   derived from the target function app's Easy Auth state (ARM
   `authsettingsV2`, online): Easy Auth on → `authResourceId` set, key carrier
   removed; off → `x-rigg-auth: function-key` when `A` used a key, else
   anonymous. `A`'s `x-rigg-auth`, `authResourceId` and key header never cross
   as-is. Offline (`--offline`), the carrier is left unresolved and reported;
   push's auth gate handles it.
5. **Everything else** comes from `A` — that is the promotion.

Resources only in `B` are never touched. Nothing is deleted. Sidecars are
promoted as content (inline on read, extract on write).

## 3. Questions

Translation stops on anything it cannot decide. Each is a *question* in the
interaction model's protocol:

| Situation | Question | Candidates |
|---|---|---|
| `A` value matches no `A` binding (unbound) | "`<file>` at `<path>` uses storage `X`, which is not bound in `A`. Bind it as?" | proposed name; record in `rigg.yaml` for `A` |
| binding `n` exists in `A` but not in `B` | "`B` has no binding `n` (type storage). Use?" | same as `A` (shared), pick from ARM list in `B`'s subscription, enter id, skip |
| target env `B` does not exist | "Create environment `B` like `A`?" | runs `env add B --like A` (interaction spec) |
| Deployment new in `B`: model unavailable or quota short in `B`'s account region (online check) | "Model `m` version `v` is not available in `<region>` / quota `q` short of `capacity`. Continue, change capacity, or skip this deployment?" | continue / capacity / skip |
| External `api` URL with no `api` binding | "`<uri>` is an external API not bound in `A`. Keep verbatim in `B`?" | keep / bind |

Interactive: asked inline, answers that create bindings are written to
`rigg.yaml` immediately so they are remembered. Non-interactive: all pending
questions are emitted as `needs-input` JSON with exit 6 and nothing is
written; `--answer id=value` / `--answers-file` supply them. `--yes` applies a
plan that has no pending questions.

## 4. Preview

Always shown before writing (and the entire output for `--dry-run`):

```
Promote project 'regulus': dev → prod
  Targets: Search mklabsrch → mklabsrch-prod, Foundry mklabaifndr/proj-default → mklabaifndr-prod/proj-prod

Rewiring (bindings)
  docs-storage   storage        mklabstorageacc            = mklabstorageacc            shared
  enrichment     ai-services    mklabaisrvc                → mklabaisrvc-prod
  enrich-fn      function-app   mklab-enrich-fn            = mklab-enrich-fn            shared (auth: Entra, re-derived)
  foundry        (implicit)     mklabaifndr                → mklabaifndr-prod           model host for 3 references
  pipeline-mi    identity       rigg-dev-mi                → rigg-prod-mi

Renamed siblings
  indexes/docs-index       docs-index-dev → docs-index      (2 references rewritten)

Resources
  4 changed, 2 new, 3 unchanged, 1 kept (only in 'prod')
  <per-resource semantic diff, columns 'prod' | 'dev (incoming)'>

Checks
  ✓ deployments/gpt-5-mini: model available in swedencentral, quota ok
  ! agents/Regulus: instructions differ (sidecar) — content change, promoted
```

`--output json` carries the same sections: `rewiring[]`, `renamed[]`,
`resources{changed,new,unchanged,kept_only_in_to}`, `checks[]`, `questions[]`.

## 5. After writing

Hints, in order: `rigg validate <p>` (already clean by construction, shown
for the habit), `rigg auth doctor -e <B>` (what `B` needs before pushing —
workstream 3 makes doctor binding-aware and plan-aware), `rigg push <p> -e
<B> --dry-run`, `rigg push <p> -e <B>`.

## 6. Symmetry and `diff --compare-env`

A→B and B→A remain the same operation. `rigg diff --compare-env` stays a raw
environment-vs-environment comparison; "equal modulo infrastructure" is what
`promote --dry-run` shows, so no separate mode is added.

## 7. Removed from 1.x

`registry::env_pinned` / `env_pinned_extra` / `restore_path` as the merge
engine (the array-preserving restore is kept for `x-rigg-pin` only);
`resolve_new_webapi_uris` (subsumed by translation + re-derivation).

## 8. Testing

Unit (rigg-core `promote` module, moved out of the CLI crate so the MCP tool
and tests share it):

- New-in-target data source, knowledge source, index vectorizer, skillset
  embedding skill, skillset billing, knowledge base model, agent tool URL,
  connection target: each translated to the target binding; shared bindings
  unchanged and reported shared.
- Renamed sibling: indexer's three references, knowledge base → knowledge
  source, agent → deployment and connection, MCP URL's knowledge-base name.
- Auth carrier re-derivation: Easy Auth on/off, target already has a carrier,
  offline.
- `x-rigg-pin` array semantics regression (1.x test cases carried over).
- Unbound and missing-binding questions; deployment availability question
  (fake ARM); `--offline` skips online checks and reports them as skipped.
- Non-interactive: pending questions → exit 6, zero files written; answers
  via flag and file; `--yes` with no questions writes.
- JSON preview shape.

CLI (assert_cmd): preview text sections; dry-run writes nothing; hint lines.

Live (Kristofer): promote `regulus` dev → staging on `e2e-test` with staging
sharing storage and function app but a different Foundry project.

## 9. Open decisions

- Whether the deployment availability/quota check should also run for
  *changed* deployments whose capacity increased (proposed: yes, same check).
