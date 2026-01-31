# roz - Implementation

This document describes where to find the Rust implementation for all
components described in the [Architecture](./01-architecture.md). Rather than
duplicating code, this document references the actual source files.

## 1. Project Structure

```text
roz/
├── Cargo.toml
├── src/
│   ├── lib.rs                 # Library root
│   ├── main.rs                # CLI entry point (clap-based)
│   │
│   ├── core/
│   │   ├── mod.rs
│   │   ├── state.rs           # SessionState, Decision, ReviewState
│   │   └── hooks.rs           # Hook logic (pure functions)
│   │
│   ├── storage/
│   │   ├── mod.rs
│   │   ├── traits.rs          # MessageStore trait
│   │   ├── file.rs            # File-based session storage
│   │   └── memory.rs          # In-memory (testing)
│   │
│   ├── hooks/
│   │   ├── mod.rs
│   │   ├── input.rs           # HookInput parsing
│   │   ├── output.rs          # HookOutput generation
│   │   └── runner.rs          # Hook dispatch
│   │
│   ├── cli/
│   │   ├── mod.rs
│   │   ├── hook.rs            # hook command (runner)
│   │   ├── trace.rs           # trace command
│   │   ├── debug.rs           # debug command
│   │   ├── context.rs         # context command
│   │   ├── decide.rs          # decide command
│   │   ├── list.rs            # list command
│   │   ├── clean.rs           # clean command
│   │   └── stats.rs           # stats command
│   │
│   ├── config.rs              # Configuration loading
│   ├── template.rs            # Template system
│   └── error.rs               # Error types
│
├── tests/
│   └── hook_flow.rs           # Integration tests
│
├── agents/
│   └── roz.md                 # Reviewer agent instructions
│
└── hooks/
    └── hooks.json             # Claude Code hook configuration
```

## 2. Core Types

### 2.1 Hook I/O

| Type | Location | Description |
|------|----------|-------------|
| `HookInput` | `src/hooks/input.rs:10-48` | Input from Claude Code hooks |
| `HookOutput` | `src/hooks/output.rs:8-51` | Output for Stop/UserPrompt hooks |
| `HookDecision` | `src/hooks/output.rs:37-42` | Approve/Block enum |
| `PreToolUseOutput` | `src/hooks/output.rs:54-60` | Output for PreToolUse hooks |
| `PreToolUseDecision` | `src/hooks/output.rs:62-75` | PreToolUse decision details |
| `PermissionDecision` | `src/hooks/output.rs:77-84` | Allow/Deny/Ask enum |

Helper constructors for outputs are in `src/hooks/output.rs:86-132`.

### 2.2 Session State

| Type | Location | Description |
|------|----------|-------------|
| `SessionState` | `src/core/state.rs:9-40` | Main session container |
| `ReviewState` | `src/core/state.rs:42-82` | Review tracking state |
| `Decision` | `src/core/state.rs:85-110` | Pending/Complete/Issues enum |
| `DecisionRecord` | `src/core/state.rs:112-120` | Historical decision entry |
| `GateTrigger` | `src/core/state.rs:122-136` | Gate trigger context |
| `TruncatedInput` | `src/core/state.rs:138-196` | Truncated tool input (10KB limit) |

The `TruncatedInput::from_value()` implementation handles truncation with
SHA-256 hashing at `src/core/state.rs:155-196`.

### 2.3 Trace Events

| Type | Location | Description |
|------|----------|-------------|
| `TraceEvent` | `src/core/state.rs:280-294` | Individual trace entry |
| `EventType` | `src/core/state.rs:297-318` | Event type enum |

Event types: `SessionStart`, `PromptReceived`, `GateBlocked`, `GateAllowed`,
`ToolCompleted`, `StopHookCalled`, `RozDecision`, `TraceCompacted`,
`SessionEnd`.

### 2.4 Review Attempts (A/B Testing)

| Type | Location | Description |
|------|----------|-------------|
| `ReviewAttempt` | `src/core/state.rs:240-251` | Block attempt record |
| `AttemptOutcome` | `src/core/state.rs:254-278` | Outcome of attempt |

**Note:** `AttemptOutcome::Success` uses `decision_type: String` instead of
`decision: Decision` for serialization simplicity.

## 3. Storage Layer

### 3.1 MessageStore Trait

**Location:** `src/storage/traits.rs:8-36`

| Method | Description |
|--------|-------------|
| `get_session(id)` | Retrieve session by ID |
| `put_session(state)` | Save session (atomic write) |
| `list_sessions(limit)` | List recent sessions |
| `delete_session(id)` | Remove session |

`SessionSummary` struct: `src/storage/traits.rs:38-52`

### 3.2 File Backend

**Location:** `src/storage/file.rs`

| Component | Lines | Description |
|-----------|-------|-------------|
| `FileBackend` struct | 11-34 | File-based storage |
| `get_session` | 37-45 | Read JSON file |
| `put_session` | 47-59 | Atomic write (temp + rename) |
| `list_sessions` | 61-92 | Directory scan with sort |
| `delete_session` | 94-100 | Remove file |
| `get_roz_home` | 103-115 | Default path resolution |

The atomic write pattern at lines 47-59 prevents corruption by writing to a
`.tmp` file first, then renaming.

### 3.3 Memory Backend

**Location:** `src/storage/memory.rs`

In-memory implementation for testing, using `RwLock<HashMap>` for thread
safety.

## 4. Hook Handlers

All hook handlers are in `src/core/hooks.rs`.

### 4.1 Session-Start Hook

**Location:** `src/core/hooks.rs:27-66`

- Creates or retrieves session state
- Adds `SessionStart` trace event
- Detects second opinion sources (codex, gemini)
- Returns context injection if available

Second opinion detection: `src/core/hooks.rs:68-82`

### 4.2 User-Prompt Hook

**Location:** `src/core/hooks.rs:93-137`

- Tracks `last_prompt_at` for approval scope
- Detects `#roz` prefix to enable review
- Stores prompts and adds trace events
- Always approves (never blocks user input)

### 4.3 Stop Hook

**Location:** `src/core/hooks.rs:143-256`

| Logic | Lines | Description |
|-------|-------|-------------|
| Main handler | 143-175 | Entry point with fail-open |
| Review check | 177-256 | Decision evaluation |
| Circuit breaker | 199-220 | Force approve after max blocks |
| Block for review | 222-256 | Template loading and blocking |

The stop hook implements the core blocking logic:

- If review not enabled -> approve
- If `Decision::Complete` -> approve
- If `Decision::Pending` or `Decision::Issues` -> block (spawn roz)
- Circuit breaker trips after `max_blocks` (default 3)

### 4.4 Subagent-Stop Hook

**Location:** `src/core/hooks.rs:258-328`

Validates that:

1. Subagent type is `roz:roz`
2. SESSION_ID was extracted from prompt
3. Decision was posted during roz execution (timestamp validation)

Session ID extraction: `src/core/hooks.rs:330-349`

Timestamp validation prevents the main agent from self-approving by running
`roz decide` directly.

### 4.5 PreToolUse Hook (Gate)

**Location:** `src/core/hooks.rs:352-445`

| Logic | Lines | Description |
|-------|-------|-------------|
| Main handler | 352-445 | Gate evaluation |
| Gate approval check | 447-491 | Scope-based approval |
| Trace gate allowed | 493-507 | Debugging visibility |
| Trace compaction | 509-558 | Size limiting |

The gate handler:

1. Checks if gates are enabled (non-empty tools array)
2. Matches tool against patterns
3. Checks circuit breaker and approval status
4. Stores `GateTrigger` context for roz to review
5. Denies with instructions to spawn roz

### 4.6 Bash Command Normalization

**Location:** `src/core/hooks.rs:754-923`

| Function | Lines | Description |
|----------|-------|-------------|
| `format_tool_key` | 754-774 | Format tool for matching |
| `normalize_bash_command` | 776-813 | Main normalization |
| `find_last_unquoted_pipe` | 815-852 | Pipe handling |
| `skip_env_command` | 854-874 | `env` prefix handling |
| `extract_nested_shell_command` | 876-897 | `bash -c` extraction |
| `strip_env_vars` | 899-923 | `VAR=value` stripping |

Handles:

- Pipes: matches rightmost command (`echo "y" | gh issue close` ->
  `gh issue close`)
- Env vars: `GH_TOKEN=x gh issue` -> `gh issue`
- Nested shells: `bash -c "gh issue close"` -> `gh issue close`

### 4.7 Pattern Matching

**Location:** `src/core/hooks.rs:927-939`

Uses glob-style matching. Pattern order matters - first match wins.

## 5. CLI Commands

### 5.1 Main Entry Point

**Location:** `src/main.rs`

| Component | Lines | Description |
|-----------|-------|-------------|
| CLI definition | 11-89 | Clap derive macros |
| Command dispatch | 91-127 | Match on subcommand |

### 5.2 Hook Runner

**Location:** `src/cli/hook.rs:19-61`

Reads JSON from stdin, dispatches to appropriate handler, writes JSON to
stdout.

Hook dispatch logic: `src/hooks/runner.rs:10-21`

### 5.3 Context Command

**Location:** `src/cli/context.rs:14-74`

Shows session state including:

- Session metadata (ID, timestamps)
- Review state and decision
- Gate trigger details (if present)
- User prompts

### 5.4 Decide Command

**Location:** `src/cli/decide.rs:19-80`

Posts COMPLETE or ISSUES decision:

- Validates session exists
- Creates decision record
- Sets `gate_approved_at` for COMPLETE
- Adds trace event

### 5.5 Other Commands

| Command | Location | Description |
|---------|----------|-------------|
| `debug` | `src/cli/debug.rs:14-28` | Full session state dump |
| `trace` | `src/cli/trace.rs:14-52` | Trace event viewer |
| `list` | `src/cli/list.rs:21-47` | List recent sessions |
| `clean` | `src/cli/clean.rs:16-55` | Remove old sessions |
| `stats` | `src/cli/stats.rs:82-120` | Template A/B results |

## 6. Template System

**Location:** `src/template.rs`

| Component | Lines | Description |
|-----------|-------|-------------|
| `DEFAULT_BLOCK_TEMPLATE` | 10-28 | Hardcoded fallback |
| `load_template` | 34-52 | Load with fallback |
| `select_template` | 59-64 | Random or pinned |
| `weighted_random` | 72-111 | A/B test selection |

The default template instructs the agent to spawn `roz:roz` with the session
ID.

## 7. Configuration

**Location:** `src/config.rs`

| Type | Lines | Description |
|------|-------|-------------|
| `Config` | 16-39 | Main config struct |
| `StorageConfig` | 42-55 | Storage paths |
| `ReviewConfig` | 57-75 | Review mode settings |
| `ReviewMode` | 77-90 | Always/Prompt/Never |
| `GatesConfig` | 92-112 | Gate tool patterns |
| `ApprovalScope` | 114-127 | Session/Prompt/Tool |
| `CircuitBreakerConfig` | 129-147 | Max blocks, cooldown |
| `CleanupConfig` | 149-161 | Retention settings |
| `ExternalModelsConfig` | 163-181 | Codex/Gemini paths |
| `TemplateConfig` | 183-202 | A/B test config |
| `TraceConfig` | 203-227 | Max events limit |

Configuration precedence:

1. Environment variables (`ROZ_HOME`, etc.)
2. Config file (`~/.roz/config.toml`)
3. Defaults

## 8. Error Handling

**Location:** `src/error.rs`

| Error | Lines | Description |
|-------|-------|-------------|
| `Storage` | 13-14 | I/O errors |
| `Serde` | 17-18 | JSON errors |
| `InvalidState` | 20-22 | State machine errors |
| `SessionNotFound` | 24-26 | Missing session |
| `InvalidDecision` | 28-30 | Bad decision input |
| `MissingField` | 33-34 | Required field missing |
| `Config` | 36-38 | Config loading errors |

### Fail-Open Philosophy

All hook handlers follow fail-open: infrastructure errors approve rather than
block. Examples:

- Session-start: `src/core/hooks.rs:48-50`
- User-prompt: `src/core/hooks.rs:105-107`
- Stop hook: `src/core/hooks.rs:165-168`
- PreToolUse: `src/core/hooks.rs:374-376`

## 9. Plugin Integration

### 9.1 hooks.json

**Location:** `hooks/hooks.json`

Configures Claude Code hooks:

- `SessionStart` -> `roz hook session-start`
- `UserPromptSubmit` -> `roz hook user-prompt`
- `PreToolUse` (tissue/beads/Bash) -> `roz hook pre-tool-use`
- `Stop` -> `roz hook stop`
- `SubagentStop` (roz:roz) -> `roz hook subagent-stop`

### 9.2 Plugin Structure

**Location:** `.claude-plugin/`

| File | Description |
|------|-------------|
| `plugin.json` | Plugin manifest |
| `marketplace.json` | Marketplace metadata |
| `install.sh` | Binary installer (postinstall) |

### 9.3 Agent Instructions

**Location:** `agents/roz.md`

Full behavioral specification for the roz:roz reviewer agent.

## 10. Testing

### 10.1 Unit Tests

Each module contains `#[cfg(test)]` sections with unit tests:

| Module | Test Location | Coverage |
|--------|---------------|----------|
| `core/state.rs` | Bottom of file | State serialization, defaults |
| `core/hooks.rs` | Bottom of file | All hook handlers, trace compaction |
| `storage/file.rs` | Bottom of file | CRUD, atomic writes, corruption |
| `storage/memory.rs` | Bottom of file | CRUD, concurrency |
| `hooks/input.rs` | Bottom of file | JSON parsing |
| `hooks/output.rs` | Bottom of file | Serialization |
| `cli/*.rs` | Bottom of files | Command logic |
| `config.rs` | Bottom of file | Loading, defaults |
| `template.rs` | Bottom of file | Selection, loading |

### 10.2 Integration Tests

**Location:** `tests/hook_flow.rs`

Full flow tests:

- User prompt -> stop -> decide -> approve flow
- Issues flow with re-review
- Gate approval scopes (session/prompt/tool)
- Circuit breaker behavior
- Subagent timestamp validation

## Related Documents

- [Overview](./00-overview.md) - Vision, core concepts, design principles
- [Architecture](./01-architecture.md) - System diagrams, domain model,
  sequences
- [Test Plan](./03-test-plan.md) - Testing strategy
- [CI](./04-ci.md) - Version management and release workflow
- [Agent Instructions](./agents-roz.md) - Full roz agent behavioral
  specification
