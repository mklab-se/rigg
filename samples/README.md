# Rigg samples

One rigg **workspace**, three **projects** — copy what you need. `rigg.yaml`
uses placeholder service names; point them at your own services (or run
`rigg init` in a fresh directory and copy the project folders in).

| Project | What it shows |
|---|---|
| [`quickstart-blob`](projects/quickstart-blob/) | The minimal explicit pipeline: blob data source → index → indexer → knowledge source → knowledge base |
| [`agentic-stack`](projects/agentic-stack/) | The full showcase: skillset with a custom Web API skill (OpenAPI spec in `apis/`), knowledge base, Foundry agent + model deployment + guardrail |
| [`cosmos-sql-patterns`](projects/cosmos-sql-patterns/) | Cosmos DB and Azure SQL data sources with the change/deletion-detection policies people usually get wrong |

Because the three projects live in one workspace, this also demonstrates the
multi-project model: each project is pushed/pulled/diffed independently
(`rigg push agentic-stack`), and a resource belongs to exactly one project.

```bash
# from this directory
rigg validate                # everything is checked, including the OpenAPI contract
rigg describe                # dependency graph + the API you must implement
rigg push quickstart-blob    # after pointing rigg.yaml at your services
```
