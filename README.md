# roz

Quality gate for Claude Code that enforces independent review before task
completion.

Named after Monsters Inc.'s Roz ("I'm watching you, Wazowski. Always
watching."), roz uses Claude Code hooks to block agent exit until a separate
reviewer agent approves the work. This project is inspired by alice.

## Installation

### 1. Install the Binary

```bash
curl -fsSL https://raw.githubusercontent.com/bivory/roz/main/.claude-plugin/install.sh | bash
```

This installs `roz` to `~/.local/bin`. Ensure it's in your PATH:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Verify:

```bash
roz --version
```

### 2. Install the Plugin

In Claude Code:

```text
/plugin marketplace add bivory/roz
/plugin install roz@bivory
```

## Usage

Prefix any prompt with `#roz` to enable review:

```text
#roz Implement the user authentication feature
```

When Claude finishes, roz blocks until the `roz:roz` reviewer agent approves
the work.

## How It Works

1. `#roz` prompt enables review for that task
2. Stop hook blocks exit when review is pending
3. Claude spawns `roz:roz` agent to review work
4. Reviewer posts COMPLETE or ISSUES
5. COMPLETE allows exit; ISSUES requires fixes

## Configuration

Optional configuration at `~/.roz/config.toml`. All settings have sensible
defaults.

### Review Mode

```toml
[review]
mode = "prompt"  # "prompt" (default), "always", or "never"
```

### Gate Triggers

Automatically trigger review when specific tools are called:

```toml
[review.gates]
tools = ["mcp__tissue__close*", "mcp__beads__complete*"]
approval_scope = "prompt"  # "session", "prompt", or "tool"
```

### Circuit Breaker

Prevents infinite blocking loops:

```toml
[circuit_breaker]
max_blocks = 3        # Force-approve after N blocks
cooldown_seconds = 300
```

### Full Example

```toml
# ~/.roz/config.toml

[review]
mode = "prompt"

[review.gates]
tools = ["mcp__tissue__close*", "Bash:git push*"]
approval_scope = "prompt"

[circuit_breaker]
max_blocks = 3
cooldown_seconds = 300
```

## CLI Commands

```bash
roz list                     # List recent sessions
roz debug <session_id>       # Full session state
roz trace <session_id>       # Show trace events
roz clean --before 7d        # Remove old sessions
```

## License

AGPL-3.0-or-later
