---
name: test-complete-enduser-experience
description: End-to-end acceptance test of rigg against live Azure by walking the four tutorials in docs/tutorials/ exactly as a reader would (mklabsrch + mklabaifndr/proj-default + mklabstorageacc). Run before releases, after registry/API-version changes, and whenever a tutorial changes.
---

# End-to-end acceptance test: walk the four tutorials

The acceptance criterion for rigg is that a reader who follows
`docs/tutorials/01`…`04` gets what the pages say they will get. So the test
*is* the tutorials: run each page's commands in order, against live Azure,
and compare what the terminal prints with the block printed under it.

Anything that differs is a bug in one of the two — fix rigg, or fix the page.
Never fix it by relaxing a test or a docs guard.

## Live resources — the rules

- Services: Search `mklabsrch`, Foundry `mklabaifndr`/`proj-default`, storage
  `mklabstorageacc`. Never create a new service, account or model deployment;
  reuse a deployment that exists (`az cognitiveservices account deployment
  list -n mklabaifndr -g mklab-rg`).
- **Name every resource you create `tut-*`.** That prefix is what the teardown
  checklist below keys on, and what keeps you clear of the real
  `regulus`/`regulatory-*`/`quelch-*`/`test-ks*` stack on the same service.
- **Never run `rigg delete <project> --remote` against a project that has
  adopted the existing resources.** Tutorial 1 adopts the real stack; its
  round trip is a throwaway synonym map, removed with `push --prune`.
- Work in `e2e-test/tutorials/<n>/` (git-excluded). Build once with
  `cargo build` and put `target/debug` first on `PATH`, so transcripts read
  `rigg …`.
- Interactive steps need a terminal. Drive them through a pty (a short
  `python3 -c 'import pty…'` harness, or `expect`) rather than a pipe:
  `inquire` cancels on EOF, and `--non-interactive` takes a different code
  path from the one the page documents.

## Tutorial 1 — adopt an existing solution

Fresh workspace. `rigg init .` → `rigg new project` → `rigg adopt <p> all` →
`env bind --learn` → `status` → `describe`.

What must hold:

- Adopt reports every unmanaged resource on both services, and `status` says
  `in sync` for all of them immediately afterwards. Anything else means
  `normalize_for_disk` is out of step with what Azure returns.
- The round trip: `rigg new synonym-map tut-roundtrip-demo -p <p>`, push,
  `rm` the file, `push --prune --dry-run` (one `delete` line),
  `push --prune` (gone from Azure), restore from Git, push (back).
- Without `--prune` the same plan prints
  `orphan synonym-maps/… (file deleted locally; pass --prune to delete remotely)`
  and changes nothing.
- `rigg verify <p>` runs every indexer, retrieves from every knowledge base
  and asks every agent. Expect `0 processed` on indexers whose corpus has not
  changed — that is the high-water mark, not a failure.

## Tutorial 2 — build a stack from scratch

Fresh workspace. Create the container and corpus first:
`az storage container create --account-name mklabstorageacc -n tut-docs
--auth-mode login`, then upload two small Markdown files. Blob *upload* needs
a data-plane role; if `--auth-mode login` is refused, fall back to
`--account-key "$(az storage account keys list …)"`.

Then the page: `rigg new pipeline tut-docs` → fill the data source, the
indexer's field mappings and the knowledge base's model → `env bind` →
`validate` → `auth doctor` → `push` → `az indexer reset`/`run --watch` →
`az index stats`/`query` → `az knowledge-base ask` → `new connection` +
`new agent` → `push --verify` → `az agent ask`.

What must hold:

- `rigg new pipeline` writes six files; three carry `<…>` placeholders
  (data source, knowledge base model) and the indexer needs field mappings
  added, or `title`/`url` come back null.
- `auth doctor` derives `Cognitive Services User` on the Foundry account from
  the knowledge base's `models[0].azureOpenAIParameters.resourceUri`.
- The push of the agent + connection trips the auth preflight, which offers
  `Search Index Data Reader` for the Foundry project, applies it, waits for it
  to become visible, and only then writes. **That assignment must be removed
  in teardown.**
- The agent answers from the corpus with a citation.
- A `remote ahead (pull pending)` on the skillset right after the first push
  is expected: Azure back-fills a skill's optional defaults after the create.
  `rigg pull` settles it; a push in the meantime prints
  `skip skillsets/… (remote changed since last sync — pull first)`.

## Tutorial 3 — add an environment and promote

Copy tutorial 2's workspace (including `.rigg/`). `rigg env add staging
--like dev` (answer "same as dev" for every binding) → `env show staging
--refresh` → `promote --dry-run` → `promote` → rename staging's `"name"`
fields to `tut-staging-*` → `promote` again → `auth doctor -e staging` →
`push -e staging --dry-run` → `push -e staging` → `verify -e staging` →
`status`.

What must hold:

- The rewiring table marks every shared binding `= … shared`, never `→`.
- After the rename, the second promote prints a **Renamed siblings** table and
  rewrites every reference — including the agent's `x-rigg-ref`,
  `project_connection_id` and `server_url`, and the connection's
  `properties.target`. A third run reports `0 changed, 0 new, 8 unchanged`.
- The staging push plan names `tut-staging-*` resources. If it names the dev
  ones, stop: the rename did not land and the push would take over dev's
  resources.
- `status` with no argument reports both environments.

## Tutorial 4 — protected production

Copy tutorial 3's workspace. Add `prod` to `rigg.yaml` by hand with
`policy.protected: true`, pointed at the same services → `env show prod` →
`env show prod --refresh` → `promote --from staging --to prod` → edit prod's
agent instructions sidecar so there is one pending change → the gate.

What must hold:

- `push -e prod --dry-run` prints the plan and asks nothing.
- `push -e prod --yes --non-interactive` exits **6** with the `needs-input`
  JSON on stdout and writes nothing. `--yes` must not open the gate.
- `--confirm-env staging` on the prod push exits **2**.
- `--confirm-env prod` pushes, and `--verify` passes.
- `validate -e prod --strict --show-bindings` exits 0. It validates every
  environment, not just prod.
- `rigg ci init github -e prod` writes three workflows and prints the role
  list derived from the workspace's own files, pinned to `--env prod`.

Do **not** create new prod resources: the prod tree is the staging content
re-pointed at the same services, so the only write is the one agent update.

## Teardown checklist — run all of it, in this order

```bash
# 1. rigg-created role assignments, per environment that has any
rigg auth roles list -e dev            # then: rigg auth roles remove -e dev
rigg auth roles list -e staging
rigg auth roles list -e prod           # protected → add --confirm-env prod

# 2. the tut-* resources, per environment, newest environment first.
#    NEVER pass --yes here: the confirmation prompt is the only preview
#    `rigg delete` has, so read the plan it prints before answering.
rigg delete <project> --remote -e prod --confirm-env prod
rigg delete <project> --remote -e staging
rigg delete <project> --remote -e dev

# 3. the blob container
az storage container delete --account-name mklabstorageacc -n tut-docs --auth-mode login

# 4. confirm nothing tut-* survives
for k in indexes indexers datasources skillsets synonymmaps aliases; do
  az rest --method get --resource https://search.azure.com \
    --url "https://mklabsrch.search.windows.net/$k?api-version=2026-04-01" \
    -o json | python3 -c "import sys,json;print([x['name'] for x in json.load(sys.stdin)['value']])"
done
for k in knowledgeSources knowledgeBases; do
  az rest --method get --resource https://search.azure.com \
    --url "https://mklabsrch.search.windows.net/$k?api-version=2026-08-01-preview" \
    -o json | python3 -c "import sys,json;print([x['name'] for x in json.load(sys.stdin)['value']])"
done
az cognitiveservices account deployment list -n mklabaifndr -g mklab-rg -o table
az storage container list --account-name mklabstorageacc --auth-mode login --query "[].name"
```

Rules for the teardown, not suggestions:

- **`rigg delete` has no `--dry-run`.** Its confirmation prompt is the whole
  preview, so run it on a terminal, without `--yes`, and read every line
  before answering. A project that adopted pre-existing resources owns them,
  and delete does not care that you did not create them.
- Remove only the role assignments rigg created (`rigg auth roles remove`
  matches on the `rigg:<workspace>:<env>:` description). Never
  `az role assignment delete` by hand.
- Model deployments are never yours to delete here — the tutorials reuse an
  existing one. Confirm it is still listed at the end.
- The local `e2e-test/tutorials/<n>/` workspaces may stay; nothing in them is
  committed.

## Finally

- `cargo run -q --bin rigg -- dev docs-check --root .` — the docs guards.
- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- Record every command, its exit code and its trimmed output. A tutorial block
  that no longer matches is a defect to file, not a transcript to edit by
  hand: re-run the command and paste what it actually printed, anonymised
  (`mklab*` → `contoso-*`, ids → zeros or `<…>` placeholders).
