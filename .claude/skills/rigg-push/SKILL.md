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
5. Report the result; on exit 5 (conflict) run `rigg_diff`, explain both sides,
   and let the user choose (pull remote / push local / merge).
