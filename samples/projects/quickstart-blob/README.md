# quickstart-blob

The "hello world" of rigg: index blob documents and expose them for agentic
retrieval. Every piece is an explicit file you can edit and push
independently.

For the same pipeline built from scratch against your own subscription, with
expected output at every step, follow
[tutorial 2 — Build from scratch](../../../docs/tutorials/02-build-from-scratch.md).

## Files

```
envs/demo/search/data-sources/quickstart-docs.json    # WHERE the data lives (blob container, identity auth)
envs/demo/search/indexes/quickstart-index.json        # HOW it is searchable (fields, semantic config)
envs/demo/search/indexers/quickstart-indexer.json     # HOW data flows from source to index
envs/demo/search/knowledge-sources/quickstart-ks.json # exposes the index for agentic retrieval
envs/demo/search/knowledge-bases/quickstart-kb.json   # what agents query (routes across knowledge sources)
```

## Step by step

1. Edit `quickstart-docs.json`: set the storage account `ResourceId=` and the
   container name. No keys — the search service's managed identity needs
   **Storage Blob Data Reader** on the account, which
   `rigg auth doctor --fix` grants.
2. Shape `quickstart-index.json` to your documents.
3. Push the project — resources are created in dependency order:
   `rigg push quickstart-blob`.
4. Run the indexer and watch it finish:
   `rigg az indexer run quickstart-indexer --watch`, then
   `rigg az index stats quickstart-index` for the document count.
5. Prove the whole project works: `rigg verify quickstart-blob` runs every
   indexer to completion and retrieves from every knowledge base.
6. Agents can now ground on `quickstart-kb` (see the agentic-stack sample).

Test each step with `rigg diff quickstart-blob` and
`rigg status quickstart-blob`.
