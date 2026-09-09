# rigg 2.0 — Interaction model: guided flows, question protocol, onboarding

**Date:** 2026-09-09
**Status:** Design, approved direction. Workstream 4 of
`2026-09-09-rigg-2.0-scope-and-principles-design.md`. Cross-cutting: the
bindings, promote and auth specs all use the mechanisms defined here.

## 1. Goals

- **Interactive first.** On a terminal with flags omitted, rigg guides: it
  discovers, proposes, explains what will happen, and asks before acting.
- **Scriptable always.** Every guided flow has a flag-complete non-interactive
  form, and any unanswered question is emitted in a machine-readable form so
  CI and AI agents (the MCP server) can answer it and re-run.
- **Every line carries context.** Environment, project, service and URL are
  on every output line where they matter. Hints teach the next command.

## 2. Modes

| Mode | When | Behaviour |
|---|---|---|
| interactive | stdin and stdout are a TTY and neither `--yes`, `--non-interactive` nor `--output json` is given | wizards, pick-lists, confirmations |
| non-interactive | otherwise | no prompts; missing input → question protocol (§3) |

`RIGG_NON_INTERACTIVE=1` forces non-interactive. `--yes` skips *routine*
confirmations only; it never answers a question and never satisfies a
protected-environment gate or a replace gate.

## 3. Question protocol

When a command in non-interactive mode needs input it cannot infer, it writes
one JSON document to stdout and exits **6** (`NeedsInput`, new exit code):

```json
{
  "status": "needs-input",
  "command": "promote",
  "context": { "project": "regulus", "from": "dev", "to": "prod" },
  "questions": [
    {
      "id": "binding.prod.docs-storage",
      "kind": "choice",
      "prompt": "Environment 'prod' has no binding 'docs-storage' (storage). Which storage account should prod use?",
      "candidates": [
        { "value": "same", "label": "same as dev: mklabstorageacc (shared)" },
        { "value": "/subscriptions/…/storageAccounts/mklabstorageprod", "label": "mklabstorageprod (mklab-rg-prod)" }
      ],
      "allow_other": true,
      "default": "same"
    },
    { "id": "confirm.protected.prod", "kind": "confirm-env", "prompt": "Type the environment name to confirm push to protected environment 'prod'." }
  ]
}
```

Question kinds: `choice`, `text`, `confirm`, `confirm-env` (typed name).
Ids are stable and documented per command. Answers are supplied with
`--answer <id>=<value>` (repeatable) or `--answers-file <path>` (JSON object
id → value); unknown ids are a usage error. A command re-run with all
questions answered proceeds without prompting. Interactive mode asks the same
questions inline with the same ids, so the transcript and the protocol always
agree.

MCP tools gain an `answers` object parameter and return the `needs-input`
document verbatim when the subprocess exits 6, so an AI agent can complete
any guided flow.

## 4. Guided flows

### 4.1 Not in a workspace

`rigg pull`, `rigg status`, `rigg adopt`, `rigg describe` outside a workspace
(interactive) offer to create one in place instead of failing:

1. "No rigg workspace here. Create one in `<cwd>`?" 
2. Tenant and subscription (pick-lists from ARM; default = az CLI current).
3. Search service and Foundry project pick-lists (existing `init` discovery).
4. Environment name (default `dev`), `protected?`.
5. For `pull`/`adopt`: project name (default from the Search service), then
   the adopt wizard for what to bring in, then bindings learn (§4.4).

Non-interactive: the existing usage error, now naming the exact `rigg init`
flags.

### 4.2 Promote to an environment that does not exist

`rigg promote --to staging` with no `staging` in `rigg.yaml` runs
`env add staging --like <from>` inline (bindings spec §3), then continues
with the promote preview. Non-interactive: question protocol with the
`env.create` question set (tenant, subscription, targets, one question per
binding).

### 4.3 First push into an environment

`rigg push` whose environment has no baseline yet (first push) runs the auth
preflight (auth spec) before the plan: identity existence, operator rights,
service roles, network. Interactive: offers to fix. Non-interactive: refuses
with exit 4 and the list, unless `--skip-auth-preflight`.

### 4.4 Bindings learn after adopt/pull

After files are written, if unbound infrastructure references appeared:
interactive → show the proposed bindings table and offer to record them;
non-interactive → hint line.

### 4.5 Scaffolds take bindings

`rigg new data-source docs --storage docs-storage --container regulatory`
writes a real `ResourceId=` from the binding; interactively, missing
`--storage` is a pick-list over the environment's storage bindings (plus
"bind a new one"). Same for `--model-host` (ai-services / implicit foundry)
on knowledge bases, indexes and skillsets, `--function-app` on Web API
skills, `--identity` for user-assigned identity fields. `rigg new pipeline`
asks these once and threads them through all parts. Placeholders like
`<subscription-id>` no longer exist in scaffolds; a scaffold that cannot be
completed non-interactively fails with the question protocol.

## 5. Output contract

- Every command that touches Azure prints its targets first: environment,
  project, Search service and URL, Foundry account/project and URL. Every
  per-resource line carries the environment when more than one is in play.
- Plans precede actions; actions are confirmed; results are summarized with
  counts and the next command as a hint.
- `--output json` shapes are stable interfaces from 2.0.0 (scope spec §7).
- Exit codes: 0 ok · 1 error · 2 usage · 3 validation · 4 auth · 5
  drift/conflict · **6 needs input**.

## 6. Onboarding documentation

- `GETTING_STARTED.md` rewritten around the guided flows: start from
  nothing (`rigg pull` in an empty directory), adopt an existing solution,
  add an environment, promote, push to protected prod.
- `CONCEPTS.md` gains "Environments and infrastructure" (bindings, shared vs
  different, learn vs declare) and "How rigg handles authentication"
  (principles from the auth spec, in user terms).
- `rigg concepts` and `--help` carry the same content (help parity is kept).
- The `rigg-guide` skill and the MCP server instructions are updated to the
  2.0 model.

## 7. Testing

- `interactive`/`non-interactive` detection table (TTY, flags, env var).
- Question protocol: JSON shape, exit 6, `--answer` and `--answers-file`
  parsing, unknown-id usage error, re-run proceeds. One end-to-end case per
  command that can ask (promote, env add, push preflight, new).
- Guided flows are tested at the seam: the wizard functions take an
  `Asker` trait (interactive implementation over `inquire`; scripted
  implementation for tests) so every branch is covered without a PTY.
- MCP: `answers` round-trip test.
