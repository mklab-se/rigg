# Exit codes and questions

rigg is meant to be scripted and to be driven by AI agents, so two things have
to be exact: **what a non-zero exit means**, and **how rigg asks for
information when there is nobody at the keyboard**. This page is the contract
for both.

## Contents

- [Exit codes](#exit-codes)
- [`needs-input`](#needs-input) — [the document](#the-document),
  [answering](#answering), [coercion](#how-answers-are-coerced),
  [the protected-environment gate](#the-protected-environment-gate)
- [Question ids](#question-ids)
- [Scripting recipes](#scripting-recipes)
- [Common mistakes](#common-mistakes)

## Exit codes

| Code | Name | Produced when |
|---|---|---|
| 0 | success | The command did what it said |
| 1 | error | Anything unclassified |
| 2 | usage error | The invocation itself is wrong |
| 3 | validation failed | `rigg validate` found problems |
| 4 | auth / permission denied | Azure returned 401 or 403, or no token could be obtained |
| 5 | drift or conflict detected | Local and remote both moved, or `diff --exit-code` found differences |
| 6 | needs input | A guided flow needs an answer and cannot prompt |

**0** includes "nothing to do" and a user declining a confirmation.

**1** covers an Azure 5xx, a network failure, malformed JSON in a resource
file, an unreadable `rigg.yaml`.

**2** covers a project name that is needed and missing, `--project` and
`--all` together, an unknown `--answer` id, an answer that does not coerce, a
`--confirm-env` that does not match, a non-interactive `push` with changes to
apply and no `--yes` (`non-interactive push requires --yes`), and a push plan
containing replaces without `--allow-replace`.

**3** is also produced when a command refuses because its input would not
validate.

**4** is also produced explicitly by flows that establish rigg cannot reach a
resource it needs.

**5** is `rigg pull` / `rigg push` stopping because local and remote both
moved since the baseline, or `rigg diff --exit-code` finding differences.

**6** puts the [`needs-input`](#needs-input) document on **stdout**.

The codes are stable and unit-tested; treat them as API.

```bash
rigg diff --all --exit-code
```

```text
0 → no differences
5 → differences found
3 → the files do not validate
4 → cannot read Azure with this identity
```

> [!NOTE]
> `rigg diff` reports drift on stdout but exits 0 **unless** `--exit-code` is
> given — the flag is what makes it a CI gate.

Four boundaries are worth spelling out.

**Declining a confirmation is success.** Answering `n` to "Apply 3 change(s)?"
prints `  aborted`; typing the wrong name at a protected-environment prompt
prints `Aborted.` — both exit 0. Nothing was asked for, nothing failed.

**Cancelling is not declining.** Esc or Ctrl-C aborts the command with an
error (exit **1**), and so does declining a prompt whose "no" leaves the
command with nothing to do (`rigg adopt`'s "Create one now?" when there is no
project yet). A *wrong pre-supplied answer* is different again — that is exit
2, because you told rigg something and it was not usable.

**401/403 from any Azure plane is 4**, not 1, even when it surfaces in the
middle of a push. That is what lets CI distinguish "grant the service
principal a role" from "retry later".

**Exit 6 is not a failure.** It is a request. See below.

## `needs-input`

Guided flows — `promote`, `env bind --learn`, `auth doctor --fix`,
`auth easy-auth`, the protected-environment gate — ask questions. At a
terminal rigg prompts. When it cannot prompt, it prints a JSON document
describing every outstanding question to **stdout** and exits 6, so the
caller can answer and re-run.

A session is non-interactive when any of these hold:

- `--non-interactive`
- `--yes`
- `--output json`
- `RIGG_NON_INTERACTIVE` is set to anything but empty / `0` / `false`
- stdin or stdout is not a terminal

### The document

Pushing a project to a protected environment with nobody at the keyboard:

```bash
rigg push contoso-docs --env prod --non-interactive
```

```json
{
  "status": "needs-input",
  "command": "push",
  "context": {
    "project": "contoso-docs",
    "env": "prod"
  },
  "questions": [
    {
      "id": "confirm.protected.prod",
      "kind": "confirm-env",
      "prompt": "Environment 'prod' is protected. Type its name to confirm push:"
    }
  ]
}
```

A different flow, a different document. `rigg promote` asks about the bindings
the target environment is missing — every open question in one document, one
entry in `questions` each:

```bash
rigg promote contoso-docs --from dev --to prod --non-interactive
```

```json
{
  "status": "needs-input",
  "command": "promote",
  "context": {
    "project": "contoso-docs",
    "from": "dev",
    "to": "prod"
  },
  "questions": [
    {
      "id": "binding.prod.docs-storage",
      "kind": "choice",
      "prompt": "'prod' has no binding 'docs-storage' (storage), used by 2 reference(s). Use:",
      "candidates": [
        { "value": "same", "label": "same as dev: contosostoragedev (shared)" },
        { "value": "contosostorageprod", "label": "contosostorageprod" },
        { "value": "skip", "label": "skip (keep the value 'dev' has)" }
      ],
      "allow_other": true
    },
    {
      "id": "binding.prod.enrich-fn",
      "kind": "choice",
      "prompt": "'prod' has no binding 'enrich-fn' (function-app), used by 1 reference(s). Use:",
      "candidates": [
        { "value": "same", "label": "same as dev: contoso-enrich-dev (shared)" },
        { "value": "contoso-enrich-prod", "label": "contoso-enrich-prod" },
        { "value": "skip", "label": "skip (keep the value 'dev' has)" }
      ],
      "allow_other": true
    }
  ]
}
```

> [!NOTE]
> These are two *different* documents, and never one. `rigg promote` has no
> protected-environment gate at all — it writes the target environment's
> files, not Azure — so a promote document never carries a
> `confirm.protected.*` question, and the gate (which builds its own asker,
> with its own `command` and `context`) never carries a binding question.
> Answer each document with the questions it actually contains.

| Key | Type | Always present | Meaning |
|---|---|---|---|
| `status` | string | yes | Always `"needs-input"` — the discriminator to key on |
| `command` | string | yes | What to re-run (`"push"`, `"promote"`, `"az indexer run"`, …) |
| `context` | object | yes | What that re-run needs: `project`, `from`, `to`, `indexer`, … |
| `questions` | array | yes | Every outstanding question, in one document |
| `questions[].id` | string | yes | The id to answer with. See [Question ids](#question-ids) |
| `questions[].kind` | string | yes | `choice`, `text`, `confirm` or `confirm-env` |
| `questions[].prompt` | string | yes | Human-readable question |
| `questions[].candidates` | array of `{value,label}` | only when non-empty | The offered values. Answer with a `value`, not a `label` |
| `questions[].allow_other` | bool | only when `true` | A value outside `candidates` is accepted |
| `questions[].default` | string | only when set | The value used if answered with nothing interactively |

**`context`** also gains `env` when the caller left it out.

In text mode a human-readable summary also goes to **stderr**:

```text
2 question(s) need an answer: binding.prod.docs-storage, binding.prod.enrich-fn
Answer with --answer <id>=<value> (or --answers-file) and re-run.
```

stdout stays pure JSON either way, which is what makes `rigg … | jq` and the
MCP server work.

### Answering

```bash
rigg promote contoso-docs --from dev --to prod \
  --answer binding.prod.docs-storage=contosostorageprod \
  --answer binding.prod.enrich-fn=contoso-enrich-prod
```

Or from a file, for anything more than a couple of answers:

```bash
rigg promote contoso-docs --from dev --to prod --answers-file answers.json
```

```json
{
  "binding.prod.docs-storage": "contosostorageprod",
  "binding.prod.enrich-fn": "contoso-enrich-prod"
}
```

`--answer` flags override the file. Both are global options, accepted by
every command. An answered question is never asked again, in either mode: an
interactive run also consumes pre-supplied answers and only prompts for
what is left.

### How answers are coerced

| Kind | Accepted | Rejected |
|---|---|---|
| `choice` | any `candidates[].value`; anything at all when `allow_other` is true | a value that is not offered, when `allow_other` is absent |
| `text` | any string | — |
| `confirm` | `yes`, `y`, `true`, `no`, `n`, `false` (case-insensitive) | anything else |
| `confirm-env` | the environment's name, exactly | anything else, including a different case |

A rejected answer is a **usage error (exit 2)**, on a terminal too — rigg
deliberately does not fall back to prompting for a value you already tried to
give:

```text
Error: invalid answer for 'confirm.protected.prod': must type the environment name 'prod' exactly
Error: invalid answer for 'binding.prod.docs-storage': 'maybe' is not one of the offered candidates
Error: invalid answer for 'auth.fix.all': expected yes/no
```

When a batch of questions contains both unanswered ones and badly answered
ones, the **bad answers win**: you get exit 2 naming every invalid answer,
because re-running with the missing answers alone would fail again on the
same bad value.

Answer ids are validated at startup against the known prefixes, so a typo
fails immediately rather than after a round trip to Azure:

```text
Error: unknown answer id 'foo': no question with this id is known
```

### The protected-environment gate

An environment with `policy: { protected: true }` requires its name to be
typed back before any cloud mutation. Every command behind the gate:

| Command | Gated because |
|---|---|
| `rigg push` (apply or `--prune`) | creates, updates, replaces and deletes resources |
| `rigg delete <project> --remote` | deletes the project's resources from the service |
| `rigg verify` | runs every indexer (ingestion, skill and embedding cost) |
| `rigg az indexer run` | runs an indexer |
| `rigg az indexer reset` | reprocesses every document on the next run |
| `rigg auth doctor --fix` | assigns roles, changes auth options and firewall rules |
| `rigg auth easy-auth` | rewrites a function app's authentication |
| `rigg auth roles remove` | deletes role assignments |

Three commands are deliberately outside it. `rigg env remove --clean-roles`
removes role assignments without the gate: the flag names the removal and the
environment is going away anyway. `rigg push --verify` is not gated twice —
the push's own gate covered it. And `rigg promote` writes the target
environment's files, so the push that follows is where the gate fires.

At a terminal the gate is one prompt:

```text
Environment 'prod' is protected. Type its name to confirm push:
```

Non-interactively that is the question `confirm.protected.<env>`, and
`--confirm-env <env>` is shorthand for
`--answer confirm.protected.<env>=<env>`:

```bash
rigg push contoso-docs --env prod --confirm-env prod
```

A `--confirm-env` that does not match is a usage error naming what was
expected:

```text
Error: --confirm-env must equal the environment name 'prod' exactly
```

> [!WARNING]
> **`--yes` does not satisfy this gate.** `--yes` skips the routine "apply N
> change(s)?" prompt, and scripts reach for it reflexively; if it also
> unlocked protection, a protected environment would be no safer than an
> unprotected one. `--yes` in fact makes the session non-interactive, so a
> protected push with `--yes` and no confirmation exits 6 asking for one.

## Question ids

Every question has a stable, hierarchical id. Ids are validated against these
prefixes; anything else is rejected at startup.

| Prefix | Asked by | Id form | Kind | Answer |
|---|---|---|---|---|
| `confirm.protected.` | any cloud mutation against a protected environment | `confirm.protected.<env>` | `confirm-env` | the environment's name, exactly |
| `binding.` | `rigg promote`; `rigg env add --like` | `binding.<env>.<binding>` | `choice`, other values allowed | a resource name or ARM id, or `skip` |
| `env.` | `rigg env add` | `env.<name>.protected` | `confirm` | `yes` / `no` |
| `learn.` | `rigg env bind <env> --learn` | `learn.<env>.<proposed-name>`, `learn.<env>.record` | `text`, `confirm` | a binding name (or `skip`); `yes` / `no` |
| `promote.` | `rigg promote` | `promote.bind.<from-env>.<physical>`, `promote.external.<host>`, `promote.deployment.<stem>` | `text`, `confirm`, `choice` | a binding name or `skip`; `yes` / `no`; `skip` or `capacity:<n>` |
| `auth.` | `rigg auth doctor --fix`, `rigg push` (auth preflight), `rigg auth easy-auth`, `rigg auth roles remove`, `rigg auth doctor` key-vault carrier | `auth.fix.all`, `auth.easyauth.<site>`, `auth.roles.remove`, `auth.webapi.key-vault` | `confirm`, `text` | `yes` / `no`; `<secret>@<key-vault binding>` |

**`binding.`** is asked by `rigg promote` for a binding the target
environment lacks, and by `rigg env add --like` for a binding to carry over.

**`learn.<env>.record`** is the `confirm` that writes the accepted names to
`rigg.yaml`.

**The variable segment is always the thing the question is about** — an
environment name, a binding name, a resource stem, a host — so an agent can
construct the id it needs to answer without parsing the prompt.

In a `learn.<env>.<proposed-name>` id that segment is the **binding name rigg
proposes** (the physical resource's name in lower-kebab), not the resource id
it was learned from; the answer is the name to record instead, or `skip`.

Three sentinel answers recur:

| Answer | Where | Means |
|---|---|---|
| `skip` | `binding.*`, `promote.bind.*`, `promote.deployment.*`, `learn.*` | Leave it as it is; do not bind, do not promote this one |
| `continue` | `promote.deployment.*` | Promote the deployment anyway, despite the reason given |
| `capacity:<n>` | `promote.deployment.*` | Promote the deployment with this capacity (at least 1 whole unit) |
| `yes` / `no` | every `confirm` question | The obvious |

Anything else on a deployment question is rejected:

```text
Error: invalid answer for 'promote.deployment.gpt-5.2-chat': 'maybe' — expected 'continue', 'skip' or 'capacity:<number>' (a whole number of units, at least 1)
```

## Scripting recipes

**Answer-and-retry loop** — the shape an agent implements once and reuses for
every command:

```bash
rigg push contoso-docs --env prod --output json --yes \
  --answer confirm.protected.prod=prod
```

Exit 6 → parse stdout, answer each `questions[].id`, re-run with the
`--answer` flags appended. Exit 2 → the invocation is wrong; fix it, do not
retry blindly (an answer that was rejected, or a missing flag — a
non-interactive `push` with changes to apply needs `--yes`, which is why it
is there from the first attempt). Exit 5 → a real conflict; a human decides
between `rigg pull` and `rigg push`.

`--yes` and `--confirm-env` / `--answer confirm.protected.prod=prod` are
different keys to different locks and a protected push needs both: `--yes`
for the routine "apply N change(s)?" prompt that a non-interactive push
cannot show, the answer for the gate. Supplying only `--yes` exits 6 (the
gate has no answer); supplying only the answer exits 2
(`non-interactive push requires --yes`).

**Fail CI on drift:**

```bash
rigg validate
rigg diff --all --exit-code
```

Exit 3 from the first stops the job on a bad definition; exit 5 from the
second stops it on drift. Without `--exit-code`, `rigg diff` prints the
differences and still exits 0.

**Deploy from CI, non-interactively:**

```bash
rigg push --all --env prod --confirm-env prod --yes
```

`--yes` covers the routine apply prompt, `--confirm-env` covers the protected
gate. Any *other* question still exits 6 — which is the point: CI should stop
and ask rather than guess at a binding.

## Common mistakes

**Parsing the human summary instead of stdout.** The summary is on stderr and
is not stable. The JSON document on stdout is.

**Treating exit 6 as a failure.** It is a request for input, and re-running
with the answers is the intended flow. A CI job that treats it as fatal will
never get past its first new binding.

**Using `--yes` to get through a protected environment.** See
[the gate](#the-protected-environment-gate).

**Answering with a candidate's `label`.** Answer with its `value`.

**Assuming a question id is stable across environments.** It is not — the id
embeds the environment or resource name, which is what makes it
unambiguous. Read the ids out of the document rather than hard-coding them.

**Retrying an exit 2 unchanged.** The invocation itself is wrong; the same
invocation will fail the same way.

## See also

- [`CONCEPTS.md`](../../CONCEPTS.md) — exit codes and "when rigg needs an answer" in prose (`rigg concepts`).
- [rigg.yaml § Policy](rigg-yaml.md#policy) — `protected` and `strict-bindings`.
- [Environment variables](environment-variables.md) — `RIGG_NON_INTERACTIVE` and the rest of the session controls.
- [`MCP.md`](../../MCP.md) — how the MCP server surfaces this protocol to an agent.
- [CLI reference](cli.md#global-options) — `--answer`, `--answers-file`, `--yes`, `--non-interactive`, `--output`.
- [Tutorial 4 — push to protected production](../tutorials/04-push-to-protected-production.md).
