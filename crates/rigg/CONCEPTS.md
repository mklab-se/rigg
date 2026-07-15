# Concepts

rigg has two levels: a **workspace** and its **projects**. Understanding the
split is the key to using rigg well.

## Workspace vs project

- A **workspace** (`rigg.yaml`) is the top level. It declares your
  **environments** (dev, test, prod) and the **service connections** each
  environment points at — which Azure AI Search service, which Microsoft
  Foundry account/project — plus shared assets like `apis/`. A workspace holds
  *no* resource definitions itself.
- A **project** (`projects/<name>/`) is a **named group of resource
  definitions you pull, push, diff, review, and deploy as one unit**. Indexes,
  indexers, skillsets, knowledge bases, agents, and model deployments live as
  files inside a project.
- **A resource belongs to exactly one project.** rigg enforces this. It is what
  makes sync unambiguous: when you push a project, rigg knows exactly which
  remote resources that project owns — so it never half-syncs or fights another
  project over the same resource.

## Why two levels?

The workspace answers *"where do things go?"* — which services and
environments, shared across everything. Projects answer *"what do I manage
together?"* — the unit of change, review, and deployment.

Separating them means you can promote one coherent project from dev to prod
without dragging along unrelated resources, and different projects can be owned
and reviewed independently while sharing the same service and environment
configuration.

## One or many projects? Choosing boundaries

Use **one** project when your whole stack ships and is reviewed together — for
example, a single agent plus the retrieval pipeline it depends on.

Use **several** projects to draw boundaries you care about:

- **By deployable unit** — each agent or app that ships independently.
- **By ownership / review scope** — a team owns its project; pull requests stay
  focused on one project's files.
- **By lifecycle** — group things that change on the same cadence; separate
  things that don't.

Rule of thumb: **if you would pull, push, and review it as a unit, it is a
project.** If two things never need to deploy together, they can be separate
projects.

Because a resource lives in exactly one project, a *shared* resource goes in
the project that owns it; other projects refer to it by name and environment
rather than co-owning it.

**Naming:** Name a project after the thing it owns — a project holding the
`regulus` agent and its retrieval stack is naturally called `regulus`. Names
follow the same rules as resource names (no `/` or `\`, at most 260
characters).

## Workspace layout

```
rigg.yaml                     # workspace: environments + service connections
apis/<name>.json              # shared OpenAPI specs for custom Web API skills
projects/<name>/
  project.yaml                # metadata only — the directory IS the membership
  envs/<env>/
    search/{data-sources,indexes,skillsets,indexers,synonym-maps,aliases,
            knowledge-sources,knowledge-bases}/<name>.json
    foundry/{agents,deployments,connections,guardrails}/<name>.json
.rigg/<env>/<project>/...     # per-environment sync state (gitignored)
```

Platform-provided resources (such as Microsoft's built-in guardrail policies)
are never adopted or listed as unmanaged — rigg only tracks configuration you
can actually change. Your resources reference them by name instead. The same
applies to sub-resources that Azure creates automatically (for example the
index and indexer behind a managed-ingestion knowledge source) — manage the
knowledge source; Azure manages what it generates.

## Environments

An **environment** is a named Azure target — which Azure AI Search service,
which Microsoft Foundry account/project — plus an optional **policy**.
Environments are declared under `environments:` in `rigg.yaml`:

```yaml
environments:
  dev:
    default: true
    search: { service: my-search-dev }
    foundry: { account: my-foundry, project: my-project-dev }
  prod:
    policy: { protected: true }
    search: { service: my-search-prod }
    foundry: { account: my-foundry, project: my-project-prod }
```

Every project keeps a **separate resource tree per environment**, rooted at
`envs/<env>/` (see the layout above). This is deliberate: dev and prod
genuinely diverge — different field mappings while you're testing, different
agent instructions before a rollout — and a full tree, rather than a shared
file with overlay patches, makes that divergence something you can see and
diff instead of logic hidden behind a merge step.

### Logical identity vs. physical name

A resource is identified by **where its file lives**: the kind directory and
file stem (e.g. `envs/dev/search/indexes/docs-index.json` → logical id
`indexes/docs-index`) — that file path is its identity across environments.
The `name` field inside the file is a different thing: the resource's
**physical** name, what Azure actually calls it. The two usually match, but
don't have to — `envs/dev/search/indexes/docs-index.json` can have
`"name": "docs-index-dev"` while its `prod` counterpart, at the same logical
path, has `"name": "docs-index"`. rigg correlates the two files by path, not
by name, so renaming a resource in one environment never breaks its link to
the same resource in another.

### Promoting between environments

`rigg promote` copies one environment's project tree into another, entirely
locally — the only optional Azure contact is discovering function apps when
an interactive run resolves a new Web API skill URL (below):

```bash
rigg promote --from dev --to prod --dry-run          # preview only (project optional when there is exactly one)
rigg promote my-rag --from dev --to prod             # write prod's tree
rigg push my-rag --env prod                          # then sync it to Azure
```

Promotion preserves the target environment's **pinned fields** instead of
overwriting them: the resource's `name` (always — physical names are never
promoted), each kind's registry-default env-pinned fields (secrets,
write-only fields, and genuinely per-environment values like an agent's
`tools[].server_url` or a Web API skill's function `uri` and auth carrier),
and anything named in the file's own `x-rigg-pin` annotation. Resources new
in the source are created verbatim (logical id preserved, physical name
copied as-is); resources that only exist in the target are left untouched.
A→B and B→A are the same operation — you choose the sync direction with
`--from`/`--to`, not a fixed "deploy" direction.

A skillset that is **new** in the target has no URL to pin: its custom Web
API skills would silently keep calling the SOURCE environment's function.
Interactive promotes resolve each such URL — automatically kept when your
login sees only that one function app (the environments share it), otherwise
rigg lists the visible function apps (ARM) or asks for the URL. A skill's
`x-rigg-auth` annotation never crosses environments; authorize the new env's
function on the next `rigg push`, which gates on it. Non-interactive
promotes copy the URL as-is and flag it for review.

### Protected environments

Marking an environment `policy: { protected: true }` requires an explicit,
per-invocation confirmation before rigg mutates it: `push` (create/update or
`--prune`) and `delete --remote`.

```bash
rigg push my-rag --env prod --yes                        # blocked: prod is protected
rigg push my-rag --env prod --yes --confirm-env prod     # proceeds
```

Interactively, rigg instead prompts you to type the environment's name.
`--yes` alone never satisfies a protected environment's gate — it only skips
the routine "apply N changes?" prompt, and scripts reach for it reflexively;
if it also cleared this gate, a protected environment would be no safer than
an ordinary one.

## See also

- **Getting Started** (`GETTING_STARTED.md`) — build a stack from scratch.
- Run `rigg describe` to see how your resources connect, and `rigg status` to
  see what is in sync.
