---
name: rigg-pull
description: Pull Azure AI Search and Foundry resource definitions from Azure to local files
disable-model-invocation: true
allowed-tools: mcp__rigg__rigg_status, mcp__rigg__rigg_pull, mcp__rigg__rigg_describe, mcp__rigg__rigg_env_list, Read, Glob
argument-hint: "[environment-name]"
---

Pull resources from Azure for the specified environment (or default).

## Current state
!`rigg status --output json 2>/dev/null || echo "Not in a rigg project"`

## Steps
1. Use the `rigg_status` MCP tool to verify auth and environment
2. Use the `rigg_pull` MCP tool to pull resources (without `force` first to preview)
3. Review the changes and report what was pulled
4. If the preview looks good, re-run with `force: true` to execute
5. After pulling, use `rigg_describe` to give a summary of the project

Environment: $ARGUMENTS (use default if empty)
