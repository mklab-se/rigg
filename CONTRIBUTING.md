# Contributing to hoist

Thank you for considering contributing to hoist! This guide will help you get started.

## Getting Started

1. Fork the repository and clone your fork
2. Install Rust 1.82+ via [rustup](https://rustup.rs/)
3. Build the project: `cargo build`
4. Run tests: `cargo test`

## Development Workflow

### Project Structure

```
crates/
  hoist-az/       # CLI binary and command implementations
  hoist-core/      # Resource types, config, normalization, copy logic
  hoist-azent/    # Azure REST API client and authentication
  hoist-diff/      # Standalone semantic JSON diff engine
```

### Running Tests

```bash
cargo test                    # All tests
cargo test -p hoist-core   # Single crate
cargo test test_name          # Single test
cargo clippy                  # Lint check
```

### Code Style

- Run `cargo clippy` before submitting — CI will check this
- Follow existing patterns in the codebase
- Use `serde_json` with `preserve_order` — Azure's property ordering is intentionally maintained
- Add tests for new functionality

## Making Changes

### Bug Fixes

1. Create a branch: `git checkout -b fix/description`
2. Write a test that reproduces the bug
3. Fix the bug
4. Verify all tests pass: `cargo test`
5. Open a pull request

### New Features

1. Open an issue to discuss the feature first
2. Create a branch: `git checkout -b feature/description`
3. Implement with tests
4. Update the README if the feature is user-facing
5. Open a pull request

### Adding a New Resource Type

To add support for a new Azure AI Search resource type:

1. Add a variant to `ResourceKind` in `crates/hoist-core/src/resources/traits.rs`
2. Create a struct implementing the `Resource` trait (volatile fields, dependencies, etc.)
3. Register it in the resource module
4. Add the API path and directory mapping
5. Add CLI flags (both plural `--newthings` and singular `--newthing <NAME>`)
6. Add tests

### Key Concepts

- **Volatile fields**: Stripped during normalization (both pull and push). Examples: `@odata.etag`, credentials.
- **Read-only fields**: Kept in local files for documentation, stripped only before push. Examples: `createdResources`, `startTime`.
- **Identity keys**: Used for array diffing — arrays are matched by a key field (usually `name`) rather than position.
- **Checksums**: Pull uses checksums to skip unchanged resources, but always verifies the file exists on disk.

## Pull Requests

- Keep PRs focused — one feature or fix per PR
- Include tests for new code paths
- Write a clear description of what changed and why
- CI must pass (build, test, clippy)

## Reporting Issues

- Use GitHub Issues
- Include: what you expected, what happened, reproduction steps
- Include your `hoist.toml` (with credentials removed) if relevant

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
