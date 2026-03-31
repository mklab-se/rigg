//! AI agent skill management
//!
//! `rigg ai skill`             — show setup guide
//! `rigg ai skill --emit`      — output skill markdown file
//! `rigg ai skill --reference`  — output reference documentation

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
        r#"rigg AI Skill Setup
====================

rigg is a configuration-as-code tool for Azure AI Search and Microsoft
Foundry. A skill helps AI agents manage search indexes, Foundry agents,
knowledge bases, and other Azure AI resources.

To create the skill file, run:

  rigg ai skill --emit > ~/.claude/skills/rigg.md

Or ask your AI agent:

  "Use `rigg ai skill --emit` to set up a skill for managing Azure AI Search"

The skill instructs the AI agent to run `rigg ai skill --reference` at
runtime to fetch full documentation, so the agent always has up-to-date
command details and workflow patterns without bloating the skill file itself.

Note: rigg also provides an MCP server for direct tool integration:

  rigg mcp install claude-code
"#
    );
}

fn print_skill_file() {
    print!(
        r#"---
name: rigg
description: Configuration-as-code for Azure AI Search and Microsoft Foundry — manage search indexes, indexers, skillsets, knowledge bases, knowledge sources, Foundry agents, and more.
---

# rigg — Azure AI Search & Foundry Config-as-Code

Use this skill when the user is working with rigg, Azure AI Search, or
Microsoft Foundry configuration. rigg pulls resource definitions from Azure
as JSON/YAML files, enables local editing under Git, and pushes changes back.

## Getting detailed reference

Run this command for comprehensive documentation:

```
rigg ai skill --reference
```

This provides complete CLI reference, resource type flags, file structure,
key workflows, knowledge source managed sub-resources, rigg.yaml format,
MCP tools reference, and safety rules.

## MCP server

If the rigg MCP server is available, prefer using MCP tools for structured
operations (rigg_status, rigg_describe, rigg_validate, rigg_diff,
rigg_pull, rigg_push, rigg_delete, rigg_list, rigg_env_list).

Mutating tools (pull, push, delete) use a two-step pattern: call without
`force` for a preview, then with `force: true` to execute.

## Quick command reference

| Task | Command |
|------|---------|
| Pull all resources | `rigg pull --all` |
| Push all resources | `rigg push --all` |
| Diff against Azure | `rigg diff --all` |
| Validate locally | `rigg validate --strict --check-references` |
| Project status | `rigg status` |
| Describe project | `rigg describe` |
| List environments | `rigg env list` |
| Create resource | `rigg new <type> <name>` |
| Delete remote | `rigg delete --<type> <name> --target remote` |
| Delete local | `rigg delete --<type> <name> --target local` |
| Scaffold RAG system | `rigg new agentic-rag <name>` |
"#
    );
}

fn print_reference() {
    print!("{}", include_str!("../../doc/ai-reference.md"));
}
