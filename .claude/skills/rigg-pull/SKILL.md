---
name: rigg-pull
description: Pull rigg-managed configuration from Azure into project files, previewing changes first.
---

Bring remote configuration into the project's files:

1. `rigg_pull` without `force` — returns the diff (preview).
2. Summarize what would change locally; flag conflicts (both sides changed).
3. On approval: `rigg_pull` with `force: true`. To claim ALL unmanaged remote
   resources into the project, add `adopt: true` (requires explicit project);
   for finer-grained adoption (one kind or one resource), use the
   `rigg adopt <project> <selector>` CLI instead.
4. `rigg_status` afterwards to confirm everything is in sync.
