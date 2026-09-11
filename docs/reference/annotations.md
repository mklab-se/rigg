# Annotations (`x-rigg-*`)

Resource files sometimes need to say something to rigg that Azure has no
field for: which OpenAPI contract a custom skill implements, where its
function key comes from, which sibling resource a Foundry agent is grounded
on, which fields `rigg promote` must leave alone. Those statements are
**annotations** — JSON keys beginning `x-rigg-`, written inside the resource
file next to the Azure fields they qualify.

Four have meaning to rigg today:

| Annotation | Valid on | Value | Purpose |
|---|---|---|---|
| [`x-rigg-api`](#x-rigg-api) | a `WebApiSkill` in a skillset (recognised anywhere in any file) | spec name | Links the skill to `apis/<name>.json` |
| [`x-rigg-auth`](#x-rigg-auth) | a skill in a skillset's `skills[]` | `function-key` or `key-vault:<secret>@<binding>` | Says where the function key comes from, without storing it |
| [`x-rigg-pin`](#x-rigg-pin) | top level of any resource file | array of dot-paths | Fields `rigg promote` must keep at this environment's value |
| [`x-rigg-ref`](#x-rigg-ref) | any object, at any depth | `<kind-dir>/<name>` | A cross-service reference to another rigg-managed resource |

## Contents

- [The rules every annotation obeys](#the-rules-every-annotation-obeys)
- [Complete example](#complete-example)
- [`x-rigg-api`](#x-rigg-api) — the OpenAPI contract a Web API skill implements
- [`x-rigg-auth`](#x-rigg-auth) — where a function key comes from
- [`x-rigg-pin`](#x-rigg-pin) — fields promote must not overwrite
- [`x-rigg-ref`](#x-rigg-ref) — a reference Azure has no field for
- [Common mistakes](#common-mistakes)

## The rules every annotation obeys

**Kept on disk, stripped before the wire.** `x-rigg-*` keys live in your
files and in Git. Every PUT and POST body is normalized first, and that
normalization removes every key beginning `x-rigg-` at any depth — Azure
never sees one.

**They survive the push write-back.** After a successful push rigg GETs the
document back and writes it to disk (this is what kills false-positive
drift). The server's copy has no annotations, so the ones from the file being
replaced are carried back over: top-level keys by name, and keys inside array
elements by matching the element's `name` (or `type`) — which is how a
`skills[]` annotation stays attached to its own skill.

**They are excluded from every comparison.** `rigg status` and `rigg diff`
compare push-normalized documents, so adding, changing or removing an
annotation never shows as drift against Azure. It is a local edit, visible in
Git.

**Unknown `x-rigg-*` keys are inert.** A key rigg does not recognise is kept
on disk, carried over, and stripped on push like the others — it has no
effect. Use one as a private note if you like; do not expect rigg to
act on it.

## Complete example

```json
{
  "name": "contoso-enrich",
  "x-rigg-pin": ["skills[].uri"],
  "skills": [
    {
      "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
      "name": "summarize",
      "uri": "https://contoso-enrich-fn.azurewebsites.net/api/enrich",
      "httpMethod": "POST",
      "timeout": "PT30S",
      "x-rigg-api": "contoso-enrich",
      "x-rigg-auth": "key-vault:enrich-fn-key@secrets",
      "inputs": [{ "name": "text", "source": "/document/content" }],
      "outputs": [{ "name": "summary" }]
    }
  ]
}
```

And on a Foundry agent, grounding a tool on a knowledge base by name rather
than by URL:

```json
{
  "name": "contoso-assistant",
  "model": "gpt-5.2-chat",
  "instructions": { "$file": "contoso-assistant.instructions.md" },
  "tools": [
    {
      "type": "mcp",
      "server_label": "contoso_kb",
      "x-rigg-ref": "knowledge-bases/contoso-kb"
    }
  ]
}
```

## `x-rigg-api`

```json
"x-rigg-api": "contoso-enrich"
```

| | |
|---|---|
| **Type** | string — the stem of a file in `apis/` |
| **Valid on** | a `WebApiSkill` object inside a skillset's `skills[]` |
| **Required** | no |
| **Default** | none — a `WebApiSkill` without it is not contract-checked |

rigg also recognises the key anywhere else in any resource file, and requires
the spec to exist there too.

The value names `apis/<value>.json`, an OpenAPI 3.x document describing the
HTTP API the skill calls. See [APIs](apis.md) for the contract itself.

**`rigg validate`** requires the spec to exist and parse, then checks the
skill against it: the skill's `uri` path must match one of the spec's paths,
and — when the spec's `values[].data` schemas are closed
(`additionalProperties: false`) — every `inputs[].name` and `outputs[].name`
must be a property the schema declares. The three failures look like this:

```text
✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] x-rigg-api 'contoso-enrich' has no spec at apis/contoso-enrich.json (create with `rigg new api contoso-enrich`)
✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] WebApiSkill uri path '/api/summarize' does not match any path in apis/contoso-enrich.json (/api/enrich)
✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] skill inputs 'body' is not in apis/contoso-enrich.json's data schema (text, language)
```

**`rigg describe`** lists every annotated skill under "APIs to implement
(specs in apis/)", with the spec path and the resource that uses it — the
hand-off to whoever writes the function.

**`rigg promote`** treats it like any other local annotation: it belongs to
the source environment's file and does not cross into the target. The
target's own file keeps its own `x-rigg-api`. `apis/` itself is shared by the
whole workspace and is not per-environment.

**`rigg push`** strips it, like every `x-rigg-*` key. It never reaches Azure.

## `x-rigg-auth`

Either the key comes from the function app itself:

```json
"x-rigg-auth": "function-key"
```

Or it comes from a key vault:

```json
"x-rigg-auth": "key-vault:enrich-fn-key@secrets"
```

| | |
|---|---|
| **Type** | string, one of two forms |
| **Valid on** | an element of a skillset's `skills[]` |
| **Required** | no |
| **Default** | none — a skill without it is expected to be keyless |

A skill with no carrier authenticates with Entra ID / Easy Auth.

The Azure AI Search service must authenticate to the function a
`WebApiSkill` calls. The preferred answer is no key at all: enable Microsoft
Entra authentication on the function app (`rigg auth easy-auth <binding>`)
and let the search service's identity present a token. When the function
still wants a key, `x-rigg-auth` records **where the key comes from** so the
key itself never has to sit in a file.

| Value | Key source | Resolved at push by |
|---|---|---|
| `function-key` | the function app itself | an ARM `listkeys` call against the site named by the skill's `uri` |
| `key-vault:<secret>@<binding>` | a key vault | reading `<secret>` from the vault named by the `key-vault` dependency binding `<binding>` |

The secret name is everything up to the **last** `@`, so a vault binding name
is never ambiguous.

**`rigg push`** fetches the key and places it in the outgoing body only —
either as the `code=` query parameter of the skill's `uri` or as an
`x-functions-key` HTTP header, whichever slot the skill already uses. The
annotation is then stripped with the other `x-rigg-*` keys, the PUT goes out,
and before anything is written back to disk the local placeholders are
restored.

> [!NOTE]
> The key exists in memory, in one request body, and nowhere else: it is
> never written to disk, printed, or traced — not even in an error, which
> names the secret and the vault but never the value.

**`rigg validate`** checks the form, and that a key-vault carrier names a
real `key-vault` binding:

```text
✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] unknown "x-rigg-auth" value 'vault:enrich-fn-key' — expected 'function-key' or 'key-vault:<secret-name>@<key-vault binding>'
✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] "x-rigg-auth": 'key-vault:enrich-fn-key@secrets' names no dependency 'secrets' — declare it with `rigg env bind <env> secrets key-vault:<vault-name>`
✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] "x-rigg-auth": 'key-vault:enrich-fn-key@docs-storage' names binding 'docs-storage', which is not a key-vault dependency
```

Push refuses the same cases rather than pushing an unauthorized skill:

```text
Error: `x-rigg-auth: key-vault:…@secrets` names no dependency in environment 'dev' — declare it: `rigg env bind dev secrets key-vault:<vault-name>`
```

**`rigg promote` never carries it across.** A carrier authorizes exactly one
environment's function app; copying it into another environment would either
leak a key across a trust boundary or point at a vault that is not there.

So promote strips the source's `x-rigg-auth` and re-applies the *target's*
own carriers, matching the target's skill to the merged skill by `name`, then
by (already translated) `uri`, and only then — when both skill lists are the
same length — by position. A skill that matches by none of those keeps no
carrier at all, rather than inheriting one it does not own.

**`rigg auth doctor --fix`** offers to set the annotation for you when it
finds a skill whose key was lost to Azure's redaction (`code=<redacted>` in
the URI with no other authorization), and `rigg auth easy-auth` removes the
key path entirely by wiring Entra authentication instead.

## `x-rigg-pin`

```json
"x-rigg-pin": ["sku.capacity", "skills[].uri"]
```

| | |
|---|---|
| **Type** | array of strings — registry dot-paths |
| **Valid on** | the top level of any resource file |
| **Required** | no |
| **Default** | none — a path is protected only if this list names it |

There are no per-kind default pins.

`x-rigg-pin` lives in the **target** environment's file and answers: "when
something is promoted onto this file, which of my current values must
survive?"

**Path syntax is the registry's:** dot-separated keys, with `[]` after a key
to descend into each element of an array — `sku.capacity`, `skills[].uri`,
`vectorSearch.vectorizers[].azureOpenAIParameters.resourceUri`. A path that
neither document has is a silent no-op, so check the path against the file
you mean to protect: a model deployment's capacity is `sku.capacity`, not
`properties.capacity`.

**A `[]` segment pairs the target's array with the promoted one by
position**, not by name: element 0 keeps element 0's pinned value, element 1
keeps element 1's. When the target's array is longer, its extra elements are
appended to the promoted document wholesale — a tool only prod has survives
the promote — and when the promoted array is longer, its extra elements are
left alone. Reordering an array in one environment therefore changes what an
array pin protects.

`rigg promote <project> --from dev --to prod` builds each target document by
taking the source's shape, translating references and infrastructure values
into the target's world, then restoring from the target: its `name`
(unconditionally — a promoted document is never renamed to the source's
name) and every path this list asks for. `name` is the *only* unconditional
restore; everything else the target must keep has to be listed here
explicitly, or the source's value replaces it. The list itself is restored
too, so it does not evaporate on the first promote.

Use it for values that are legitimately different in this environment and
that promote would otherwise overwrite — a production deployment's
`sku.capacity`, a hand-tuned scoring profile, a URL rigg has no binding for.

> [!WARNING]
> Nothing is pinned for you: an unlisted path is promoted over, and a
> capacity that goes *down* is not even flagged (the `promote.deployment.*`
> question only fires on an increase).

`x-rigg-pin` in the **source** file does nothing to that promote (it is
stripped along with the source's other annotations), and it never reaches
Azure.

## `x-rigg-ref`

```json
"x-rigg-ref": "knowledge-bases/contoso-kb"
```

| | |
|---|---|
| **Type** | string of the form `<kind-dir>/<name>` |
| **Valid on** | any JSON object, at any depth |
| **Required** | no |
| **Default** | none |

`<kind-dir>` is the on-disk directory name of the target kind
(`indexes`, `skillsets`, `knowledge-bases`, `deployments`, `connections`, …
— the full list is in [Resource files](resource-files.md)); `<name>` is the
referenced resource's physical name.

Most references between resources have an Azure field to live in
(`dataSourceName`, `targetIndexName`, `knowledgeSources[].name`), and rigg
reads those straight from the registry. `x-rigg-ref` exists for the
references that have no such field — above all a Foundry agent grounded on an
Azure AI Search knowledge base, where the Azure field is a fully-qualified
MCP URL containing the search service name and an api-version, i.e. exactly
the things that differ per environment.

It does three things.

**It creates a dependency edge.** `rigg push` orders resources so that
everything a document references is created first, and `rigg delete` reverses
that order. An `x-rigg-ref` counts as a reference for both. `rigg describe`
draws the same edge.

**It is resolved into a URL at push time — for knowledge bases.** When the
annotation names `knowledge-bases/<kb>`, push computes that knowledge base's
MCP endpoint for the environment being pushed to and writes it into the
object's `server_url`, `url` or `endpoint` field (whichever the object
already has, defaulting to `server_url`), immediately before the annotations
are stripped:

```text
https://contoso-search.search.windows.net/knowledgebases/contoso-kb/mcp?api-version=2026-08-01-preview
```

The annotation is authoritative: the URL is recomputed on every push, so one
agent file works unchanged in every environment. Annotations naming other
kinds contribute the dependency edge only.

**It follows a rename across environments.** When the referenced sibling is
physically named differently in the target environment, `rigg promote`
rewrites the annotation's value to the target's name — the same way it
rewrites a registry reference field.

**`rigg validate`** checks the shape:

```text
✗ [projects/contoso-assistant/envs/dev/foundry/agents/contoso-assistant.json] x-rigg-ref 'contoso-kb' is not of the form <kind-dir>/<name>
```

A reference — annotated or not — to a resource that is not in the workspace
is a warning by default, because it may legitimately be a pre-existing Azure
resource. `rigg validate --strict` makes it an error.

## Common mistakes

**Putting `x-rigg-auth` on the skillset instead of the skill.** It qualifies
one skill's endpoint; at the top level it is an inert unknown annotation and
push will send an unauthorized skill.

**Writing an `x-rigg-ref` with the Azure kind name.** The first segment is
the *directory* name: `knowledge-bases`, not `knowledgeBases`; `data-sources`,
not `datasources`. Anything else fails validation.

**Expecting `x-rigg-pin` in the source environment to protect the target.**
Pins are declared where they apply — in the file being promoted *onto*.

**Expecting an annotation to show up in `rigg diff`.** It never will;
annotations are stripped before comparison. Review them in Git.

**Storing the key instead of naming its source.** A literal key in `uri` or
in an `x-functions-key` header is rejected by `rigg validate`; that is what
`x-rigg-auth` is for.

## See also

- [Resource files](resource-files.md) — the kind directories, and the infrastructure fields promote translates.
- [APIs](apis.md) — the `apis/<name>.json` contract `x-rigg-api` points at.
- [rigg.yaml § Dependencies](rigg-yaml.md#dependencies) — the `key-vault` binding `x-rigg-auth` names.
- [`CONCEPTS.md`](../../CONCEPTS.md) — logical identity vs. physical name, promoting between environments, when Azure still wants a key.
- [CLI reference](cli.md#rigg-auth-easy-auth) — `rigg auth easy-auth`, `rigg auth doctor`.
