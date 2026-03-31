---
name: test-complete-enduser-experience
description: Comprehensive end-to-end test of the rigg CLI user experience — covers cloud-first and local-first workflows, all resource types, sync verification, and knowledge source edge cases against live Azure services.
allowed-tools: mcp__rigg__rigg_status, mcp__rigg__rigg_describe, mcp__rigg__rigg_env_list, mcp__rigg__rigg_validate, mcp__rigg__rigg_list, mcp__rigg__rigg_diff, mcp__rigg__rigg_pull, mcp__rigg__rigg_push, mcp__rigg__rigg_delete, Bash, Read, Write, Edit, Glob, Grep, AskUserQuestion
argument-hint: "[environment-name]"
---

Perform a comprehensive end-to-end test of the rigg CLI user experience against live Azure services. This test covers every stage from project setup to resource synchronization, verifying that all commands succeed on first try and the experience is smooth and reliable.

Environment: $ARGUMENTS (use default if empty)

---

## Phase 0: Azure resource discovery and user confirmation

Before any testing, discover available Azure resources and let the user choose what to test against.

1. Run `az account show` to confirm the active Azure subscription
2. Run `az search service list` to find available Azure AI Search services
3. Run `az cognitiveservices account list` to find AI Services accounts (for Foundry)
4. Run `az storage account list` to find available storage accounts
5. Present the discovered resources to the user and ask them to select which ones to use
6. **Warn the user clearly**: resources WILL be created, modified, and deleted during testing. There may be costs associated with Azure resource usage. Ask for explicit confirmation before proceeding.

Use `AskUserQuestion` to let the user select from the discovered resources and confirm they accept the risks.

---

## Phase 1: Project initialization

1. Create a temporary test directory (e.g., `test-projects/e2e-test-<timestamp>`)
2. Run `rigg init` in that directory, configuring it with the user-selected Azure services
3. Verify that `rigg.yaml` was created with the correct configuration
4. Run `rigg status` and `rigg env list` to confirm the project is properly initialized

---

## Phase 2: Cloud-first scenario (pull from existing Azure resources)

Test pulling resources that already exist in Azure into the local project.

### 2a. Pull all resources
1. Run `rigg pull` (preview first, then execute) to pull all available resources
2. Verify that local files were created for each pulled resource
3. Run `rigg describe` to get a full inventory of what was pulled

### 2b. Sync verification (critical)
For EVERY resource type that was pulled:
1. Run `rigg diff` immediately after pull
2. **Verify that diff reports NO differences** — if any diffs appear, this is a bug
3. Pay special attention to:
   - Knowledge sources and their managed sub-resources (index, indexer, data source, skillset)
   - Agents (YAML normalization)
   - Resources with array fields (ordering sensitivity)

### 2c. Pull individual resource types
Test pulling each resource type in isolation using the appropriate flags:
- `--indexes`
- `--indexers`
- `--datasources`
- `--skillsets`
- `--synonymmaps`
- `--aliases`
- `--knowledgebases` (if `include_preview: true`)
- `--knowledgesources` (if `include_preview: true`)
- `--agents` (Foundry)

For each: pull, then diff, then verify no differences.

---

## Phase 3: Local-first scenario (push new resources to Azure)

Test creating resources locally and pushing them to Azure.

### 3a. Create and push individual resources
For each resource type, create a minimal valid definition locally and push it:

1. **Index**: Create a simple index with a few fields, push, verify with diff
2. **Data source**: Create a data source pointing to the user's storage account, push, verify
3. **Skillset**: Create a basic skillset, push, verify
4. **Synonym map**: Create a synonym map, push, verify
5. **Alias**: Create an alias pointing to the test index, push, verify
6. **Knowledge base**: Create a knowledge base definition, push, verify
7. **Knowledge source**: Create a knowledge source with its managed sub-resources, push, verify. This is the most complex — ensure all sub-resources (index, indexer, data source, skillset) are created correctly
8. **Agent**: Create a Foundry agent YAML definition, push, verify

For each resource:
- Use `rigg validate` before pushing
- Use `rigg diff` to preview changes
- Push with `rigg push` (preview first, then force)
- Immediately run `rigg diff` after push — **must report no differences**

### 3b. Modify and re-push
1. Make a change to at least one resource of each type (e.g., add a field to an index, update agent instructions)
2. Validate, diff, push
3. Verify diff shows no differences after push

---

## Phase 4: Multi-resource workflow (CI/CD simulation)

Test deploying a complete set of related resources together, simulating a CI/CD pipeline.

1. Create a cohesive set of resources: an agent + knowledge base + knowledge source (with managed sub-resources) + standalone index
2. Push all resources together using `rigg push --all`
3. Verify with `rigg diff --all` — must report no differences
4. Modify several resources at once
5. Run `rigg validate` to check for dependency/reference errors
6. Push all changes at once
7. Verify sync again

---

## Phase 5: Knowledge source deep-dive

Knowledge sources are the most complex resource type and deserve extra attention.

1. **Creation**: Push a new knowledge source and verify all managed sub-resources are created
2. **Pull verification**: Pull the knowledge source and verify the KS directory structure:
   - `<ks-name>/<ks-name>.json` (KS definition)
   - `<ks-name>/<ks-name>-index.json` (managed index)
   - `<ks-name>/<ks-name>-indexer.json` (managed indexer)
   - `<ks-name>/<ks-name>-datasource.json` (managed data source)
   - `<ks-name>/<ks-name>-skillset.json` (managed skillset)
3. **Sync verification**: Diff immediately after pull — no differences
4. **Modification**: Modify the KS definition and push (test the delete-and-recreate workaround if needed)
5. **Deletion and recreation**: Delete the KS from Azure (`rigg delete --knowledgesource <name> --target remote`), then re-push to verify clean recreation
6. **Standalone vs managed**: Verify that standalone resources (not managed by a KS) remain in their top-level directories and are not confused with managed resources

---

## Phase 6: Delete and cleanup verification

1. Delete each test resource from Azure using `rigg delete --target remote`
2. Verify that local files are untouched after remote deletion
3. Delete local files using `rigg delete --target local`
4. Verify that Azure resources are untouched after local deletion
5. Run `rigg status` to confirm clean state

---

## Phase 7: Edge cases and error handling

1. **Conflict detection**: Modify a resource in Azure (via pull from a different state), then try to push a local change — verify that rigg warns about conflicts
2. **Overwrite warning**: Pull a resource, modify the local file manually, then pull again — verify that rigg warns before overwriting local changes
3. **Invalid resources**: Try pushing an invalid resource definition — verify helpful error messages
4. **Auth issues**: If possible, test behavior when auth token is expired or missing

---

## Phase 8: Report generation

After completing all phases, generate a comprehensive test report:

### Report structure
1. **Executive summary**: Overall pass/fail status, number of tests run, critical issues found
2. **Phase-by-phase results**: For each phase, list:
   - Tests executed
   - Pass/fail status for each test
   - Any unexpected behavior or errors encountered
   - Screenshots or command output for failures
3. **Sync verification matrix**: A table showing every resource type, whether pull-then-diff and push-then-diff both reported clean (no differences)
4. **Knowledge source sub-resource tracking**: Detailed results for KS managed resource handling
5. **Issues found**: Categorized by severity (critical / major / minor / cosmetic)
6. **Improvement recommendations**: Specific, actionable items that an AI agent could pick up and execute. Each recommendation should include:
   - What the issue is
   - Where in the codebase it likely lives
   - Suggested approach to fix it
   - Expected impact on user experience

Save the report as `test-projects/e2e-test-<timestamp>/TEST-REPORT.md`.

---

## Guidelines

- **Ask before destructive actions**: Always confirm with the user before deleting resources from Azure
- **First-try success**: Every command should succeed on the first attempt. If it doesn't, note it as a bug
- **No manual workarounds**: If a step requires a manual workaround, document it as an issue
- **Timing**: Note how long knowledge source operations take (indexing can be slow)
- **Cost awareness**: Use minimal resource definitions to minimize Azure costs
- **Clean up**: Always clean up test resources at the end to avoid leaving orphaned Azure resources
