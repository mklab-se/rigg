# rigg — Environments: per-env project trees, promote, policies

**Date:** 2026-07-10
**Status:** Design — approved direction by user; open forks resolved by
delegated judgment (uniform layout; in-file pin annotation). NO backwards
compatibility required (sole user, explicit instruction).
**Workstream:** H — the environments redesign.

## Problem

Environments exist today only as *named connection sets* (`rigg.yaml`
`environments:` block): the same local files push to any environment, per-env
baselines track sync separately, and `-e`/`RIGG_ENV`/`default: true` select
the target. That model cannot express the user's real scenarios:

1. **Per-env values** — a data source's `ResourceId=` connection string is
   dev's; pushing the same file to prod points prod at dev's storage. Only
   `x-rigg-ref` KB URLs get env-injected today.
2. **Env-specific resources** — experimental dev resources push wholesale to
   prod; nothing scopes a resource to an environment.
3. **Shared-service environments** — env A and env B may live in the *same*
   Search service / Foundry project with different physical resource names.
   Today identity == name, so A's and B's resources collide.
4. **Free physical naming with remembered correlation** — the user names
   resources per environment however they like; rigg must remember that
   Name-A (env A) corresponds to Name-B (env B) without inferring from names.
5. **Promotion** — a local, reviewable operation taking env A's configuration
   into env B's, then pushing B when ready. Also symmetric A↔B sync
   (hot-swap scenario).
6. **Policies** — dev pushes frictionless; prod requires explicit
   confirmation for every cloud-mutating operation.
7. Nobody is told where "(env: dev)" comes from — `rigg init` silently names
   it.

## Core design: per-environment project trees

### Layout (uniform — no special single-env case)

```
projects/<project>/
  project.yaml                      # metadata only, env-agnostic
  envs/<env>/
    search/<kind-dir>/<file>.json   # + .md sidecars, exactly as today
    foundry/<kind-dir>/<file>.json
```

- Every project stores resources under `envs/<env>/…` for each environment it
  participates in. A project participates in an environment iff the directory
  exists with files. `rigg new project` creates `project.yaml` only; env
  dirs materialize on first resource (scaffold/adopt/pull into the resolved
  env).
- **Env-specific resources fall out for free**: a resource that exists only
  in `envs/dev/` cannot reach prod — there is no file to push.

### Identity: file stem = logical, `name` field = physical

- The **relative path** (kind dir + file stem) is the resource's *logical*
  identity — the correlation across environments. Same path in `envs/a/` and
  `envs/b/` = the same logical resource.
- The **`name` field** inside the file is the *physical* Azure name for that
  environment. By default stem == name (today's behavior); they diverge only
  when the user renames a physical resource in one environment. This is how
  "Name-A belongs to env A, Name-B to env B" is remembered **without
  inference** — the user just edits the `name` field in one env's copy.
- Each env dir is a **complete, self-consistent physical description**:
  references between resources inside it use that env's physical names,
  connection strings, and URLs. No push-time rewriting, no overlays.

### Store semantics (rigg-core)

- `Store::new(project, env)` — the store root becomes
  `<project>/envs/<env>/`.
- `list()` returns refs keyed by the **physical** name (the `name` field,
  falling back to the file stem when absent), plus the path. Sync machinery
  (baselines, classify, snapshots, adoption ownership) continues to key on
  physical `kind/name` — per env, as baselines already are.
- `read(r)`/`delete(r)` locate the file whose `name` field (or stem) matches
  `r.name` by scanning the kind directory (small dirs; correctness over
  micro-optimization). `write(r, doc)` updates the located file, or creates
  `<sanitized physical name>.json` for new resources (logical id defaults to
  physical).
- Validation: the old "name must match filename" rule is **relaxed** to a
  default-affirming note; a NEW rule rejects two files in the same env dir
  with the same physical name (duplicate physical identity).
- `assert_exclusive_ownership` applies per environment (a physical resource
  belongs to exactly one project *within an env*).

### Command surface changes

- All resource commands (`adopt`, `pull`, `push`, `status`, `diff`,
  `describe`, `delete`, `validate`, `copy`, `new <resource>`, `new pipeline`)
  operate on the **resolved environment's** tree; output already shows
  `(env: X)` where relevant — `describe` gains it.
- `validate` (local-only, cheap) validates **every** env dir it finds,
  reporting per env.
- `copy` stays intra-env (the resolved env), cross-project as today.
  Cross-ENV copying is `promote`'s job.
- CLI selectors and displays keep using **physical** names (they match what
  the user sees in Azure and in `status`).

### `rigg promote <project> --from <envA> --to <envB>`

A local, reviewable, direction-explicit copy between env trees. Nothing
touches Azure.

1. **Preview**: labeled diff table of A's tree vs B's tree (columns = env
   names — reuses the diff renderer), listing per logical resource: changed /
   new-in-A (will be created in B) / only-in-B (untouched — env-specific
   resources are never deleted by promote).
2. **Apply** (after confirm; `--dry-run` stops at the preview): for each
   logical resource in A, write A's content into B's file **except pinned
   fields**, which keep B's existing values:
   - Always pinned: `name` (B keeps its physical name).
   - Registry defaults per kind (`env_pinned` = the kind's `secret_fields` ∪
     `write_only_fields` ∪ an explicit per-kind extra list — e.g. Agent:
     `tools[].server_url`, `tools[].project_connection_id`; DataSource:
     `credentials.connectionString`).
   - Per-resource additions via an `x-rigg-pin: ["<dot.path>", …]`
     annotation **in the target env's file** (travels with the resource,
     reviewable in the same diff; stripped on push like all `x-rigg-*`).
   - New-in-B files are created verbatim from A (stem preserved; user then
     renames/repoints env-specific fields — promote prints a hint listing
     the pinned-by-default fields worth reviewing on new copies).
3. Sidecars are promoted as content (inline on read, re-extract on write —
   existing store behavior).
4. After apply, hint the natural next steps:
   `rigg diff <p> -e <envB>` / `rigg push <p> -e <envB>`.
5. Symmetric by construction: A→B and B→A are the same operation — this IS
   the A/B sync + hot-swap workflow.
6. Non-interactive: `--dry-run` or `-y`; JSON output lists promoted /
   created / kept-only-in-B / pinned-fields-kept.

### Environment policies

`rigg.yaml`:

```yaml
environments:
  dev:
    default: true
    search: { service: mklabsrch }
    foundry: { account: mklabaifndr, project: proj-default }
  prod:
    policy: { protected: true }
    search: { service: mklabsrch-prod }
```

`protected: true` gates every **cloud-mutating** operation against that env
(`push` apply, `push --prune`, `delete --remote`):

- Interactive: requires typing the environment name (the `delete` pattern —
  stronger than y/N). `--yes` does NOT bypass it.
- Non-interactive: fails with exit 2 unless `--confirm-env <name>` is passed
  (the CI-safe typed equivalent; must match the env name exactly).
- Read-only ops (status/diff/pull/adopt… pull writes local only) are never
  gated. Promote INTO a protected env is local-only → not gated (the
  subsequent push is).

### UX & docs

- `rigg env add <name>` with no service flags on a TTY runs an interactive
  wizard: ARM discovery pick-lists (reusing init's discovery), then optional
  `protected` question. Flags path unchanged.
- `rigg init` output explains the environment it created: name, that
  `-e`/`RIGG_ENV` select others, and `rigg env add` for more.
- `rigg concepts` / CONCEPTS.md gain an **Environments** chapter: env =
  named target + policy; per-env trees; logical path vs physical name;
  promote; protected envs.
- README environments/promotion section rewritten; GETTING_STARTED touched
  where it mentions layout.
- `samples/` converted to the new layout (env `demo`).
- The user's live `e2e-test/` workspace (untracked) is migrated by the
  controller after merge so manual testing works immediately.

## Explicitly rejected / deferred

- Overlay/patch files and inline env-value annotations — rejected in favor of
  full per-env trees (user's explicit choice; divergence is managed by
  `promote` + preview instead of prevented by sharing).
- Automatic name suffix rules — rejected; correlation is by path, physical
  names are free (user's explicit requirement).
- A separate local-vs-local diff mode — `promote --dry-run` IS that preview
  (YAGNI).
- Backwards compatibility with the flat layout — none (explicit instruction);
  the binary reads only the new layout.

## Testing

- rigg-core: store env-rooting; physical-name lookup (stem ≠ name);
  duplicate-physical-name validation; exclusive ownership per env.
- Promote: pinned-field preservation (name + registry defaults + x-rigg-pin);
  new-in-B creation; only-in-B untouched; dry-run writes nothing; JSON shape;
  sidecar round-trip.
- Policies: protected env blocks non-interactive push without
  `--confirm-env`; `--confirm-env prod` passes; `--yes` alone rejected;
  unprotected env unchanged.
- All existing wiremock/cli tests migrated to the `envs/dev/...` layout and
  passing — they pin that sync semantics survived the re-rooting.
- Live acceptance (user, after merge): migrated e2e-test; `rigg env add`
  wizard; promote regulus dev→(new env); protected-env push gate.

## Files touched (major)

`rigg-core`: `store.rs` (env rooting, physical lookup), `workspace.rs`
(policy model, layout constants), `registry.rs` (`env_pinned` machinery),
`identity.rs` (store call). `rigg`: every command in `commands/` touching
Store; new `commands/promote.rs`; `cli.rs` (Promote, `--confirm-env`);
`env.rs` (wizard, show policy); `init.rs` (messaging). Docs: `CONCEPTS.md`,
`README.md`, `GETTING_STARTED.md`, `samples/`. Tests: store inline,
`cli_surface.rs`, `sync.rs` (layout migration), new promote/policy tests.
