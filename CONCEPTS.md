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

## See also

- **Getting Started** (`GETTING_STARTED.md`) — build a stack from scratch.
- Run `rigg describe` to see how your resources connect, and `rigg status` to
  see what is in sync.
