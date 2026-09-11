# agentic-stack

The complete Agentic RAG stack as code — retrieval layer in Azure AI Search,
agent layer in Microsoft Foundry, connected by a knowledge base.

Building the same thing step by step, with expected output and the identity
wiring explained, is
[tutorial 2 — Build from scratch](../../../docs/tutorials/02-build-from-scratch.md).

## The pieces

**Search:** `docs-ds` → `docs-index` ← `docs-indexer` (+ `docs-skills`) →
`docs-ks` → `docs-kb`.

**Custom skill:** `docs-skills` contains a WebApiSkill linked to the OpenAPI
spec in `../../apis/doc-enrichment.json` via
`"x-rigg-api": "doc-enrichment"`. *You* implement that API — an Azure
Function is the usual choice. `rigg describe` lists it under "APIs to
implement", and `rigg validate` checks the skill matches the spec.

**Foundry:** a `gpt-4.1-mini` model deployment (referencing the
`default-guardrail` RAI policy) and `docs-agent`. The agent's instructions
live in `docs-agent.instructions.md` (the `$file` sidecar pattern), and its
MCP tool grounds on `docs-kb` via `"x-rigg-ref": "knowledge-bases/docs-kb"` —
rigg injects the environment-specific endpoint at push time.

## Order of operations

```bash
rigg validate agentic-stack        # includes the OpenAPI contract check
rigg auth doctor --fix             # wire up managed-identity RBAC first
rigg push agentic-stack --dry-run  # see the dependency-ordered plan
rigg push agentic-stack --verify   # push, then prove the stack actually runs
```

`--verify` runs every indexer to completion, retrieves from every knowledge
base and asks every agent one question — the same checks as
`rigg verify agentic-stack`. To exercise the pieces individually:

```bash
rigg az indexer run docs-indexer --watch
rigg az knowledge-base ask docs-kb "What does the handbook say about leave?"
rigg az agent ask docs-agent "Summarize the leave policy."
```

**Identity note:** prefer a user-assigned managed identity shared by the
pipeline when your stack spans services — role assignments survive service
re-creation. See
[CONCEPTS.md](../../../CONCEPTS.md#how-rigg-handles-authentication).
