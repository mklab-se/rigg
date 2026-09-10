# `apis/<name>.json` — custom skill contracts

A custom **Web API skill** lets an Azure AI Search skillset call HTTP code you
wrote — typically an Azure Function — in the middle of an indexer's enrichment
pipeline. rigg keeps the contract for that call in the workspace, as an
OpenAPI document in `apis/`, so the skillset and the function can be checked
against each other before anything is pushed and so whoever implements the
function has a spec to implement rather than a screenshot to copy.

`apis/` is workspace-wide, not per project and not per environment: one
contract describes the API, while each environment's skillset points at its
own deployment of it.

```
apis/contoso-enrich.json          # the contract
projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json
projects/contoso-docs/envs/prod/search/skillsets/contoso-enrich.json
```

A skillset opts in with [`x-rigg-api`](annotations.md#x-rigg-api):

```json
{
  "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
  "name": "summarize",
  "uri": "https://contoso-enrich-fn.azurewebsites.net/api/enrich",
  "x-rigg-api": "contoso-enrich"
}
```

## Creating one

```bash
rigg new api contoso-enrich
```

writes `apis/contoso-enrich.json` — a complete, valid starting point you edit
in place. Its two `data` schemas are empty and marked
`"additionalProperties": true`, so the scaffold is an **open** contract:
nothing is name-checked until you fill the schemas in (see
[openness](#what-rigg-reads) below).

## Complete example

This is the scaffold, with the `data` objects filled in for a summarizing
skill. `values[]`, `recordId`, `data`, `errors` and `warnings` are Azure's
Web API skill envelope, not rigg's invention; the parts you change are the
two `data` schemas.

```json
{
  "openapi": "3.1.0",
  "info": {
    "title": "contoso-enrich",
    "version": "1.0.0",
    "description": "Custom Web API skill contract. Implement this API (e.g. as an Azure Function) and point a skillset WebApiSkill at it with \"x-rigg-api\": \"contoso-enrich\"."
  },
  "paths": {
    "/api/enrich": {
      "post": {
        "operationId": "contoso-enrich",
        "summary": "Enrich a batch of documents",
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/EnrichmentRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Enriched values",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/EnrichmentResponse" }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "EnrichmentRequest": {
        "type": "object",
        "required": ["values"],
        "properties": {
          "values": {
            "type": "array",
            "items": {
              "type": "object",
              "required": ["recordId", "data"],
              "properties": {
                "recordId": { "type": "string" },
                "data": {
                  "type": "object",
                  "properties": {
                    "text": { "type": "string" },
                    "language": { "type": "string" }
                  },
                  "additionalProperties": false
                }
              }
            }
          }
        }
      },
      "EnrichmentResponse": {
        "type": "object",
        "required": ["values"],
        "properties": {
          "values": {
            "type": "array",
            "items": {
              "type": "object",
              "required": ["recordId", "data"],
              "properties": {
                "recordId": { "type": "string" },
                "data": {
                  "type": "object",
                  "properties": {
                    "summary": { "type": "string" }
                  },
                  "additionalProperties": false
                },
                "errors": { "type": "array", "items": { "type": "object" } },
                "warnings": { "type": "array", "items": { "type": "object" } }
              }
            }
          }
        }
      }
    }
  }
}
```

## What rigg reads

rigg does not validate the full OpenAPI grammar. It reads exactly the four
things a skillset needs, from the **first** path item that has a `post`
operation. Local `$ref`s (`#/components/…`) are followed.

| Key | Type | Required | Default | Meaning to rigg |
|---|---|---|---|---|
| `openapi` | string | **yes** | — | Must be present. Its absence is what distinguishes an OpenAPI document from any other JSON. |
| `paths` | object | **yes** | — | Its keys are the path templates a skill's `uri` may end with. |
| `paths.<path>.post.requestBody.content.application/json.schema` | schema | no | — | The request envelope. rigg reads `properties.values.items.properties.data`. |
| `paths.<path>.post.responses.200.content.application/json.schema` | schema | no | — | The response envelope, read the same way. |
| `…data.properties` | object | no | `{}` | The input (request) / output (response) field names available to the skill. |
| `…data.additionalProperties` | bool | no | closed when `properties` are declared | A `data` schema is **closed** when `additionalProperties: false`, and also when the key is simply absent and `properties` is non-empty — which is how most schemas are written. It is **open** when `additionalProperties` is present with any other value, when `properties` is empty, or when the `data` schema is missing entirely. |

Everything else in the document — `info`, `servers`, `security`, other
operations, other status codes, descriptions, examples — rigg reads past. Put
whatever your API tooling needs there; it is your document.

**Openness is one flag for the whole contract, not one per schema.** rigg
combines the request and response schemas into a single open/closed verdict:
the contract is closed only when it declares a **request** schema that is
closed *and* any response schema it declares is closed too. Leaving the
response schema open (or declaring only a response schema and no request
schema) opens the whole contract, and the `inputs` check goes with it — the
name check in step 4 below is all-or-nothing across `inputs` and `outputs`.

Because the closed state is the default for any schema that declares
`properties`, writing a `data` schema and forgetting `additionalProperties`
turns the name check **on**, not off. That is usually what you want: it is
what turns a typo in a skillset's `outputs` into a validation error instead of
an empty enriched field discovered three indexer runs later. To opt out while
the field names are still moving, set `additionalProperties: true` on every
`data` schema in the document; to lock the contract down, spell out
`additionalProperties: false` on each of them.

## What `rigg validate` checks

For every `WebApiSkill` carrying `x-rigg-api`:

1. **The spec exists.**

   ```
   ✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] x-rigg-api 'contoso-enrich' has no spec at apis/contoso-enrich.json (create with `rigg new api contoso-enrich`)
   ```

2. **The spec parses as OpenAPI.**

   ```
   ✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] apis/contoso-enrich.json: missing `openapi` version field
   ✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] apis/contoso-enrich.json: missing `paths`
   ```

3. **The skill's URI path matches a path in the spec** (compared as a
   suffix, so the function app's host and route prefix are irrelevant).

   ```
   ✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] WebApiSkill uri path '/api/summarize' does not match any path in apis/contoso-enrich.json (/api/enrich)
   ```

4. **Closed contracts only:** every `inputs[].name` is a request `data`
   property, and every `outputs[].name` is a response `data` property. Both
   loops are gated on the one contract-wide closed verdict above — if the
   contract is open, neither runs.

   ```
   ✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] skill inputs 'body' is not in apis/contoso-enrich.json's data schema (language, text)
   ✗ [projects/contoso-docs/envs/dev/search/skillsets/contoso-enrich.json] skill outputs 'sumary' is not in apis/contoso-enrich.json's data schema (summary)
   ```

An `x-rigg-api` key outside a skillset still requires the spec to exist; the
contract checks in 3 and 4 are skillset-specific.

## Handing the work off

`rigg describe` lists every annotated skill under **APIs to implement**, with
the spec path and the resource that consumes it:

```bash
rigg describe contoso-docs
```

```
  APIs to implement (specs in apis/):
    contoso-enrich (used by skillsets/contoso-enrich)
```

The same list is in `rigg describe --output json` as `apis_to_implement`,
with `api`, `spec_path` and `consumed_by` — which is how an AI agent asked to
"implement the missing APIs" finds them.

## Authenticating the call

The contract says *what* the search service sends. It does not say how the
service is allowed to call at all — that is the function app's problem, and
rigg has an opinion.

**Preferred: no key.** Enable Microsoft Entra authentication ("Easy Auth") on
the function app and let the search service present a token:

```bash
rigg auth easy-auth enrich-fn
```

The positional argument is the `function-app` **binding name** from
`rigg.yaml`, not a site name — every scope rigg acts on is a resolved
binding. The command creates (or reuses, with `--client-id`) an Entra
application for the app, admits the identities that actually call it (each
user-assigned identity the matching skillsets declare, plus the search
service's system-assigned identity for any skill that declares none), shows
the planned `authsettingsV2` document as a diff, and — once confirmed — sets
`"authResourceId": "api://<client-id>"` on the matching Web API skills
locally. Then `rigg push`. The environment must declare a `tenant:`, because
Easy Auth's OpenID issuer is
`https://login.microsoftonline.com/<tenant>/v2.0`.

On the function side that means: accept a bearer token, issued by your
tenant, whose audience is the app's identifier URI (`api://<client-id>`), from
one of the admitted client ids. Nothing in the request body changes.

**When a key is unavoidable**, name its source with
[`x-rigg-auth`](annotations.md#x-rigg-auth) — `function-key` or
`key-vault:<secret>@<binding>` — and rigg fetches it at push time and puts it
in the outgoing body only. A literal key written into `uri` or into an
`x-functions-key` header is rejected by `rigg validate`.

## Common mistakes

**Editing the skillset's `uri` route without editing the spec.** The path
check compares suffixes; a renamed route fails validation until `paths` says
the same thing.

**A closed contract and a `WebApiSkill` that sends a context field.** Every
name the skill declares must be in the schema. Add the property, or open the
contract while the shape is still moving.

**Expecting the spec to be pushed.** `apis/` is a local contract. Azure never
sees it; only the skillset is pushed. Nothing in `apis/` is per-environment
either — a second deployment of the same API is a different `uri` in a
different environment's skillset, not a second spec.

**Pointing several skills at one spec with different routes.** That is fine —
list every route under `paths`. But rigg reads request/response schemas from
the *first* path item with a `post`, so contract checking for the other
routes falls back to the path check alone. Give a genuinely different
request shape its own spec.

## See also

- [Annotations § `x-rigg-api`](annotations.md#x-rigg-api) — the key that links a skill to a spec.
- [Annotations § `x-rigg-auth`](annotations.md#x-rigg-auth) — key carriers, when a key cannot be avoided.
- [Resource files](resource-files.md) — skillsets and the infrastructure fields in them.
- [rigg.yaml § Dependencies](rigg-yaml.md#dependencies) — the `function-app` and `key-vault` bindings used above.
- [CLI reference](cli.md#rigg-new) — `rigg new api`, `rigg describe`, `rigg auth easy-auth`.
