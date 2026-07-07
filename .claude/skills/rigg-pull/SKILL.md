---
name: rigg-pull
description: Pull rigg-managed configuration from Azure into project files, previewing changes first.
---

Bring remote configuration into the project's files:

1. `rigg_pull` without `force` — returns the diff (preview).
2. Summarize what would change locally; flag conflicts (both sides changed).
3. On approval: `rigg_pull` with `force: true`. To claim unmanaged remote
   resources into the project, add `adopt: true` (requires explicit project).
4. `rigg_status` afterwards to confirm everything is in sync.
