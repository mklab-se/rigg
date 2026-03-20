//! AI agent skill management
//!
//! `hoist ai skill`             — show setup guide
//! `hoist ai skill --emit`      — output skill markdown file
//! `hoist ai skill --reference`  — output reference documentation

/// Run the skill subcommand.
pub fn run(emit: bool, reference: bool) {
    if emit {
        print_skill_file();
    } else if reference {
        print_reference();
    } else {
        print_guide();
    }
}

fn print_guide() {
    println!(
        r#"hoist AI Skill Setup
====================

hoist is a configuration-as-code tool for Azure AI Search and Microsoft
Foundry. A skill helps AI agents manage search indexes, Foundry agents,
knowledge bases, and other Azure AI resources.

To create the skill file, run:

  hoist ai skill --emit > ~/.claude/skills/hoist.md

Or ask your AI agent:

  "Use `hoist ai skill --emit` to set up a skill for managing Azure AI Search"

The skill instructs the AI agent to run `hoist ai skill --reference` at
runtime to fetch full documentation, so the agent always has up-to-date
command details and workflow patterns without bloating the skill file itself.

Note: hoist also provides an MCP server for direct tool integration:

  hoist mcp install claude-code
"#
    );
}

fn print_skill_file() {
    print!(
        r#"---
name: hoist
description: Configuration-as-code for Azure AI Search and Microsoft Foundry — manage search indexes, indexers, skillsets, knowledge bases, knowledge sources, Foundry agents, and more.
---

# hoist — Azure AI Search & Foundry Config-as-Code

Use this skill when the user is working with hoist, Azure AI Search, or
Microsoft Foundry configuration. hoist pulls resource definitions from Azure
as JSON/YAML files, enables local editing under Git, and pushes changes back.

## Getting detailed reference

Run this command for comprehensive documentation:

```
hoist ai skill --reference
```

This provides complete CLI reference, resource type flags, file structure,
key workflows, knowledge source managed sub-resources, hoist.yaml format,
MCP tools reference, and safety rules.

## MCP server

If the hoist MCP server is available, prefer using MCP tools for structured
operations (hoist_status, hoist_describe, hoist_validate, hoist_diff,
hoist_pull, hoist_push, hoist_delete, hoist_list, hoist_env_list).

Mutating tools (pull, push, delete) use a two-step pattern: call without
`force` for a preview, then with `force: true` to execute.

## Quick command reference

| Task | Command |
|------|---------|
| Pull all resources | `hoist pull --all` |
| Push all resources | `hoist push --all` |
| Diff against Azure | `hoist diff --all` |
| Validate locally | `hoist validate --strict --check-references` |
| Project status | `hoist status` |
| Describe project | `hoist describe` |
| List environments | `hoist env list` |
| Create resource | `hoist new <type> <name>` |
| Delete remote | `hoist delete --<type> <name> --target remote` |
| Delete local | `hoist delete --<type> <name> --target local` |
| Scaffold RAG system | `hoist new agentic-rag <name>` |
"#
    );
}

fn print_reference() {
    print!("{}", include_str!("../../doc/ai-reference.md"));
}
