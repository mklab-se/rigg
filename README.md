<p align="center">
  <img src="https://raw.githubusercontent.com/mklab-se/rigg/main/media/rigg-horizontal.png" alt="rigg" width="600">
</p>

<h1 align="center">rigg</h1>

<p align="center"><em>Previously known as <strong>hoist</strong>.</em></p>

<p align="center">
  Configuration-as-code for <a href="https://learn.microsoft.com/en-us/azure/search/">Azure AI Search</a> and <a href="https://learn.microsoft.com/en-us/azure/ai-services/agents/">Microsoft Foundry</a>.<br>
  Version control your entire Agentic RAG stack — and give AI tools like Claude Code and Copilot the context to help you build it.
</p>

<p align="center">
  <a href="https://github.com/mklab-se/rigg/actions/workflows/ci.yml"><img src="https://github.com/mklab-se/rigg/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/rigg"><img src="https://img.shields.io/crates/v/rigg.svg" alt="crates.io"></a>
  <a href="https://github.com/mklab-se/rigg/releases/latest"><img src="https://img.shields.io/github/v/release/mklab-se/rigg" alt="GitHub Release"></a>
  <a href="https://github.com/mklab-se/homebrew-tap/blob/main/Formula/rigg.rb"><img src="https://img.shields.io/badge/dynamic/regex?url=https%3A%2F%2Fraw.githubusercontent.com%2Fmklab-se%2Fhomebrew-tap%2Fmain%2FFormula%2Frigg.rb&search=%5Cd%2B%5C.%5Cd%2B%5C.%5Cd%2B&label=homebrew&prefix=v&color=orange" alt="Homebrew"></a>
  <a href="https://github.com/mklab-se/rigg/blob/main/LICENSE.md"><img src="https://img.shields.io/crates/l/rigg.svg" alt="License"></a>
</p>

## The Problem

An Agentic RAG system in Azure spans two services. **Azure AI Search** does
retrieval — indexes, skillsets, indexers, knowledge bases. **Microsoft
Foundry** holds the agent layer — agent definitions, instructions, tools,
model deployments. Agents query knowledge bases, which route to knowledge
sources, which search indexes built from your data.

None of that configuration is managed by traditional IaC. ARM, Bicep and
Terraform provision the *services*. The configuration *inside* them — index
schemas, skillset pipelines, agent instructions, retrieval rules — lives in
REST APIs and portal blades. Which means:

- **No change history.** Azure does not record who changed an index schema
  or an agent instruction, so a regression has no diff to look at.
- **Portal drift.** Ad-hoc changes are frictionless, and configurations
  silently diverge from what anyone remembers deploying.
- **No review.** Agent instructions and scoring profiles go live unreviewed,
  though they shape every answer your system gives.
- **No pipeline.** Nothing to validate in a pull request, deploy on merge,
  or check for drift on a schedule.
- **Manual promotion.** Moving dev → staging → prod means hand-exporting
  JSON across two services and re-pointing every cross-resource reference.
- **Nothing for your AI tools to read.** Ask Claude Code to help optimise
  your retrieval pipeline and it cannot see any of it.

## What Rigg Does

**Files, not portals.** `rigg` pulls resource definitions from Azure AI
Search and Microsoft Foundry into local files, versions them in Git, and
pushes changes back. A **workspace** (`rigg.yaml`) holds your environments; a
**project** is the group of resources you pull, push, review and deploy as
one unit. Every resource belongs to exactly one project, which is what keeps
sync unambiguous.

**What that buys you:** Git history and code review over the whole stack,
semantic drift detection against both services, environment promotion that
*translates* infrastructure references rather than copying them, and CI/CD
with OIDC and no stored secrets.

**Identity-first authentication.** No file rigg writes ever contains a
credential. `rigg auth doctor` derives the role assignments your files
require, and can create them for you.

**A way in for your AI tools.** `rigg describe` returns the full dependency
graph in one call, and a built-in [MCP server](MCP.md) lets Claude Code,
Copilot, Cursor and others pull, push, diff and explore through structured
tool calls.

Use rigg for **Azure AI Search alone**, **Microsoft Foundry alone**, or both.
See [docs/how-rigg-works.md](docs/how-rigg-works.md) for the mechanism.

## Install

```bash
cargo install rigg
```

macOS, via Homebrew:

```bash
brew install mklab-se/tap/rigg
```

See [INSTALL.md](INSTALL.md) for pre-built binaries and shell completions.

## Quick Start

1. Point rigg at your Azure services (discovered via the Azure CLI).

   ```bash
   rigg init .
   ```

2. Group what you manage into a project.

   ```bash
   rigg new project docs-rag
   ```

3. Adopt what already exists in Azure.

   ```bash
   rigg adopt docs-rag all
   ```

4. Review the plan before anything is written.

   ```bash
   rigg push docs-rag --dry-run
   ```

**Then apply it.** `validate` checks the files on their own — structure,
ownership, references, no secrets — before the push writes anything.

```bash
rigg validate docs-rag
rigg push docs-rag
```

**Starting from nothing?** Scaffold a pipeline instead of step 3.

```bash
rigg new pipeline docs -p docs-rag --type azureblob
```

**Connect your AI tool** — optional, but recommended.

```bash
rigg mcp install claude-code    # or vs-code
```

## Documentation

**Start here:** [`rigg concepts`](CONCEPTS.md) for the mental model, then
tutorial 1.

| Tutorial | What it covers |
|---|---|
| [1 — Put an existing Azure solution under version control](docs/tutorials/01-pull-an-existing-solution.md) | `init`, `adopt`, bindings, the first commit, a delete/push round trip |
| [2 — Build from scratch](docs/tutorials/02-build-from-scratch.md) | blob → index → indexer → knowledge base → Foundry agent, with `auth doctor --fix` |
| [3 — Add an environment and promote](docs/tutorials/03-add-an-environment-and-promote.md) | `env add --like`, `promote` as translation, the binding questions |
| [4 — Push to protected production](docs/tutorials/04-push-to-protected-production.md) | `protected`/`strict-bindings`, `--confirm-env`, `ci init`, the agent gate |

| Reference | What it answers |
|---|---|
| [docs/README.md](docs/README.md) | The index: which page answers what |
| [CLI reference](docs/reference/cli.md) | Every command, argument and flag (generated from the binary) |
| [rigg.yaml](docs/reference/rigg-yaml.md) · [project.yaml](docs/reference/project-yaml.md) | Every workspace and project key |
| [Resource files](docs/reference/resource-files.md) · [Annotations](docs/reference/annotations.md) · [APIs](docs/reference/apis.md) | The 12 resource kinds, `x-rigg-*`, the WebApiSkill contract |
| [State](docs/reference/state.md) · [Environment variables](docs/reference/environment-variables.md) | `.rigg/`, and every `RIGG_*`/`AZURE_*` variable |
| [Exit codes and questions](docs/reference/exit-codes-and-questions.md) | Exit codes, the `needs-input` protocol, every question id |

Also worth reading:

- [CONCEPTS.md](CONCEPTS.md) — the model, including
  [how rigg handles authentication](CONCEPTS.md#how-rigg-handles-authentication).
- [how-rigg-works.md](docs/how-rigg-works.md) — sync classes, bindings, the
  identity graph, promotion and the question protocol.
- [MCP.md](MCP.md) — the MCP server and its 14 tools.
- [SKILLS.md](SKILLS.md) — agent skills.
- [samples/](samples/) — a runnable workspace with two projects.

## Exit Codes

Standardized for scripting and CI. `--non-interactive` guarantees rigg never
blocks on a prompt.

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Error |
| 2 | Usage error |
| 3 | Validation failed |
| 4 | Auth / permission denied |
| 5 | Drift or conflict detected |
| 6 | Needs input |

Exit 6 means a guided flow needs an answer it cannot prompt for. Instead of
failing blind, rigg prints a `needs-input` JSON document with the questions,
their ids, prompts and candidates. Answer with `--answer <id>=<value>`
(repeatable) or `--answers-file <path>` and re-run; answered questions are
never asked again.

## License

MIT — see [LICENSE.md](LICENSE.md).
