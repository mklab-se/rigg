# quickstart-blob

The "hello world" of rigg: index blob documents and expose them for agentic
retrieval. Every piece is an explicit file you can edit and push independently.

## Files

```
search/data-sources/quickstart-docs.json    # WHERE the data lives (blob container, identity auth)
search/indexes/quickstart-index.json        # HOW it is searchable (fields, semantic config)
search/indexers/quickstart-indexer.json     # HOW data flows from source to index
search/knowledge-sources/quickstart-ks.json # exposes the index for agentic retrieval
search/knowledge-bases/quickstart-kb.json   # what agents query (routes across knowledge sources)
```

## Step by step

1. Edit `quickstart-docs.json`: set the storage account `ResourceId=` and the
   container name. No keys — grant your search service's managed identity
   **Storage Blob Data Reader** on the account (`rigg auth doctor --fix`).
2. Shape `quickstart-index.json` to your documents.
3. `rigg push quickstart-blob` — resources are created in dependency order.
4. Run the indexer once from the portal (or wait for its schedule), then
   verify: `az search index show-statistics ...` or the portal's search explorer.
5. Agents can now ground on `quickstart-kb` (see the agentic-stack sample).

Test each step with `rigg diff quickstart-blob` and `rigg status quickstart-blob`.
