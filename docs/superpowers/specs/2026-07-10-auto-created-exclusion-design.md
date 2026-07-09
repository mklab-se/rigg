# rigg — Auto-created sub-resource exclusion + managed-dependency visibility

**Date:** 2026-07-10
**Status:** Design — approved (user-directed)
**Workstream:** E (follows D; both from live Regulus testing).

## Problems

1. **Auto-created sub-resources are advertised as adoptable.** A managed-
   ingestion knowledge source (kind `azureBlob`, and future cosmos/sql kinds)
   auto-creates its backing index, indexer, data source, and skillset. Azure
   names them in the KS's `createdResources` (verified live: the `regulatory`
   KS created `regulatory-{index,indexer,datasource,skillset}`). rigg's design
   already refuses to manage these (`createdResources` is read-only:
   "Rigg never manages Azure-created sub-resources") — yet `status`, `pull`,
   and the adopt wizard list them as unmanaged, inviting exactly that mistake.
   Same defect class as SystemManaged guardrails.
2. **The dependency step hides already-managed dependencies.** When the wizard
   offers upstream dependencies, it shows only the adoptable delta — the user
   asked "did we miss the index?" because the full dependency picture
   (deployment already managed, KB already managed, …) is invisible.

## Design

### 1. Auto-created exclusion (mirror of `is_platform_managed`)

New rigg-core helper:

```
registry::auto_created_by(snapshot: &[(ResourceRef, Value)])
    -> BTreeMap<String /*resource key*/, String /*creating KS name*/>
```

For every `KnowledgeSource` doc in the snapshot, find `createdResources`
objects at any depth (the live shape nests it under `azureBlobParameters`;
`normalize` strips it recursively for the same reason) and map each value to a
resource key using the member-name → kind table: `datasource` → data-sources,
`indexer` → indexers, `skillset` → skillsets, `index` → indexes. Unknown
member names are ignored (future-proof).

Applied at the same choke points as platform-managed:

- **adopt classification**: explicitly named → reasoned skip
  `auto-created by knowledge source '<ks>' — manage it via the knowledge source`;
  swept by `all`/kind → silent skip.
- **wizard menu**: hidden.
- **dependency expansion**: never added.
- **status / pull unmanaged reporting**: hidden.

Owned-resource behavior unchanged: if a project already has a file for such a
resource, rigg does not retroactively reject it (same stance as guardrails).

### 2. Managed-dependency visibility (wizard)

`expand_deps` additionally reports the references it *encountered but skipped
because they are owned* — as `(key, owner)` pairs. The wizard's dependency
step prints them before the multi-select:

```
Already managed: deployments/gpt-5.2-chat, knowledge-bases/regulatory-kb
```

(owner == target project; references owned by *another* project print as
`managed by '<owner>'`). When the adoptable delta is empty but managed
dependencies exist, print
`All dependencies of your selection are already managed: <list>` instead of
silence — this is the "rerun on a fully-captured resource" reassurance case.
Auto-created and platform-managed refs are not listed (they are not
dependencies the user manages). Non-wizard output unchanged.

### 3. Docs

Extend the CONCEPTS.md platform-managed paragraph to also cover auto-created
sub-resources (one sentence).

## Testing

- rigg-core: `auto_created_by` finds the nested live shape; ignores unknown
  member names; non-KS docs contribute nothing.
- sync (wiremock): a KS with `createdResources` naming an existing index →
  the index is not adoptable via sweep (silent), explicit adopt skips with the
  KS-naming reason, status/pull don't count it.
- adopt.rs unit: wizard_candidates hides auto-created; expand_deps reports
  owned-encountered refs and never adds auto-created ones.
- Live acceptance: unmanaged count drops by 4 (`regulatory-*` machinery gone);
  re-selecting `agents/Regulus (managed)` prints the all-managed reassurance.

## Files touched

`crates/rigg-core/src/registry.rs`; `crates/rigg/src/commands/{adopt,status,pull}.rs`;
`CONCEPTS.md`; tests inline + `crates/rigg/tests/sync.rs`.
