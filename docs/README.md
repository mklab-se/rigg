# rigg documentation

`rigg` is configuration-as-code for Azure AI Search and Microsoft Foundry. A
**workspace** (`rigg.yaml`) declares environments; **projects**
(`projects/<name>/`) own resource definitions as JSON files; `pull`, `push`,
`diff` and `promote` operate on whole projects, so the entire Agentic RAG
stack lives in Git.

## Start here

Read in this order. Each step assumes the one before it.

1. [`CONCEPTS.md`](../CONCEPTS.md) — the mental model: workspace, project,
   environment, logical id vs. physical name. Also available offline as
   `rigg concepts`.
2. [Tutorial 1 — pull an existing solution](tutorials/01-pull-an-existing-solution.md)
   — put resources you already have in Azure under version control.
3. [How rigg works](how-rigg-works.md) — the sync engine, the binding layer,
   the identity graph, promote-as-translation.
4. The rest of the tutorials, then the reference pages as you need them.

## I want to…

| I want to… | Read |
|---|---|
| Understand what a project is and where its files live | [`CONCEPTS.md`](../CONCEPTS.md), [project.yaml](reference/project-yaml.md) |
| Write or fix `rigg.yaml` | [rigg.yaml](reference/rigg-yaml.md) |
| Know which environment a command will act on | [rigg.yaml § Environment resolution](reference/rigg-yaml.md#environment-resolution) |
| Point a resource at a storage account, function app or key vault | [rigg.yaml § Dependencies](reference/rigg-yaml.md#dependencies) |
| Know what a resource JSON file may contain | [Resource files](reference/resource-files.md) |
| Understand `x-rigg-api` / `x-rigg-auth` / `x-rigg-pin` / `x-rigg-ref` | [Annotations](reference/annotations.md) |
| Implement a custom Web API skill | [APIs](reference/apis.md) |
| Know what `.rigg/` holds and whether to commit it | [State](reference/state.md) |
| Look up a command or a flag | [CLI reference](reference/cli.md) |
| Configure rigg for CI or a service principal | [Environment variables](reference/environment-variables.md) |
| Script rigg, or drive it from an agent | [Exit codes and questions](reference/exit-codes-and-questions.md) |
| Take a working solution from dev to production | [Tutorial 3](tutorials/03-add-an-environment-and-promote.md), [Tutorial 4](tutorials/04-push-to-protected-production.md) |
| Let an AI agent operate rigg | [`MCP.md`](../MCP.md) |

## Tutorials

Each one is runnable top to bottom against your own subscription, states its
prerequisites and cost up front, and ends with a clean-up step.

1. [Pull an existing solution](tutorials/01-pull-an-existing-solution.md) —
   `rigg init`, `rigg adopt`, the bindings cache, the round trip back to Azure.
2. [Build from scratch](tutorials/02-build-from-scratch.md) — `rigg new
   pipeline`, managed identity, `rigg auth doctor`, a Foundry agent grounded
   on a knowledge base.
3. [Add an environment and promote](tutorials/03-add-an-environment-and-promote.md)
   — `rigg env add`, binding questions, `rigg promote`.
4. [Push to protected production](tutorials/04-push-to-protected-production.md)
   — `policy: protected`, `strict-bindings`, CI with a service principal.

## Reference

| Page | Covers |
|---|---|
| [rigg.yaml](reference/rigg-yaml.md) | The workspace file: environments, targets, policy, dependency bindings |
| [project.yaml](reference/project-yaml.md) | The project manifest, directory-is-membership, exclusive ownership |
| [Resource files](reference/resource-files.md) | The 12 resource kinds on disk: paths, naming, what push strips, infrastructure fields |
| [Annotations](reference/annotations.md) | The `x-rigg-*` keys: syntax, where each is valid, how validate/promote/push treat them |
| [APIs](reference/apis.md) | `apis/<name>.json`, the OpenAPI contract rigg validates for a WebApiSkill |
| [State](reference/state.md) | `.rigg/`: baselines, the bindings cache, what is safe to delete, `.gitignore` |
| [CLI reference](reference/cli.md) | Every command, argument and option — generated from the binary |
| [Environment variables](reference/environment-variables.md) | Every `RIGG_*` and `AZURE_*` variable rigg reads |
| [Exit codes and questions](reference/exit-codes-and-questions.md) | Exit codes 0–6, the `needs-input` protocol, every question id |

## Keeping the docs honest

Three artefacts are generated from the binary and guarded by
`cargo test -p rigg --test docs_guards`:

```bash
rigg dev cli-reference > docs/reference/cli.md
rigg dev infra-table
rigg mcp tools --markdown
```

The last two print a Markdown region that replaces the text between the
`<!-- generated:… -->` markers in `docs/reference/resource-files.md` and
`MCP.md`. Never hand-edit inside those markers.

`rigg dev docs-check --root .` parses every `rigg …` line in every code
fence, resolves every relative link and anchor, and asserts that every
environment variable and question-id prefix the code uses is documented.

## See also

- [`README.md`](../README.md) — what rigg is, install, 60-second quick start.
- [`CONCEPTS.md`](../CONCEPTS.md) — the model, also `rigg concepts`.
- [`MCP.md`](../MCP.md) — the MCP server and its tools.
- [`CHANGELOG.md`](../CHANGELOG.md) — what changed, per release.
