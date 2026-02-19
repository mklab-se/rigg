---
name: hoist-pull
description: Pull Azure AI Search and Foundry resource definitions from Azure to local files
disable-model-invocation: true
allowed-tools: mcp__hoist__hoist_status, mcp__hoist__hoist_pull, mcp__hoist__hoist_describe, mcp__hoist__hoist_env_list, Read, Glob
argument-hint: "[environment-name]"
---

Pull resources from Azure for the specified environment (or default).

## Current state
!`hoist status --output json 2>/dev/null || echo "Not in a hoist project"`

## Steps
1. Use the `hoist_status` MCP tool to verify auth and environment
2. Use the `hoist_pull` MCP tool to pull resources (without `force` first to preview)
3. Review the changes and report what was pulled
4. If the preview looks good, re-run with `force: true` to execute
5. After pulling, use `hoist_describe` to give a summary of the project

Environment: $ARGUMENTS (use default if empty)
