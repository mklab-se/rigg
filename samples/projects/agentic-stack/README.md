# agentic-stack

The complete Agentic RAG stack as code — retrieval layer in Azure AI Search,
agent layer in Microsoft Foundry, connected by a knowledge base.

## The pieces

- **Search**: `docs-ds` → `docs-index` ← `docs-indexer` (+ `docs-skills`) → `docs-ks` → `docs-kb`
- **Custom skill**: `docs-skills` contains a WebApiSkill linked (via
  `"x-rigg-api": "doc-enrichment"`) to the OpenAPI spec in
  `../../apis/doc-enrichment.json`. *You* implement that API (an Azure
  Function is the usual choice) — `rigg describe` lists it under "APIs to
  implement", and `rigg validate` checks the skill matches the spec.
- **Foundry**: `gpt-4.1-mini` model deployment (referencing the
  `default-guardrail` RAI policy) and `docs-agent`, whose instructions live in
  `docs-agent.instructions.md` (the `$file` sidecar pattern) and whose MCP
  tool grounds on `docs-kb` via `"x-rigg-ref": "knowledge-bases/docs-kb"` —
  rigg injects the environment-specific endpoint at push time.

## Order of operations

```bash
rigg validate agentic-stack        # includes the OpenAPI contract check
rigg push agentic-stack --dry-run  # see the dependency-ordered plan
rigg push agentic-stack
rigg auth doctor --fix             # wire up managed-identity RBAC
```

Identity note: prefer a user-assigned managed identity shared by the pipeline
when your stack spans services — role assignments survive service re-creation.
