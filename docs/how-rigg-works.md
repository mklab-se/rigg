# How rigg works

The connective narrative: what rigg actually computes when you run `status`,
`push`, `promote` or `auth doctor`. Written for a reader who has done
[tutorial 1](tutorials/01-pull-an-existing-solution.md) and wants to know why
the commands behave the way they do.

[CONCEPTS.md](../CONCEPTS.md) is the model — workspace, project, environment,
logical vs. physical identity. This page is the mechanism. The exact flags
live in [the CLI reference](reference/cli.md).

## Contents

- [Three states, not two: sync classes and baselines](#three-states-not-two-sync-classes-and-baselines)
- [The binding layer](#the-binding-layer)
- [The identity requirement graph](#the-identity-requirement-graph)
- [Promote is translation, not copy](#promote-is-translation-not-copy)
- [The question protocol](#the-question-protocol)
- [What rigg never does](#what-rigg-never-does)

## Three states, not two: sync classes and baselines

A naive config tool compares two things: the file and the cloud. That cannot
tell "I edited this" apart from "someone edited it in the portal" — both look
like "they differ". rigg compares **three**: the local file, the live remote
document, and a **baseline** — the document as it was the last time the two
agreed.

The baseline for every resource a project owns lives in
`.rigg/<env>/<project>/state.json` (gitignored — it is a cache, not a source
of truth). Every successful `pull`, `push` and `adopt` rewrites it.

The three-way comparison yields exactly one class per resource, which is what
`rigg status` prints:

| Class | Local | Remote | What it means | `status` label |
|---|---|---|---|---|
| InSync | = baseline | = baseline | nothing to do | `in sync` |
| LocalAhead | changed | = baseline | you edited a file; push it | `local ahead (push pending)` |
| RemoteAhead | = baseline | changed | someone changed Azure; pull it | `remote ahead (pull pending)` |
| Conflict | changed | changed | both moved since the last sync | `CONFLICT` |
| LocalOnly | exists | absent | new resource, or remote-deleted | `local only (push to create)` |
| RemoteOnly | absent | exists | unmanaged, or you deleted the file | `remote only` |
| Untracked | exists | exists | no baseline, and they differ | `untracked (never synced)` |

Two design decisions make this reliable rather than noisy:

**Comparison is semantic, not textual.** Before anything is compared, a
document is normalized: the registry marks per kind which fields are
*volatile* (`@odata.etag`, timestamps) and which are *read-only* (server-set
defaults), and both are stripped. Checksums are computed over a canonical
ordering and treat an explicit `null` the same as an absent key. Azure
re-serializing your index in a different field order is not a change.

**Every push ends with a read-back.** After a successful PUT, rigg GETs the
server's copy of the document, normalizes it, and writes *that* to disk and to
the baseline. This is what kills false-positive drift: the file on disk is
always the document Azure actually holds, defaults and all, so the next
`status` says `in sync` instead of inventing a diff out of a field the service
filled in for you.

Conflicts are never resolved silently. `rigg push` refuses a conflicted
resource and exits 5; you pull, re-apply your edit, and push again.

## The binding layer

Resource files reference infrastructure that lives *outside* Search and
Foundry: a storage account inside a data source's connection string, a
function app inside a custom skill's URI, a user-assigned identity inside a
skillset, a key vault inside an encryption key. Those references are the
reason a dev file cannot simply be copied to prod.

rigg's answer is a **binding**: a name in an environment's `dependencies:` map
pointing at one physical resource, e.g. `docs-storage: { storage:
contoso-docs-dev }`. The same name in another environment is the same *role*,
played by a possibly different resource. Two implicit bindings, `search` and
`foundry`, come free with every environment.

What makes this mechanical rather than heuristic is the **InfraRef table** in
`crates/rigg-core/src/registry.rs`: per resource kind, the exact JSON paths
that carry infrastructure, the binding type each maps to, the syntactic form
to parse it out of (an ARM id embedded in a connection string, a bare
`userAssignedIdentity`, an origin URL), and the `@odata.type` it is valid for.
The table is printed as part of
[the resource-files reference](reference/resource-files.md#infrastructure-reference-fields);
nothing in rigg guesses at a reference that is not in it.

Every command that touches infrastructure walks the same table:

- `rigg validate` classifies each reference as Bound, Shared, Leak, Unbound or
  External (see [CONCEPTS](../CONCEPTS.md#validation-classes) for the
  severities; a Leak — a file pointing at *another* environment's
  infrastructure — is always an error).
- `rigg env bind <env> --learn` runs the extraction in reverse: it collects
  every reference in the environment's files, groups them by physical
  resource, and proposes a binding name per group. `adopt` and `pull` offer
  the same step after writing files.
- `rigg push` re-runs the classification over its own plan as a preflight, and
  refuses before writing anything.
- `rigg promote` uses it to re-point references at the target environment.

Resolved bindings (the ARM id an account name stands for) are cached in
[`.rigg/<env>/bindings.json`](reference/state.md#bindings-cache), refreshed by
`rigg env show --refresh`. The cache is a lookup accelerator; deleting it
costs a round trip, never correctness.

## The identity requirement graph

rigg writes no credential to any file, so every service-to-service connection
it manages is a managed identity plus a role assignment. That turns "does this
work?" into a question rigg can answer *offline*: derive, from the files and
the bindings, the set of `(principal, role, scope)` edges the environment
requires; then ask Azure which of them exist.

The graph and the principals behind it are documented in CONCEPTS —
**[How rigg handles authentication](../CONCEPTS.md#how-rigg-handles-authentication)**
— and that chapter is the reference. What matters here is where the same graph
is used, because it is one computation with four entry points:

| Command | Scope | What it does with the result |
|---|---|---|
| `rigg auth doctor` | the whole environment tree | reports every edge with its file evidence and an `az` line; `--fix` applies the ones rigg owns |
| `rigg push` (preflight) | only the resources this plan writes | refuses (exit 4) when something is missing that rigg may not grant; otherwise grants and waits for propagation |
| `rigg status --auth` | the whole environment tree | one summary line per environment |
| `rigg verify` | the live data plane | attributes an observed failure to the edge that would explain it |

The push preflight being **plan-scoped** is the point: a push that touches one
synonym map is not blocked by an unrelated storage grant a different resource
would need.

## Promote is translation, not copy

`rigg promote --from A --to B` never copies a file. For every logical resource
(correlated by file path, not by physical name) it *derives* the document B
should hold, choosing per field between five sources: infrastructure
translated through the bindings, sibling references rewritten to the target's
physical names, values kept from the target (its own `name`, its `x-rigg-pin`
paths), a Web API auth carrier re-derived from the target function app's own
Easy Auth state, and everything else taken from A.

The full rule list is in
[CONCEPTS → Promoting between environments](../CONCEPTS.md#promoting-between-environments).
Three consequences are worth stating plainly:

1. **A→B and B→A are the same operation.** There is no "deploy direction"; a
   hotfix promoted back from prod to dev uses the same command.
2. **Nothing is deleted, and target-only resources are never touched.**
3. **Anything rigg cannot decide becomes a question, not a guess** — an
   unbound reference in the source, a binding the target lacks, an external
   API URL bound in neither environment.

Which brings us to how questions work.

## The question protocol

Guided flows are not "interactive commands with a scripted fallback". There is
one protocol, and a terminal prompt is just one renderer for it.

A question has an **id**, a **kind** (`choice`, `text`, `confirm`,
`confirm-env`), a prompt, optional candidates, and an optional default. The id
is stable and namespaced, so the same question always has the same name:

| Prefix | Asked by |
|---|---|
| `confirm.protected.<env>` | the protected-environment gate on `push`, `delete --remote`, `verify`, `az indexer run/reset`, `auth doctor --fix`, `auth easy-auth`, `auth roles remove` |
| `binding.<env>.<name>` | `rigg env add --like`, and promote when the target lacks a binding |
| `env.<name>.…` | `rigg env add` (e.g. whether to protect the new environment) |
| `learn.<env>.record` | the offer to record learned bindings after `adopt`/`pull` |
| `promote.…` | promote's source-side binding, external-API and deployment questions |
| `auth.…` | `rigg auth doctor --fix` and `auth roles remove` batch confirmations |

On a terminal, questions are prompted. Everywhere else — `--non-interactive`,
`RIGG_NON_INTERACTIVE`, `--output json`, no TTY on stdin, an MCP tool call —
an unanswered question makes the command exit **6** and print one
`needs-input` document instead of failing blind:

```json
{
  "status": "needs-input",
  "command": "push",
  "context": { "project": "docs-rag", "env": "prod" },
  "questions": [
    {
      "id": "confirm.protected.prod",
      "kind": "confirm-env",
      "prompt": "Environment 'prod' is protected. Type its name to confirm push:"
    }
  ]
}
```

Answer with `--answer <id>=<value>` (repeatable) or `--answers-file <path>`
and re-run. Answers that were used are persisted where they represent a
decision (a promote binding), so the same question is not asked twice.
`--confirm-env <env>` is exactly `--answer confirm.protected.<env>=<env>`.

`--yes` is a different thing and deliberately weaker: it accepts routine
"apply N changes?" prompts. It never satisfies a `confirm-env` question,
because scripts reach for `--yes` reflexively and a protected environment that
`--yes` cleared would not be protected at all.

See [exit codes and questions](reference/exit-codes-and-questions.md#needs-input)
for the complete list.

## What rigg never does

Knowing the boundaries is half of trusting a tool with your production
environment.

- **It does not provision Azure services.** ARM, Bicep and Terraform create
  the Search service, the Foundry account, the storage account and the
  function app. rigg manages the configuration *inside* them, and reads ARM
  only to discover and verify. `rigg init` finds your services; it never
  creates one.
- **It does not put secrets in files.** No admin key, no connection-string
  password, no function key is ever written to disk — `rigg validate` rejects
  a file that contains key material, and `rigg push` strips `x-rigg-*`
  annotations before the request. Where Azure still insists on a runtime key,
  the file carries a key *source*
  ([`x-rigg-auth`](reference/annotations.md#x-rigg-auth)) and rigg fetches the
  value into the outgoing body only.
- **It does not grant you your own permissions.** `auth doctor --fix` can
  create the role assignments that let two *services* talk to each other. It
  will never grant the caller a role, because anyone who could run a push
  would then be able to escalate their own access. Missing operator rights
  end in exit 4 with the exact `az role assignment create` line to hand to
  whoever owns the subscription.
- **It does not delete remote resources implicitly.** A deleted local file is
  reported as an orphan; removing it from Azure takes `push --prune` or
  `rigg delete <project> --remote`.
- **It does not resolve conflicts for you.** Exit 5, and the decision stays
  yours.

## Where to next

- [Tutorials](tutorials/01-pull-an-existing-solution.md) — four end-to-end walkthroughs.
- [CONCEPTS.md](../CONCEPTS.md) — the model, and the authentication chapter.
- [Reference index](README.md) — every file, field, flag, variable and exit code.
