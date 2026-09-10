# `.rigg/` — local state

`.rigg/` is rigg's scratch directory. It holds two kinds of thing: **sync
baselines** (what each resource looked like the last time local and remote
agreed, which is what lets rigg tell "I changed it" apart from "Azure changed
it") and a **cache of resolved bindings** (what each `dependencies` entry
points at in Azure, so ARM does not have to be re-discovered on every run).

Everything in `.rigg/` is derived and machine-local. It is **not** committed:
`rigg init` adds it to `.gitignore` for you. Nothing in it is authoritative —
the resource files are the truth, Azure is the other truth, and `.rigg/` only
remembers where the two last met.

## Layout

```
.rigg/
  dev/
    bindings.json                 # resolved bindings for the `dev` environment
    contoso-docs/
      state.json                  # sync baselines for one project in one environment
    contoso-assistant/
      state.json
  prod/
    bindings.json
    contoso-docs/
      state.json
```

| Path | Written by | Holds |
|---|---|---|
| `.rigg/<env>/<project>/state.json` | every successful `pull`, `push`, `adopt` | one [baseline](#baselines-statejson) per resource |
| `.rigg/<env>/bindings.json` | `rigg env show --refresh`, and any command that resolves a binding | the [bindings cache](#bindings-cache) |
| `.rigg/<env>/<project>/replace-<name>.json` | `rigg push` during a knowledge-source replace | a [crash-recovery record](#replace-recovery-files) |

If `rigg.yaml` sets `root:`, `.rigg/` lives under that subdirectory alongside
`projects/` and `apis/`.

`rigg promote` is deliberately absent from that table. It writes the target
environment's **files** and never touches a baseline — which is exactly why a
promoted resource shows as `local-ahead` until it is pushed. It can, however,
write `rigg.yaml`: binding answers accepted during a promote are persisted to
the workspace file once the preview is confirmed (see
[rigg.yaml § Managing bindings from the CLI](rigg-yaml.md#managing-bindings-from-the-cli)).

## Baselines (`state.json`)

```json
{
  "baselines": {
    "knowledge-sources/contoso-regulatory": {
      "azureBlobParameters": {
        "connectionString": "<redacted>",
        "containerName": "regulatory",
        "ingestionParameters": {
          "contentExtractionMode": "minimal",
          "embeddingModel": {
            "azureOpenAIParameters": {
              "deploymentId": "text-embedding-3-large",
              "modelName": "text-embedding-3-large",
              "resourceUri": "https://contoso-foundry.openai.azure.com"
            },
            "kind": "azureOpenAI"
          },
          "ingestionSchedule": { "interval": "P1D" }
        }
      },
      "description": "Regulatory PDFs with structured metadata.",
      "kind": "azureBlob",
      "name": "contoso-regulatory"
    },
    "indexes/contoso-docs": "3f2a9c1de4b70856a1c9f0d2e6b48371"
  }
}
```

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `baselines` | object | no | `{}` | `<kind-dir>/<physical name>` → the baseline for that resource |
| `baselines.<key>` | object **or** string | — | — | The push-normalized document as of the last sync (write-only fields included), or (legacy) a frozen checksum of it |

The keys are the same `<kind-dir>/<name>` form used by `x-rigg-ref`,
`rigg status` and every diagnostic — `indexes/contoso-docs`,
`agents/contoso-assistant`.

**Why the document and not just a checksum.** A baseline is stored
push-normalized: volatile fields (`@odata.etag`, `created_at`, ARM
`provisioningState`, …), read-only fields and `x-rigg-*` annotations already
removed, object keys sorted, arrays of named objects sorted by name. Write-only
fields are kept: a data source's `ResourceId=…` connection string is recorded
here so that a local edit changing only that field is seen as local-ahead and
pushed (Azure never returns it). `.rigg/` therefore holds the same
identity-based reference your resource file holds — never a key — and stays
gitignored. Keeping
the document means the checksum can be *recomputed under today's
normalization rules*. When a rigg upgrade changes which fields are considered
volatile, every resource re-classifies correctly on the next run instead of
showing a workspace full of phantom drift. Older baselines stored only the
frozen checksum (the string form above); those behave as before until the
resource next syncs, at which point they are rewritten as documents.

### How a baseline is used

`rigg status` and `rigg diff` compare three things — local file, remote
document, baseline — and classify each resource:

| Class | Local vs. baseline | Remote vs. baseline | Meaning |
|---|---|---|---|
| `in-sync` | same | same | Nothing to do |
| `local-ahead` | changed | same | Your edit is waiting for `rigg push` |
| `remote-ahead` | same | changed | Azure changed; `rigg pull` to take it |
| `conflict` | changed | changed | Both moved. `rigg push`/`rigg pull` stop with exit 5 |
| `local-only` | exists | absent | New resource, or deleted in Azure |
| `remote-only` | absent | exists | Unmanaged, or deleted locally (prune candidate) |
| `untracked` | exists | exists | No baseline, and the two differ — never synced |

`untracked` is what you see before the first sync, and it is why a fresh
clone shows differences that are not really drift: the baselines are not in
Git. One `rigg pull` (or `rigg push`) re-establishes them.

### When a baseline is written

Every successful sync of a resource rewrites its baseline, from the document
the **server** returned — after a push, rigg GETs the resource back,
normalizes it and writes both the file and the baseline from that. That
round trip is what keeps server-side defaults, reordering and canonicalization
from reading as drift on the next `rigg status`. A push that fails partway
saves the baselines of everything that did succeed, so re-running resumes
rather than restarting.

## Bindings cache

`.rigg/<env>/bindings.json` is what each of the environment's bindings — the
declared `dependencies` plus the implicit `search` and `foundry` targets —
resolved to in Azure, so that ARM discovery is done once instead of on every
command.

```json
{
  "bindings": {
    "search": {
      "name": "search",
      "kind": "search",
      "physical_name": "contoso-search",
      "arm_id": "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/contoso-rg/providers/Microsoft.Search/searchServices/contoso-search",
      "subscription": "00000000-0000-0000-0000-000000000000",
      "resource_group": "contoso-rg",
      "location": "Sweden Central",
      "endpoint": "https://contoso-search.search.windows.net",
      "principal_id": null,
      "resolved_at": "2026-09-10T11:01:22.381596+00:00"
    },
    "foundry": {
      "name": "foundry",
      "kind": "foundry",
      "physical_name": "contoso-foundry",
      "arm_id": "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/contoso-rg/providers/Microsoft.CognitiveServices/accounts/contoso-foundry",
      "subscription": "00000000-0000-0000-0000-000000000000",
      "resource_group": "contoso-rg",
      "location": "swedencentral",
      "endpoint": "https://contoso-foundry.cognitiveservices.azure.com/",
      "principal_id": null,
      "resolved_at": "2026-09-10T11:01:26.366702+00:00"
    },
    "docs-storage": {
      "name": "docs-storage",
      "kind": "storage",
      "physical_name": "contosostorage",
      "arm_id": "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosostorage",
      "subscription": "00000000-0000-0000-0000-000000000000",
      "resource_group": "contoso-rg",
      "location": "swedencentral",
      "endpoint": "https://contosostorage.blob.core.windows.net/",
      "principal_id": null,
      "resolved_at": "2026-09-10T02:50:36.968946+00:00"
    }
  }
}
```

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `bindings` | object | no | `{}` | Binding name → resolution |
| `bindings.<name>.name` | string | yes | — | The binding's name in `rigg.yaml`, or `search` / `foundry` for the implicit ones |
| `bindings.<name>.kind` | string | yes | — | `storage`, `ai-services`, `function-app`, `identity`, `key-vault`, `api`, or `search` / `foundry` |
| `bindings.<name>.physical_name` | string | yes | — | The Azure resource's own name |
| `bindings.<name>.arm_id` | string or null | yes | `null` | Full ARM resource id |
| `bindings.<name>.subscription` | string or null | yes | `null` | Subscription the resource lives in |
| `bindings.<name>.resource_group` | string or null | yes | `null` | Resource group |
| `bindings.<name>.location` | string or null | yes | `null` | Azure region, as ARM reports it |
| `bindings.<name>.endpoint` | string or null | yes | `null` | Data-plane URL — the blob endpoint, the vault URI, the function app's host |
| `bindings.<name>.principal_id` | string or null | no | `null` | Object id of the resource's managed identity, where one applies |
| `bindings.<name>.resolved_at` | RFC 3339 timestamp | yes | — | When this row was captured |

The cache is a **cache**: never authoritative, safe to delete, and rebuilt on
demand. It is also what makes rigg usable without ARM read access on every
run — `rigg push` can resolve a key vault URI or a storage account's id from
here instead of calling ARM.

Refresh it explicitly after moving a resource, changing a subscription, or
renaming anything in Azure:

```bash
rigg env show dev --refresh
```

To record bindings you have not declared yet, let rigg propose them from the
infrastructure references it finds in your files:

```bash
rigg env bind dev --learn
```

That writes to `rigg.yaml` (with your answers), not to the cache; the cache
follows on the next resolution.

## Replace recovery files

Changing a knowledge source's `kind` cannot be done with a PUT, so `rigg push`
deletes and re-creates it. Knowledge bases that link to it must be unlinked
first and relinked afterwards — including knowledge bases in other projects,
whose original documents exist nowhere else while the replace is in flight.
Before unlinking, push writes `.rigg/<env>/<project>/replace-<ks>.json` with
those originals, and removes it once they are restored.

If one of these files is present, a push was interrupted. Do not delete it —
run `rigg push` again for that project and it will finish the relink and
clean up. Deleting it instead loses the linkage, and the affected knowledge
bases have to be repaired by hand.

## What is safe to delete

| File | Safe to delete? | Consequence |
|---|---|---|
| `.rigg/<env>/bindings.json` | yes, always | Bindings are re-resolved against ARM on the next command that needs one |
| `.rigg/<env>/<project>/state.json` | yes | Every resource in that project/environment goes `untracked` until the next `pull`/`push` re-establishes baselines. No data is lost, but a genuine conflict stops being detectable until then |
| `.rigg/<env>/<project>/replace-*.json` | **no** | See [above](#replace-recovery-files) |
| The whole `.rigg/` directory | yes (barring recovery files) | Both of the above, everywhere |

Deleting state is never destructive to Azure. It only removes rigg's memory
of the last agreement, so the next run has to be told what to do rather than
knowing.

## `.gitignore`

`rigg init` appends the right line for your layout — `.rigg/`, or
`<root>/.rigg/` when `rigg.yaml` sets `root:` — if it is not already there:

```gitignore
.rigg/
```

Commit everything else: `rigg.yaml`, `projects/`, `apis/` and the sidecar
Markdown files are the version-controlled definition of your stack.

Why baselines stay out of Git: they describe *one machine's* last sync with
*one* Azure state. Committing them makes every teammate's push a conflict
between their sync history and yours, and makes a stale baseline from a
merged branch look like real drift. A fresh clone showing `untracked` and
resolving on the first `rigg pull` is the intended behaviour.

## Common mistakes

**Committing `.rigg/`.** Everyone's baselines collide. Remove it from the
index (`git rm -r --cached .rigg`) and add the ignore line back.

**Reading `state.json` to find out what is in Azure.** It says what Azure
looked like at the last sync, which is precisely the thing that may have
changed. Use `rigg status` or `rigg diff`.

**Editing `state.json` to make drift go away.** The classification is a
consequence, not a setting. `rigg pull` (take Azure's version) or `rigg push`
(take yours) are the two ways to resolve a conflict; deleting the file just
turns the conflict into `untracked`.

**Renaming a project directory and wondering where the baselines went.**
They are keyed by project name under `.rigg/<env>/<project>/`. Rename that
directory too, or accept one `untracked` pass.

**Trusting a stale binding endpoint after moving a resource.** Run
`rigg env show <env> --refresh`.

## See also

- [`CONCEPTS.md`](../../CONCEPTS.md) — the sync classes in prose.
- [How rigg works](../how-rigg-works.md) — baselines, normalization and the binding layer end to end.
- [rigg.yaml § Dependencies](rigg-yaml.md#dependencies) — the bindings this cache resolves.
- [Resource files](resource-files.md) — which fields are volatile and therefore absent from a baseline.
- [Exit codes and questions](exit-codes-and-questions.md) — exit 5, the conflict exit code.
