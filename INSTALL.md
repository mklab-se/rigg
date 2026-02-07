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

## cargo binstall

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) downloads a pre-built binary instead of compiling from source:

```bash
cargo binstall hoist-az
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
