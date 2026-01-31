# roz

Quality gate for Claude Code that enforces independent review before task
completion.

Named after Monsters Inc.'s Roz ("I'm watching you, Wazowski. Always
watching."), roz uses Claude Code hooks to block agent exit until a separate
reviewer agent approves the work. This project is inspired by alice.

## Installation

### Add the Marketplace

```bash
/plugin marketplace add bivory/roz
```

### Install the Plugin

```bash
/plugin install roz@bivory
```

### Verify Installation

```bash
/plugin list
```

## Usage

### Manual Review (Recommended for Getting Started)

Prefix any prompt with `#roz` to enable review for that task:

```text
#roz Implement the user authentication feature
```

When you try to exit, roz blocks until the `roz:roz` reviewer agent approves
your work.

### Automatic Review (Gates)

Configure automatic review triggers in `~/.roz/config.toml`:

```toml
[review.gates]
tools = ["mcp__tissue__close*", "mcp__beads__complete*"]
approval_scope = "prompt"
```

This triggers review before executing matching tools (e.g., closing issues).

## How It Works

1. **Trigger**: `#roz` prompt prefix or matching gate pattern
2. **Block**: Stop hook blocks exit when review is pending
3. **Review**: Spawn `roz:roz` agent to review work
4. **Decide**: Reviewer posts COMPLETE or ISSUES
5. **Continue**: COMPLETE allows exit; ISSUES requires fixes

## Configuration

Optional configuration at `~/.roz/config.toml`. All settings have sensible
defaults - you only need to configure what you want to change.

### Review Settings

```toml
[review]
mode = "prompt"  # When to require review
```

| Value | Description |
|-------|-------------|
| `"prompt"` | Review only when `#roz` prefix is used (default) |
| `"always"` | Review every task, regardless of prefix |
| `"never"` | Disable review entirely |

### Gate Settings

Gates automatically trigger review when specific tools are called, useful for
protecting destructive operations like closing issues.

```toml
[review.gates]
tools = ["mcp__tissue__close*", "mcp__beads__complete*"]
approval_scope = "prompt"
approval_ttl_seconds = 3600
```

| Setting | Description | Default |
|---------|-------------|---------|
| `tools` | Glob patterns for tools that require review. Empty = gates disabled. | `[]` |
| `approval_scope` | How long approval lasts (see below) | `"prompt"` |
| `approval_ttl_seconds` | Optional expiry time for approvals in seconds | none |

**Approval Scopes:**

| Scope | Behavior |
|-------|----------|
| `"session"` | Once approved, all gated tools allowed until session ends |
| `"prompt"` | Approval resets when user sends a new prompt (recommended) |
| `"tool"` | Every gated tool call requires fresh review |

**Pattern Examples:**

- `mcp__tissue__close*` - Match tissue close operations
- `mcp__beads__*` - Match all beads operations
- `Bash:rm *` - Match rm commands (Bash tools use `Bash:` prefix)
- `Bash:git push*` - Match git push commands

Bash commands are normalized before matching: environment variables are
stripped, pipes match the rightmost command, and `bash -c` wrappers are
unwrapped.

### Circuit Breaker

Prevents infinite blocking loops when something goes wrong. After the maximum
number of blocks, roz force-approves to let the user continue.

```toml
[circuit_breaker]
max_blocks = 3
cooldown_seconds = 300
```

| Setting | Description | Default |
|---------|-------------|---------|
| `max_blocks` | Number of blocks before force-approving | `3` |
| `cooldown_seconds` | Time before breaker resets after tripping | `300` (5 min) |

### Trace Settings

Trace events provide debugging visibility into roz's decisions.

```toml
[trace]
max_events = 500
```

| Setting | Description | Default |
|---------|-------------|---------|
| `max_events` | Maximum trace events per session (older events dropped) | `500` |

### Cleanup Settings

Control automatic cleanup of old session files.

```toml
[cleanup]
retention_days = 7
```

| Setting | Description | Default |
|---------|-------------|---------|
| `retention_days` | Days to keep session files before cleanup | `7` |

### Template Settings

Templates control the message shown when blocking the agent. Supports A/B
testing to compare template effectiveness.

```toml
[templates]
active = "default"

[templates.weights]
v1 = 50
v2 = 50
```

| Setting | Description | Default |
|---------|-------------|---------|
| `active` | Template to use: `"default"`, `"v1"`, `"v2"`, or `"random"` | `"default"` |
| `weights` | Weight map for random selection (only used when `active = "random"`) | `{default: 100}` |

Custom templates can be placed at `~/.roz/templates/block-{name}.md`.

### Storage Settings

```toml
[storage]
path = "/custom/path/to/roz"
```

| Setting | Description | Default |
|---------|-------------|---------|
| `path` | Directory for roz data (sessions, config) | `~/.roz` |

### External Models

Configure paths to external CLI tools for second opinions during review.

```toml
[external_models]
codex = "codex"
gemini = "gemini"
```

| Setting | Description | Default |
|---------|-------------|---------|
| `codex` | Path to Codex CLI (empty string to disable) | `"codex"` |
| `gemini` | Path to Gemini CLI (empty string to disable) | `"gemini"` |

### Environment Variables

Settings can also be configured via environment variables, which take
precedence over the config file:

| Variable | Overrides |
|----------|-----------|
| `ROZ_HOME` | `storage.path` |
| `ROZ_CONFIG` | Config file path (default: `~/.roz/config.toml`) |
| `ROZ_REVIEW_MODE` | `review.mode` |
| `ROZ_MAX_BLOCKS` | `circuit_breaker.max_blocks` |
| `ROZ_COOLDOWN_SECONDS` | `circuit_breaker.cooldown_seconds` |
| `ROZ_MAX_EVENTS` | `trace.max_events` |
| `ROZ_RETENTION_DAYS` | `cleanup.retention_days` |

### Full Example

```toml
# ~/.roz/config.toml

[review]
mode = "prompt"  # Only review when #roz prefix used

[review.gates]
# Require review before closing issues or completing tasks
tools = ["mcp__tissue__close*", "mcp__beads__complete*", "Bash:git push*"]
approval_scope = "prompt"  # Reset approval on each new prompt
approval_ttl_seconds = 3600  # Expire approvals after 1 hour

[circuit_breaker]
max_blocks = 3  # Force-approve after 3 blocks
cooldown_seconds = 300  # Reset breaker after 5 minutes

[trace]
max_events = 500  # Keep last 500 trace events

[cleanup]
retention_days = 7  # Clean up sessions older than 7 days

[templates]
active = "default"  # Use default template

[external_models]
codex = "codex"  # Use codex for second opinions
gemini = "gemini"  # Fallback to gemini
```

## CLI Commands

```bash
roz hook <name>              # [Internal] Run hook
roz decide <sid> COMPLETE    # [Agent] Approve work
roz decide <sid> ISSUES      # [Agent] Request changes
roz context <sid>            # [Agent] Show prompts for review
roz list                     # [User] List recent sessions
roz debug <sid>              # [User] Full session state
roz trace <sid>              # [User] Show trace events
roz clean --before 7d        # [User] Remove old sessions
roz stats --days 30          # [User] Template performance
```

## License

AGPL-3.0-or-later
