# Installing hoist

## Homebrew (macOS / Linux)

```bash
brew install mklab-se/tap/hoist
```

## Pre-built Binaries

Download the latest binary for your platform from [GitHub Releases](https://github.com/mklab-se/hoist/releases/latest):

| Platform | Archive |
|---|---|
| macOS (Apple Silicon) | `hoist-v*-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `hoist-v*-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64) | `hoist-v*-x86_64-unknown-linux-gnu.tar.gz` |
| Windows (x86_64) | `hoist-v*-x86_64-pc-windows-msvc.zip` |

Extract and move the binary to a directory in your `PATH`:

```bash
# macOS / Linux
tar xzf hoist-v*-*.tar.gz
sudo mv hoist /usr/local/bin/
```

## cargo install

Compile from source via crates.io (requires Rust 1.82+):

```bash
cargo install hoist-az
```

## Build from Source

```bash
git clone https://github.com/mklab-se/hoist.git
cd hoist
cargo build --release
```

The binary is at `target/release/hoist`. Requires Rust 1.82 or later.

## cargo binstall

If you already have [cargo-binstall](https://github.com/cargo-bins/cargo-binstall) installed, it can download a pre-built binary from GitHub Releases instead of compiling from source — combining the convenience of `cargo install` with the speed of a binary download:

```bash
cargo binstall hoist-az
```

If you don't have cargo-binstall, install it first:

```bash
cargo install cargo-binstall
```

For most users, Homebrew or a direct binary download from [GitHub Releases](https://github.com/mklab-se/hoist/releases/latest) is simpler.

## Shell Completions

Generate completions for your shell with `hoist completion <shell>`:

**Bash** — add to `~/.bashrc`:
```bash
source <(hoist completion bash)
```

**Zsh** — add to `~/.zshrc`:
```bash
source <(hoist completion zsh)
```

**Fish** — save to completions directory:
```bash
hoist completion fish > ~/.config/fish/completions/hoist.fish
```

**PowerShell** — add to profile:
```powershell
hoist completion powershell >> $PROFILE
```

## Verify Installation

```bash
hoist --version
```

## Connect to Your AI Coding Tool

hoist includes a built-in MCP server that gives AI coding tools direct access to understand and manage your Agentic RAG stack — pull, push, diff, and explore resources through structured tool calls instead of shell commands.

```bash
# Claude Code
hoist mcp install claude-code

# VS Code (GitHub Copilot)
hoist mcp install vs-code
```

Projects that include `.mcp.json` in the repo root are auto-discovered automatically — the AI tool picks up hoist when you open the project, no install step needed.

See [MCP.md](MCP.md) for the full tool reference and [SKILLS.md](SKILLS.md) for agent skills and slash commands.
