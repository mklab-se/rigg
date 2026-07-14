---
name: rigg-push
description: Safely push rigg-managed configuration to Azure — validate, review the plan, then apply.
---

Push a project's local files to Azure, safely:

1. `rigg_validate` (project param optional) — stop and fix on any problem.
2. `rigg_push` without `force` — returns the dependency-ordered plan (dry run).
3. Show the user what will be created/updated/deleted; call out deletions
   (`--prune` orphans) and anything billing-relevant (deployments: SKU/capacity).
4. On approval: `rigg_push` with `force: true` (add `prune: true` only when
   the user explicitly wants remote deletions).
5. If the plan shows a `replace` (e.g. a knowledge-source kind change after
   `rigg migrate knowledge-source`), warn the user explicitly: the resource is
   deleted and re-created, and its index is REBUILT from source data — time,
   ingestion/embedding cost, and the source is unavailable to knowledge bases
   until repopulated. Only after explicit approval add `allow_replace: true`
   (CLI: `--allow-replace`); `force`/`--yes` alone never executes a replace.
6. Report the result; on exit 5 (conflict) run `rigg_diff`, explain both sides,
   and let the user choose (pull remote / push local / merge). If a replace was
   interrupted, re-running push resumes it (a `.rigg/<env>/<project>/replace-*.json`
   recovery file restores knowledge-base links).
