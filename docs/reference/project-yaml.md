# `project.yaml`

A **project** is the unit rigg pulls, pushes, diffs, promotes and deletes. It
is a directory under `projects/` containing a `project.yaml` and one tree of
resource definitions per environment. `project.yaml` itself holds metadata
only — one optional field. The *directory contents* are the membership: a
resource file under a project's tree is that project's resource, and no other
project may claim it.

## Complete example

```yaml
# projects/contoso-docs/project.yaml
description: Regulatory document search and the agent that answers from it
```

That is the whole file. A project with nothing to say may hold `{}`.

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `description` | string | no | — | One line about what this project owns. Shown by `rigg describe` and `rigg status`. |

Unknown keys are rejected (`deny_unknown_fields`), so a mistyped key is a
load error rather than a silently ignored setting.

## The directory tree

```
projects/contoso-docs/
  project.yaml
  envs/
    dev/
      search/
        data-sources/contoso-docs.json
        indexes/contoso-docs.json
        skillsets/contoso-enrich.json
        indexers/contoso-docs.json
        synonym-maps/contoso-terms.json
        aliases/contoso-current.json
        knowledge-sources/contoso-regulatory.json
        knowledge-bases/contoso-kb.json
      foundry/
        agents/contoso-assistant.json
        agents/contoso-assistant.instructions.md
        deployments/gpt-5.2-chat.json
        connections/contoso-kb.json
        guardrails/contoso-safety.json
    prod/
      search/…
      foundry/…
```

| Level | Meaning |
|---|---|
| `projects/<project>/` | The project. Its directory name **is** its name — there is no `name:` field to disagree with. |
| `project.yaml` | Marks the directory as a project. A subdirectory of `projects/` without one is not scanned. |
| `envs/<env>/` | One tree per environment name from `rigg.yaml`. Environments a project does not deploy to simply have no directory. |
| `search/` / `foundry/` | The service the kinds below it belong to. |
| `<kind-dir>/<stem>.json` | One resource. See [Resource files](resource-files.md). |

Projects are discovered by scanning `projects/*/project.yaml` under the
workspace's files root (the workspace directory, or the `root:` subdirectory
when `rigg.yaml` sets one). They are always processed in sorted order.

## Directory is membership

There is no manifest listing a project's resources, and nothing to keep in
sync when you add a file. Adding a resource means writing
`projects/<project>/envs/<env>/<service>/<kind-dir>/<name>.json`; removing one
means deleting the file. `rigg push --prune` and `rigg delete` are what turn a
local deletion into a remote one — see [Deletes are
explicit](#deletes-are-explicit).

Moving a resource between projects is therefore `git mv` plus a `rigg push`
from both projects. Because the resource is byte-identical in Azure either
way, nothing is re-created; only which project claims it changes.

## Exclusive ownership

Within one environment, a `(kind, name)` pair may appear in exactly one
project. rigg checks this before every operation that reads the whole
workspace:

```
Error: resource indexes/contoso-docs is defined in both project 'contoso-docs' and project 'contoso-search' — a resource must belong to exactly one project
```

The same *physical* name in two different environments is normal — that is
one logical resource deployed twice. The rule is per environment.

A related check catches two files in the same kind directory carrying the
same physical `name`:

```
Error: duplicate physical name 'contoso-docs': both projects/contoso-docs/envs/dev/search/indexes/a.json and projects/contoso-docs/envs/dev/search/indexes/b.json define a resource named 'contoso-docs' — physical (Azure) names must be unique within a kind
```

## Choosing project boundaries

A project should be the set of resources you would deploy, promote or delete
as one thing — typically one agent or one search experience together with
everything that feeds it. Two projects that always ship together should
probably be one; one project you routinely push only half of should probably
be two. `rigg concepts` walks through the trade-off in full.

Because a project is the promote unit, a resource shared by two products
(one embedding deployment used by two agents, say) forces a decision: put it
in the project that owns its lifecycle, and let the other reference it. A
reference to a resource outside the workspace is a warning, not an error —
`rigg validate --strict` turns it into one:

```
warning: [projects/contoso-assistant/envs/dev/foundry/agents/contoso-assistant.json] references deployments/text-embedding-3-large — not in this workspace (must already exist in Azure)
```

## Selecting a project on the command line

Most commands take an optional project name:

```bash
rigg status
rigg status contoso-docs
rigg push contoso-docs
rigg push --all
```

The name may be omitted when the workspace has exactly one project.
Otherwise:

```
Error: workspace has 3 projects; name one or pass --all
```

Commands that act on a single project (`promote`, `delete`, `verify`,
`describe`) accept the name but not `--all`:

```
Error: workspace has 3 projects; name one
```

And a workspace with no projects yet says so rather than doing nothing:

```
Error: workspace has no projects (create one with `rigg new project <name>`)
```

## Creating a project

```bash
rigg new project contoso-docs
```

This creates `projects/contoso-docs/project.yaml`. Resources then go into it
with `--project`, which may be omitted when there is only one project:

```bash
rigg new index contoso-docs --project contoso-docs
rigg new pipeline contoso-docs --project contoso-docs --type azureblob
```

`rigg adopt <project> all` fills a project from resources that already exist
in Azure; `rigg copy <source> <target>` duplicates a resource file locally,
optionally across projects (`rigg copy contoso-docs:indexes/a contoso-search:b`).

## Deletes are explicit

Deleting a resource file removes it from the project, not from Azure. rigg
will report the resource as an orphan (present remotely, absent locally) and
leave it alone until you say otherwise:

```bash
rigg push contoso-docs --prune
rigg delete contoso-docs --remote
```

Both are gated by the environment's `policy.protected` setting. `rigg delete`
without `--remote` removes only the local files.

## Common mistakes

**A `projects/` subdirectory with no `project.yaml`.** It is not a project
and is skipped in silence — resource files under it are invisible to every
command. Create the manifest (`rigg new project <name>`, or an empty `{}`).

**Expecting `project.yaml` to list resources.** It never does. If a resource
is missing from `rigg status`, the file is in the wrong place (wrong
environment, wrong service directory, wrong kind directory, or not `.json`),
not missing from a manifest.

**Renaming a project by editing `project.yaml`.** The directory name is the
project name; rename the directory. The `.rigg/<env>/<project>/` baseline
directory is keyed by project name too, so rename it alongside — or accept
one `Untracked` pass while rigg re-establishes baselines.

**The same resource in two projects.** Ownership is exclusive; see
[Exclusive ownership](#exclusive-ownership).

## See also

- [`CONCEPTS.md`](../../CONCEPTS.md) — workspace vs. project, and how to choose boundaries (`rigg concepts`).
- [rigg.yaml](rigg-yaml.md) — the environments a project's `envs/` directories correspond to.
- [Resource files](resource-files.md) — what goes inside each `<kind-dir>/<name>.json`.
- [State](state.md) — the per-project, per-environment baselines under `.rigg/`.
- [CLI reference](cli.md#rigg-new) — `rigg new`, `rigg copy`, `rigg adopt`.
