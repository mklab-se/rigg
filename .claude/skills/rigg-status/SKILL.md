---
name: rigg-status
description: Inspect rigg workspace sync state — projects, environments, drift, unmanaged resources.
---

1. `rigg_status` — per-resource sync classification and unmanaged remote
   resources. `rigg_env_list` for environments.
2. When anything is not "in-sync", run `rigg_diff` for the affected project
   and summarize what differs and which command reconciles it
   (push for local-ahead, pull for remote-ahead, conflict → user decides).
