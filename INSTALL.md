# Installing rigg

## Prerequisites

rigg authenticates via the Azure CLI or service principal credentials:

- **For development**: Install the [Azure CLI](https://learn.microsoft.com/en-us/cli/azure/install-azure-cli) and run `az login`
- **For CI/CD**: Set `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, and `AZURE_TENANT_ID` environment variables

If neither is configured, `rigg init` will fall back to manual service name entry. All other commands require authentication.

## Homebrew (macOS / Linux)

```bash
brew install mklab-se/tap/rigg
```

## Pre-built Binaries

Download the latest binary for your platform from [GitHub Releases](https://github.com/mklab-se/rigg/releases/latest):

| Platform | Archive |
|---|---|
| macOS (Apple Silicon) | `rigg-v*-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `rigg-v*-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64) | `rigg-v*-x86_64-unknown-linux-gnu.tar.gz` |
| Windows (x86_64) | `rigg-v*-x86_64-pc-windows-msvc.zip` |

Extract and move the binary to a directory in your `PATH`:

```bash
# macOS / Linux
tar xzf rigg-v*-*.tar.gz
sudo mv rigg /usr/local/bin/
```

## cargo install

Compile from source via crates.io (requires Rust 1.82+):

```bash
cargo install rigg
```

## Build from Source

```bash
git clone https://github.com/mklab-se/rigg.git
cd rigg
cargo build --release
```

The binary is at `target/release/rigg`. Requires Rust 1.82 or later.

## cargo binstall

If you already have [cargo-binstall](https://github.com/cargo-bins/cargo-binstall) installed, it can download a pre-built binary from GitHub Releases instead of compiling from source — combining the convenience of `cargo install` with the speed of a binary download:

```bash
cargo binstall rigg
```

If you don't have cargo-binstall, install it first:

```bash
cargo install cargo-binstall
```

For most users, Homebrew or a direct binary download from [GitHub Releases](https://github.com/mklab-se/rigg/releases/latest) is simpler.

## Shell Completions

Generate completions for your shell with `rigg completion <shell>`:

**Bash** — add to `~/.bashrc`:
```bash
source <(rigg completion bash)
```

**Zsh** — add to `~/.zshrc`:
```bash
source <(rigg completion zsh)
```

**Fish** — save to completions directory:
```bash
rigg completion fish > ~/.config/fish/completions/rigg.fish
```

**PowerShell** — add to profile:
```powershell
rigg completion powershell >> $PROFILE
```

## Verify Installation

```bash
rigg --version
```

## Connect to Your AI Coding Tool

rigg includes a built-in MCP server that gives AI coding tools direct access to understand and manage your Agentic RAG stack — pull, push, diff, and explore resources through structured tool calls instead of shell commands.

```bash
# Claude Code
rigg mcp install claude-code

# VS Code (GitHub Copilot)
rigg mcp install vs-code
```

Projects that include `.mcp.json` in the repo root are auto-discovered automatically — the AI tool picks up rigg when you open the project, no install step needed.

See [MCP.md](MCP.md) for the full tool reference and [SKILLS.md](SKILLS.md) for agent skills and slash commands.
