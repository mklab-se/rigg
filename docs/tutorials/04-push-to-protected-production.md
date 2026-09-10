# Tutorial 4 — Push to protected production

Production deserves a gate that a reflex cannot open. This tutorial marks an
environment protected, walks through what that changes for a human, for a CI
job and for an AI agent, and finishes with a GitHub Actions pipeline that
deploys on merge using OIDC and no stored secrets.

**Time:** about 30 minutes.

**Azure cost:** step 3 is the one step that is not a preview. It is a real
`rigg push … --verify` against production: the push itself costs nothing, but
`--verify` runs every indexer in the project to completion and asks every agent
one question, so it bills ingestion, embedding and tokens. Drop `--verify` (or
add `--dry-run`) if you are only rehearsing the gate. Everything else here —
steps 1, 2, 4, 5, 6 and 7 — reads, previews or writes files on your machine.

## Prerequisites

- A workspace with at least two environments, from
  [tutorial 3](03-add-an-environment-and-promote.md). Examples use `dev` and
  `prod`, and the project `docs-rag`.
- Production targets: a Search service and a Foundry project you are allowed
  to write to.
- **Roles you need**, on your own account: `Search Service Contributor` and
  `Azure AI User` on the prod resources. To create the CI identity in step 6
  you also need permission to register an Entra application, and
  `User Access Administrator`/`Owner` to grant it roles — those are the two
  things rigg will never do for you.
- A GitHub repository for the workspace, for step 6.

## 1. Mark the environment protected

Edit `rigg.yaml`:

```yaml
environments:
  prod:
    tenant: <tenant-id>
    subscription: <subscription-id>
    search:  { service: contoso-search-prod }
    foundry: { account: contoso-ai, project: rag-prod }
    policy:
      protected: true
      strict-bindings: true
    dependencies:
      docs-storage: { storage: contosodocsprod }
```

`rigg env add prod --protected` sets the same flag, and answering *yes* to the
"Protect this environment?" question during `rigg env add` does too. Both keys
are documented in [the policy reference](../reference/rigg-yaml.md#policy).

Two policy flags, doing different jobs:

- **`protected: true`** requires a typed confirmation before rigg mutates the
  environment. That is every command that writes to it: `push` (create/update
  and `--prune`), `delete --remote`, `verify`, `az indexer run`,
  `az indexer reset`, `auth doctor --fix`, `auth easy-auth` and
  `auth roles remove`. Each of them also takes `--confirm-env prod` so a
  script can supply the answer up front.
- **`strict-bindings: true`** turns *warnings* about infrastructure references
  into *errors*: a reference that matches no binding fails `validate` and
  `push` with exit 3. It defaults to the value of `protected`, so writing it
  out above is documentation rather than a change — set it to `false`
  explicitly if you want a protected environment that only warns.

Confirm it took:

```bash
rigg env show prod
```

<!-- verify-live -->
```text
# output
prod
  protected: true
  tenant: <tenant-id>
  subscription: <subscription-id>
  search: contoso-search-prod → https://contoso-search-prod.search.windows.net (Azure AI Search)
  foundry: contoso-ai/rag-prod → https://contoso-ai.services.ai.azure.com/api/projects/rag-prod (Microsoft Foundry)
  dependencies:
    docs-storage  storage  contosodocsprod
```

## 2. Try to push without confirming

A dry run is never gated — a preview mutates nothing:

```bash
rigg push docs-rag -e prod --dry-run
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: prod, protected)
  Search:  contoso-search-prod → https://contoso-search-prod.search.windows.net
  Foundry: contoso-ai/rag-prod → https://contoso-ai.services.ai.azure.com/api/projects/rag-prod
  update indexes/docs-index
  update agents/docs-agent
  (dry run — nothing pushed)
```

No confirmation was asked for, because nothing was going to be written.

Now attempt the real thing the way a script would — `--yes`, and
`--non-interactive` so that you see on your terminal what CI would see (a job,
a container or an MCP tool call gets this behaviour automatically, because
stdin is not a terminal):

```bash
rigg push docs-rag -e prod --yes --non-interactive
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: prod, protected)
  Search:  contoso-search-prod → https://contoso-search-prod.search.windows.net
  Foundry: contoso-ai/rag-prod → https://contoso-ai.services.ai.azure.com/api/projects/rag-prod
  update indexes/docs-index
  update agents/docs-agent
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
1 question(s) need an answer: confirm.protected.prod
Answer with --answer <id>=<value> (or --answers-file) and re-run.
```

Exit code 6, and **nothing was written**. The JSON document goes to stdout so
a script can parse it; the two-line summary goes to stderr, and only in text
mode.

That `--yes` did not open the gate is the point of the design. `--yes` clears
the routine "apply N changes?" prompt, and scripts reach for it reflexively —
so if it also cleared this gate, a protected environment would be no safer
than an ordinary one. The gate is a separate question with its own id, and
only an answer that equals the environment's name satisfies it.

On a terminal you are simply prompted to type `prod`.

## 3. Confirm, verify, and push

```bash
rigg push docs-rag -e prod --yes --confirm-env prod --verify
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: prod, protected)
  Search:  contoso-search-prod → https://contoso-search-prod.search.windows.net
  Foundry: contoso-ai/rag-prod → https://contoso-ai.services.ai.azure.com/api/projects/rag-prod
  update indexes/docs-index
  update agents/docs-agent
  ✓ indexes/docs-index
  ✓ agents/docs-agent
Verify project 'docs-rag' (env: prod)
  ✓ triggered a run of 'docs-indexer'
  … success
  ✓ indexer 'docs-indexer' — 4096 processed, 0 failed
  ✓ knowledge base 'docs-kb' retrieved
  ✓ agent 'docs-agent' replied
✓ 3 check(s) passed
```

`--confirm-env prod` is exactly `--answer confirm.protected.prod=prod`; a
*wrong* value is a usage error (exit 2) naming the environment it must match,
not a silent no-op.

`--verify` is the flag worth making habitual in production. A push proves the
definitions landed; `--verify` proves the stack still *runs* — every indexer
to completion, a retrieve from every knowledge base, one turn with every
agent — and exits 1 if anything fails. Because those runs cost real ingestion
and tokens, it is opt-in. `rigg verify docs-rag -e prod` does the same checks
on an already-pushed stack.

## 4. Review what rigg changed in your subscription

```bash
rigg auth roles list -e prod
```

<!-- verify-live -->
```text
# output
auth roles assignments described 'rigg:contoso-rag:prod:…' across 3 scope(s) (env: prod)
  • rigg:contoso-rag:prod:data-sources/docs-ds reads blobs
      role:  <role-guid>
      scope: /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocsprod

1 assignment(s) — remove them with `rigg auth roles remove`
```

Every role assignment rigg creates carries a description of the form
`rigg:<workspace>:<env>:<reason>`, and this command lists exactly those — not
the assignments your platform team made, not another environment's, not ones
granted subscription-wide. `rigg auth roles remove -e prod --confirm-env prod`
undoes precisely this list, which means adopting rigg is reversible. `list`
only reads, so it is not gated; `remove` writes, so it is — which is why it
takes the same `--confirm-env` as `push`.

## 5. Guard the boundary

Two habits make the gate hard to route around by accident.

Keep production out of your shell's default. `rigg env set-default dev` means
a forgotten `-e` targets dev, and `RIGG_ENV` in a shell profile is a reliable
way to surprise yourself. Every mutating command prints the environment and
the resolved service URL in its banner — read the `(env: prod, protected)`
line before answering a prompt.

Let `strict-bindings` do its work. In a protected environment, a file that
references infrastructure no binding covers is an error, so "we shipped a
reference to the dev storage account" fails validation instead of production.

```bash
rigg validate docs-rag -e prod --strict --show-bindings
```

<!-- verify-live -->
```text
# output
✓ all checks passed
✓ bound 'docs-storage' (storage contosodocsprod)
```

Validation reads files; it never writes, so it is not gated.

## 6. Deploy from CI, with no secrets

```bash
rigg ci init github -e prod
```

`-e prod` matters: `ci init` pins the workflows to whichever environment
resolves — flag, then `RIGG_ENV`, then the workspace default — and step 5 just
made that `dev`.

<!-- verify-live -->
```text
# output
  created .github/workflows/rigg-validate.yml
  created .github/workflows/rigg-deploy.yml
  created .github/workflows/rigg-drift.yml

✓ GitHub workflows created for environment 'prod'. To finish setup:
  1. Create an Entra app registration with federated credentials for this repo
     (workload identity federation — no client secrets):
       az ad app create --display-name rigg-deploy
       az ad app federated-credential create ... (subject: repo:<owner>/<repo>:ref:refs/heads/main)
  2. Grant it what 'prod' actually requires — from this workspace's files:
       <role-guid>  # Search Service Contributor
         /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Search/searchServices/contoso-search-prod
       <role-guid>  # Azure AI User
         /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.CognitiveServices/accounts/contoso-ai/projects/rag-prod
     …and, so a push can grant the service identities their own roles,
     Microsoft.Authorization/roleAssignments/write at each scope below
     (with the role rigg would grant there):
       <role-guid>  # Storage Blob Data Reader
         /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocsprod
     (or pre-grant them yourself once: rigg auth doctor -e prod --fix,
      and add --skip-auth-preflight to the deploy job's push)
     az role assignment create --assignee <AZURE_CLIENT_ID> --role <role-guid> --scope "<scope>"
     Verify: rigg auth doctor -e prod --principal <the CI identity's object id>
  3. Add repository variables: AZURE_CLIENT_ID, AZURE_TENANT_ID, AZURE_SUBSCRIPTION_ID.
```

That role list is not boilerplate. rigg ran the same identity graph over
*your* files and bindings and printed the roles the workflows will actually
need, at their real ARM scopes, naming each role by its definition GUID
because Microsoft is renaming the Foundry roles.

Three workflows are written:

| File | Trigger | What it runs |
|---|---|---|
| `rigg-validate.yml` | pull request | `rigg validate --strict`, then `rigg diff --all --format markdown` posted as a PR comment |
| `rigg-deploy.yml` | push to `main` | `rigg validate --strict`, then `rigg push --all --yes --confirm-env <env>` |
| `rigg-drift.yml` | nightly cron | `rigg diff --all --exit-code`; exit 5 opens or updates a drift issue |

All three log in with `azure/login@v2` using OIDC federated credentials
(`permissions: id-token: write`) — the repository holds three non-secret
*variables* and no client secret at all.

Note the deploy job's `--confirm-env`: it is a no-op for an unprotected
environment and the required consent for a protected one. Putting the
environment's name in the workflow file is the point — the confirmation is
reviewed once, in a pull request, rather than typed by whoever happened to
merge.

Before the first run, check the CI identity from your own machine:

```bash
rigg auth doctor -e prod --principal <the CI identity's object id>
```

`--principal` reports *that* object id's rights instead of yours, so you find
out about a missing role now rather than at 3 a.m.

## 7. The same gate, for an AI agent

If you have connected rigg's MCP server (`rigg mcp install claude-code`), your
assistant reaches the identical gate — it cannot push to prod because it
decided to.

Ask it to push and its first `rigg_push` call has no `force` — the arguments
are just the project and the environment:

```json
{ "project": "docs-rag", "env": "prod" }
```

The server turns a call without `force` into `--dry-run`. A preview writes
nothing, so it is never gated: the assistant gets the plan back and shows it
to you. You approve, and it calls `rigg_push` again with `force: true` — the
call that would actually write. *That* one comes back with, as its *result*
rather than an error:

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

The agent shows you that question, you say `prod`, and it calls the tool a
third time — `force: true` again, now with
`answers: {"confirm.protected.prod": "prod"}` (or the `confirm_env: "prod"`
shorthand). Preview, consent, write: the assistant can drive the loop, but the
one value that opens the gate has to come from you. See
[MCP.md](../../MCP.md#the-needs-input-loop) for the loop in full.

## What you have now

- A production environment that cannot be mutated without naming it, whether
  the caller is you, a script, a CI job or an agent.
- Strict binding checks, so an environment leak fails review instead of
  production.
- A CI pipeline that validates every pull request, deploys on merge and
  reports drift nightly — with OIDC and no stored credentials.
- A reversible footprint: `rigg auth roles list -e prod` shows every role
  assignment rigg made, and `remove` undoes exactly those.

## Clean up

Nothing to undo unless this was a rehearsal. To take the protection off, drop
`policy.protected` from `rigg.yaml`; to remove the workflows, delete the three
files in `.github/workflows/`; to withdraw rigg's role assignments, run
`rigg auth roles remove -e prod --confirm-env prod` (or drop the flag and type
`prod` at the prompt).

## Next

- [How rigg works → The question protocol](../how-rigg-works.md#the-question-protocol)
- [Exit codes and questions](../reference/exit-codes-and-questions.md) — every
  exit code, every question id prefix, the `needs-input` document.
- [MCP.md](../../MCP.md) — the 14 tools and the preview/`force` pattern.
