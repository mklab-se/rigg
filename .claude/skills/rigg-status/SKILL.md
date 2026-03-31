---
name: rigg-status
description: Show rigg project status, environments, and resource inventory
disable-model-invocation: true
allowed-tools: mcp__rigg__rigg_status, mcp__rigg__rigg_describe, mcp__rigg__rigg_env_list, mcp__rigg__rigg_validate
---

Inspect the current rigg project state.

## Steps
1. Use `rigg_env_list` to show all environments
2. Use `rigg_status` to show the current environment state
3. Use `rigg_describe` for a full resource inventory
4. Report findings in a clear summary
