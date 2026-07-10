# Getting Started: Build an Agentic RAG System

This walkthrough takes you from zero to a deployed Agentic RAG system using rigg, Azure AI Search, and Microsoft Foundry. Every resource will be an explicit file you own, review, and push — no hidden auto-provisioning.

For finished, working examples of everything built here, see the [`samples/`](samples/) workspace.

## How the Pieces Connect

```
Microsoft Foundry                    Azure AI Search
─────────────────                    ───────────────
Agent ── model ─► Deployment         Knowledge Base
  └─ tools: [mcp ────────────►]        └─ Knowledge Source
                                            └─ Index ◄── Indexer ◄── Data Source
                                                             └─ Skillset (optional)
```

- **Data source → index → indexer** — the classic Search pipeline: where the data lives, how it's searchable, and how it flows
- **Knowledge source** — exposes an existing index for agentic retrieval; it points at the index *you* define
- **Knowledge base** — what agents query; routes across knowledge sources with retrieval instructions
- **Deployment + agent** — the Foundry side: a model deployment and an agent whose MCP tool grounds on the knowledge base

rigg manages all of these as JSON files in a **project**, so you version, review, and deploy them together.

> New to the workspace/project model? See **[CONCEPTS.md](CONCEPTS.md)** (or run `rigg concepts`).

## 1. Install and Log In

```bash
brew install mklab-se/tap/rigg     # or: cargo install rigg — see INSTALL.md
az login
```

## 2. Initialize a Workspace

```bash
mkdir my-rag && cd my-rag
rigg init .
```

rigg discovers your Azure AI Search services and Foundry projects via ARM and writes `rigg.yaml` — environments and service connections. Resources live in projects, one resource tree per environment (`projects/<name>/envs/<env>/...` — see [CONCEPTS.md](CONCEPTS.md#environments)):

```bash
rigg new project docs-rag
```

## 3. Scaffold the Retrieval Pipeline

One command scaffolds the whole explicit chain — data source → index → skillset → indexer → knowledge source → knowledge base:

```bash
rigg new pipeline docs -p docs-rag --type azureblob
```

Prefer to build it up piece by piece? The individual scaffolds compose the same way:

```bash
rigg new data-source docs-ds -p docs-rag --type azureblob
rigg new index docs-index -p docs-rag
rigg new indexer docs-indexer -p docs-rag
```

Either way, you now edit the generated files:

1. **`envs/dev/search/data-sources/docs-ds.json`** — point it at your storage account. Scaffolds are identity-first: fill in the `ResourceId=` connection string and container name. Never put keys in files — `rigg validate` will reject them.
2. **`envs/dev/search/indexes/docs-index.json`** — shape the fields to your documents.
3. **`envs/dev/search/indexers/docs-indexer.json`** — the pipeline scaffold wires `dataSourceName` and `targetIndexName` for you (fill them in yourself if you scaffolded piece by piece); adjust field mappings and the schedule. Remove the skillset reference (and its file) if you don't need enrichment.

Validate as you go:

```bash
rigg validate docs-rag
```

## 4. Wire Up Identities

Identity-based access means your search service needs RBAC roles on your data — for blob, **Storage Blob Data Reader** on the storage account. Let rigg check and fix it:

```bash
rigg auth doctor --fix
```

`auth doctor` reads your workspace files, derives which identity needs which role where, and creates the missing role assignments (or prints the exact `az` commands). If your stack spans several services, prefer a shared user-assigned managed identity — role assignments survive service re-creation.

## 5. Push and Verify

```bash
rigg push docs-rag --dry-run    # review the dependency-ordered plan
rigg push docs-rag
```

rigg creates the resources in dependency order, then fetches each one back and normalizes your local files against what Azure actually stored — so `rigg diff` stays clean.

Run the indexer once from the portal (or wait for its schedule), then check:

```bash
rigg status docs-rag            # everything should be "in sync"
rigg diff docs-rag              # no differences
```

## 6. Expose the Index for Agentic Retrieval

Knowledge sources are explicit — they point at the index you just built:

```bash
rigg new knowledge-source docs-ks -p docs-rag
```

Edit `envs/dev/search/knowledge-sources/docs-ks.json` and set the index name (the `pipeline` scaffold has already done this):

```json
{
  "name": "docs-ks",
  "kind": "searchIndex",
  "searchIndexParameters": { "searchIndexName": "docs-index" }
}
```

Then the knowledge base — what agents actually query:

```bash
rigg new knowledge-base docs-kb -p docs-rag
```

Edit `envs/dev/search/knowledge-bases/docs-kb.json` to reference `docs-ks` and add retrieval instructions (e.g. "Find relevant passages; prefer exact text over summaries").

Push again — only what changed is sent:

```bash
rigg push docs-rag
```

## 7. Add the Foundry Agent

First a model deployment, then the agent:

```bash
rigg new deployment docs-model -p docs-rag
rigg new agent docs-agent -p docs-rag
```

The agent's instructions live in a Markdown sidecar — edit `envs/dev/foundry/agents/docs-agent.instructions.md`, not the JSON (the JSON references it via `{"$file": "docs-agent.instructions.md"}`).

Ground the agent on your knowledge base by giving its MCP tool an `x-rigg-ref` annotation in `envs/dev/foundry/agents/docs-agent.json`:

```json
{
  "type": "mcp",
  "server_label": "knowledge",
  "x-rigg-ref": "knowledge-bases/docs-kb",
  "server_url": ""
}
```

`x-rigg-ref` is rigg-local: at push time rigg injects the knowledge base's MCP endpoint for the *target environment*, so the same file works in dev and prod. See [`samples/projects/agentic-stack/`](samples/projects/agentic-stack/) for a complete working agent.

## 8. Deploy and Inspect

```bash
rigg validate docs-rag
rigg push docs-rag --dry-run
rigg push docs-rag
rigg auth doctor --fix          # re-check: the agent layer added new edges
rigg describe docs-rag          # the full dependency graph, agent to data source
```

Your Agentic RAG system is live, and its complete definition is a directory of reviewable files.

## Next Steps

- **Version control** — `git init && git add -A && git commit -m "docs-rag v1"` (`.rigg/` is already gitignored)
- **Environments** — `rigg env add prod` (interactive wizard, or `--search-service`/`--foundry-account`/`--foundry-project` flags), then `rigg promote docs-rag --from dev --to prod` to copy the tree, and `rigg push docs-rag --env prod` to sync it to Azure; mark `prod` `policy: { protected: true }` in `rigg.yaml` to require `--confirm-env prod` before any push/delete against it
- **CI/CD** — `rigg ci init github` scaffolds validate-on-PR, deploy-on-merge (OIDC), and nightly drift detection
- **Connect your AI tool** — `rigg mcp install claude-code` lets Claude Code see and manage the stack ([MCP.md](MCP.md))
- **AI assistance** — `rigg ai enable` turns on diff summaries, conflict merging, and `--describe` drafting
- **Existing resources?** — `rigg adopt <project> <selector>` brings selected unmanaged Azure resources into a project (a single `<kind>/<name>`, a whole `<kind>`, or `all`; add `--with-deps` to also pull a resource's dependencies); re-run with an already-managed resource to capture dependencies added later (e.g. via the portal)

See [README.md](README.md) for the full feature reference and [`samples/`](samples/) for three complete projects, including Cosmos DB and Azure SQL data source patterns.
