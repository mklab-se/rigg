---
name: hoist-status
description: Show hoist project status, environments, and resource inventory
disable-model-invocation: true
allowed-tools: mcp__hoist__hoist_status, mcp__hoist__hoist_describe, mcp__hoist__hoist_env_list, mcp__hoist__hoist_validate
---

Inspect the current hoist project state.

## Steps
1. Use `hoist_env_list` to show all environments
2. Use `hoist_status` to show the current environment state
3. Use `hoist_describe` for a full resource inventory
4. Report findings in a clear summary
