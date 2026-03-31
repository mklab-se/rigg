---
name: rigg-push
description: Push local resource changes to Azure AI Search and Foundry services
disable-model-invocation: true
allowed-tools: mcp__rigg__rigg_validate, mcp__rigg__rigg_diff, mcp__rigg__rigg_push, mcp__rigg__rigg_status, mcp__rigg__rigg_describe, Read
argument-hint: "[environment-name]"
---

Safely push local changes to Azure. Always validate and diff before pushing.

## Current state
!`rigg status --output json 2>/dev/null || echo "Not in a rigg project"`

## Steps
1. Use `rigg_validate` to check for errors (with `check_references: true`)
2. Use `rigg_diff` to preview what will change on Azure
3. Report the validation results and diff to the user
4. Only if the user confirms, use `rigg_push` with `force: true` to execute
5. Never push without showing the diff first

## Knowledge source handling
When pushing knowledge sources (`resource_type='knowledgesources'`), rigg handles all
managed sub-resources (index, indexer, data source, skillset) automatically. Do NOT push
these sub-resources separately — they are managed by Azure as part of the KS lifecycle.

If a knowledge source push fails with "already exist" errors, the workaround is:
1. Delete from Azure: `rigg_delete` with `resource_type='knowledgesources'`, `name='<name>'`,
   `target='remote'`, `force=true` (pass `env` to target a specific environment)
2. Re-push: `rigg_push` with `resource_type='knowledgesources'` and `force=true`
WARNING: Step 1 deletes the knowledge source from Azure only (local files are NOT affected).
The search index and all its data will be lost. Re-indexing occurs automatically but takes time.

Environment: $ARGUMENTS (use default if empty)
