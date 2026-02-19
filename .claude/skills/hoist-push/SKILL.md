---
name: hoist-push
description: Push local resource changes to Azure AI Search and Foundry services
disable-model-invocation: true
allowed-tools: mcp__hoist__hoist_validate, mcp__hoist__hoist_diff, mcp__hoist__hoist_push, mcp__hoist__hoist_status, mcp__hoist__hoist_describe, Read
argument-hint: "[environment-name]"
---

Safely push local changes to Azure. Always validate and diff before pushing.

## Current state
!`hoist status --output json 2>/dev/null || echo "Not in a hoist project"`

## Steps
1. Use `hoist_validate` to check for errors (with `check_references: true`)
2. Use `hoist_diff` to preview what will change on Azure
3. Report the validation results and diff to the user
4. Only if the user confirms, use `hoist_push` with `force: true` to execute
5. Never push without showing the diff first

Environment: $ARGUMENTS (use default if empty)
