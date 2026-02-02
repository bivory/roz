# Development Guide

## Prerequisites

- Rust 1.85+ (or use Dev Container)
- [Mise](https://mise.jdx.dev/) (optional, for task runner)

## Build Commands

```bash
mise run build          # Build debug binary
mise run test           # Run tests
mise run clippy         # Lint
mise run fmt            # Format code
mise run pre-commit     # All checks before committing
```

Or directly with cargo:

```bash
cargo build
cargo nextest run
cargo clippy -- -D warnings
cargo fmt
```

## Local Plugin Testing

Test the plugin locally without publishing to GitHub.

### 1. Build the Binary

```bash
cargo build --release
```

### 2. Add to PATH

```bash
# Option A: Symlink
ln -sf $(pwd)/target/release/roz ~/.local/bin/roz

# Option B: Add to PATH
export PATH="$(pwd)/target/release:$PATH"
```

### 3. Install the Local Plugin

From the roz repository root, in Claude Code:

```text
/plugin marketplace add ./
/plugin install roz@bivory
```

### 4. Verify Installation

```text
/plugin list
```

You should see `roz@bivory` listed with status "Enabled" and hooks installed.

### 5. Test the Flow

Send a message with `#roz` prefix:

```text
#roz Test that roz is working
```

Check that a session was created:

```bash
roz list
```

## Troubleshooting

### Hooks not showing in `/hooks` menu

Plugin hooks don't appear in the `/hooks` menu but ARE active. This is a Claude
Code UI quirk. Check `/plugin list` to verify hooks are installed.

### Sessions not being created

1. Verify the binary works:

   ```bash
   echo '{"session_id":"test","cwd":"/tmp"}' | roz hook user-prompt
   ```

   Should output `{}`.

2. Check the plugin is enabled:

   ```text
   /plugin list
   ```

3. Restart Claude Code after plugin changes.

### Review not triggering

1. Check session state:

   ```bash
   roz debug <session_id>
   ```

   Should show `review.enabled: true`.

2. Check trace events:

   ```bash
   roz trace <session_id> -v
   ```

## Dev Container

### VS Code

1. Install the **Dev Containers** extension
2. Open the repository
3. Select **Dev Containers: Reopen in Container**

### Command Line

```bash
mise run dc:build    # Build container
mise run dc:shell    # Start shell in container
mise run dc:claude   # Run Claude Code in container
```

## Releasing

### Dry Run

Preview what a release would do:

```bash
mise run release:dry-run
```

### Cut a Release

1. Ensure all changes are committed and tests pass:

   ```bash
   mise run pre-commit
   ```

2. Run the release (choose one):

   ```bash
   mise run release:patch   # 0.1.2 -> 0.1.3
   mise run release:minor   # 0.1.2 -> 0.2.0
   mise run release:major   # 0.1.2 -> 1.0.0
   ```

   This automatically:
   - Bumps version in `Cargo.toml`
   - Syncs version to `plugin.json` and `marketplace.json`
   - Creates a commit and tag
   - Pushes to GitHub

3. GitHub Actions builds binaries and creates the release.

4. Verify the release at `https://github.com/bivory/roz/releases`.

## Project Structure

```text
src/
  main.rs           # CLI entry point
  lib.rs            # Library root
  core/
    state.rs        # Session state, decisions
    hooks.rs        # Hook handlers
  storage/
    file.rs         # File backend (~/.roz/sessions/)
    memory.rs       # In-memory backend (testing)
  hooks/
    input.rs        # HookInput parsing
    output.rs       # HookOutput types
    runner.rs       # Hook dispatch
  cli/              # CLI commands
```
