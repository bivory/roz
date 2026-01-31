# Development Guide

## Prerequisites

- Docker
- [Mise](https://mise.jdx.dev/) (recommended) or Rust 1.85+

## Dev Container Setup

### Using Mise (Recommended)

Build the dev container image:

```bash
mise run dc:build
```

### Start a Shell

```bash
mise run dc:shell
```

This drops you into a bash shell inside the container with all tools installed.

### Run Claude Code

```bash
mise run dc:claude
```

This starts Claude Code in unrestricted mode inside the container.

### Using VS Code

1. Install the **Dev Containers** extension
2. Open the repository in VS Code
3. Press `Cmd+Shift+P` (macOS) or `Ctrl+Shift+P` (Linux/Windows)
4. Select **Dev Containers: Reopen in Container**

## Build Commands

All commands use Mise tasks:

```bash
mise run build          # Build debug binary
mise run test           # Run tests with nextest
mise run clippy         # Lint with warnings as errors
mise run fmt            # Format code
mise run fmt:check      # Check formatting
mise run pre-commit     # fmt:check + clippy + typos + docs:lint + dupes
mise run ci             # Full CI checks
```

Or directly with cargo:

```bash
cargo build
cargo nextest run
cargo clippy -- -D warnings
cargo fmt
```

## Project Structure

```text
src/
  main.rs           # CLI entry point
  lib.rs            # Library root
  error.rs          # Error types (thiserror)
  config.rs         # Configuration loading
  template.rs       # Template selection and loading
  core/
    state.rs        # Session state, decisions, trace events
    hooks.rs        # Hook handler implementations
    circuit_breaker.rs
  storage/
    traits.rs       # MessageStore trait
    file.rs         # File backend (~/.roz/sessions/)
    memory.rs       # In-memory backend (testing)
  hooks/
    input.rs        # HookInput parsing
    output.rs       # HookOutput, PreToolUseOutput
    runner.rs       # Hook dispatch
  cli/
    hook.rs         # roz hook <name>
    decide.rs       # roz decide
    context.rs      # roz context
    debug.rs        # roz debug
    trace.rs        # roz trace
    clean.rs        # roz clean
    stats.rs        # roz stats
```

## Testing

Run all tests:

```bash
mise run test
```

Run specific test:

```bash
cargo nextest run test_name
```

Generate coverage report:

```bash
mise run coverage:report
```

## Issue Tracking

List open issues:

```bash
mise run issue:list
```

Show issues ready to work on:

```bash
mise run issue:ready
```

## Quality Checks

Before committing:

```bash
mise run pre-commit
```

Full CI checks:

```bash
mise run ci
```

## Local Plugin Testing

Test the plugin locally without publishing to GitHub.

1. Build the release binary:

   ```bash
   cargo build --release
   ```

2. Ensure the `roz` binary is in your PATH:

   ```bash
   # Option A: Symlink to a directory in PATH
   ln -sf $(pwd)/target/release/roz ~/.local/bin/roz

   # Option B: Add target/release to PATH
   export PATH="$(pwd)/target/release:$PATH"
   ```

3. Add the local marketplace (from the roz repository root):

   ```text
   /plugin marketplace add ./
   ```

4. Install the plugin:

   ```text
   /plugin install roz@bivory
   ```

5. Verify installation:

   ```text
   /plugin list
   ```
