# roz - Implementation

This document contains the Rust implementation details for all components described in the [Architecture](./01-architecture.md).

---

## 1. Project Structure

```
roz-cli/
├── Cargo.toml
├── src/
│   ├── lib.rs                 # Library root
│   ├── main.rs                # CLI entry point
│   │
│   ├── core/
│   │   ├── mod.rs
│   │   ├── state.rs           # SessionState, Decision
│   │   ├── hooks.rs           # Hook logic (pure functions)
│   │   └── circuit_breaker.rs # Circuit breaker logic
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
│   └── cli/
│       ├── mod.rs
│       ├── hook.rs            # hook command (runner)
│       ├── trace.rs           # trace command
│       ├── debug.rs           # debug command
│       ├── context.rs         # context command
│       ├── decide.rs          # decide command
│       ├── clean.rs           # clean command
│       └── stats.rs           # stats command
```

---

## 2. Core Types

### 2.1 Hook I/O

```rust
#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub cwd: PathBuf,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
    #[serde(default)]
    pub tool_response: Option<Value>,
    #[serde(default)]
    pub source: Option<String>,  // startup, resume, clear, compact
    #[serde(default)]
    pub subagent_type: Option<String>,
    #[serde(default)]
    pub subagent_prompt: Option<String>,
    #[serde(default)]
    pub subagent_started_at: Option<DateTime<Utc>>,  // When subagent began execution
}

#[derive(Debug, Serialize)]
pub struct HookOutput {
    pub decision: HookDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,  // Injected into conversation
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HookDecision {
    Approve,
    Block,
}
```

### 2.2 Session State

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub review: ReviewState,
    pub trace: Vec<TraceEvent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewState {
    pub enabled: bool,
    pub decision: Decision,
    pub decision_history: Vec<DecisionRecord>,  // Preserved for debugging
    pub user_prompts: Vec<String>,
    pub block_count: u32,
    pub circuit_breaker_tripped: bool,
    pub attempts: Vec<ReviewAttempt>,  // Track each block attempt for A/B testing
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Decision {
    Pending,
    Complete {
        summary: String,
        second_opinions: Option<String>,
    },
    Issues {
        summary: String,
        message_to_agent: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub decision: Decision,
    pub timestamp: DateTime<Utc>,
}
```

### 2.3 Trace Events

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    SessionStart,
    PromptReceived,
    ToolCompleted,
    StopHookCalled,
    RozDecision,
    SessionEnd,
}
```

### 2.4 Review Attempts (A/B Testing)

```rust
pub struct ReviewAttempt {
    pub template_id: String,
    pub timestamp: DateTime<Utc>,
    pub outcome: AttemptOutcome,
}

pub enum AttemptOutcome {
    /// Still waiting for result
    Pending,

    /// Roz spawned and posted decision
    Success {
        decision: Decision,
        blocks_needed: u32,
    },

    /// Agent didn't spawn roz (circuit breaker tripped)
    NotSpawned,

    /// Roz spawned but didn't post decision
    NoDecision,

    /// Roz spawned but SESSION_ID was missing/wrong
    BadSessionId,
}
```

---

## 3. Storage Layer

### 3.1 MessageStore Trait

```rust
pub trait MessageStore: Send + Sync {
    /// Get session state by ID
    fn get_session(&self, session_id: &str) -> Result<Option<SessionState>>;

    /// Save session state
    fn put_session(&self, state: &SessionState) -> Result<()>;

    /// List recent sessions
    fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>>;

    /// Delete session (cleanup)
    fn delete_session(&self, session_id: &str) -> Result<()>;
}

pub struct SessionSummary {
    pub session_id: String,
    pub first_prompt: Option<String>,
    pub created_at: DateTime<Utc>,
    pub event_count: usize,
}
```

### 3.2 File Backend

```rust
pub struct FileBackend {
    base_dir: PathBuf,
}

impl FileBackend {
    pub fn new(base_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(base_dir.join("sessions"))?;
        Ok(Self { base_dir })
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.base_dir.join("sessions").join(format!("{}.json", session_id))
    }
}

impl MessageStore for FileBackend {
    fn get_session(&self, session_id: &str) -> Result<Option<SessionState>> {
        let path = self.session_path(session_id);
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&path)?;
        let state: SessionState = serde_json::from_str(&contents)?;
        Ok(Some(state))
    }

    fn put_session(&self, state: &SessionState) -> Result<()> {
        let path = self.session_path(&state.session_id);
        let temp = path.with_extension("tmp");

        // Write to temp file first
        let contents = serde_json::to_string_pretty(state)?;
        fs::write(&temp, &contents)?;

        // Atomic rename - prevents corruption if process crashes mid-write
        fs::rename(&temp, &path)?;

        Ok(())
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let sessions_dir = self.base_dir.join("sessions");
        let mut sessions = Vec::new();

        for entry in fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map_or(false, |e| e == "json") {
                if let Ok(contents) = fs::read_to_string(&path) {
                    if let Ok(state) = serde_json::from_str::<SessionState>(&contents) {
                        sessions.push(SessionSummary {
                            session_id: state.session_id,
                            first_prompt: state.review.user_prompts.first().cloned(),
                            created_at: state.created_at,
                            event_count: state.trace.len(),
                        });
                    }
                }
            }

            if sessions.len() >= limit {
                break;
            }
        }

        // Sort by created_at descending (most recent first)
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(sessions)
    }

    fn delete_session(&self, session_id: &str) -> Result<()> {
        let path = self.session_path(session_id);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}
```

---

## 4. Hook Handlers

### 4.1 Pure Hook Functions

```rust
impl SessionState {
    pub fn handle_user_prompt(&mut self, prompt: &str) -> HookOutput {
        // Parse #roz command
        // Update state
        // Return output
    }

    pub fn handle_stop(&mut self) -> HookOutput {
        // Check review state
        // Check decision
        // Apply circuit breaker
        // Return approve/block
    }

    pub fn handle_reviewer_decision(&mut self, decision: Decision) -> HookOutput {
        // Update decision
        // Return output
    }
}
```

### 4.2 Subagent-Stop Hook (Timestamp Validation)

The subagent-stop hook validates that:
1. Roz posted a decision
2. The decision was posted **during** roz's execution (not before by the main agent)

This prevents the main agent from running `roz decide` directly to approve its own work.

```rust
pub fn handle_subagent_stop(input: &HookInput, store: &dyn MessageStore) -> HookOutput {
    // Only validate roz:roz subagent
    let subagent_type = match &input.subagent_type {
        Some(t) if t == "roz:roz" => t,
        _ => return HookOutput::approve(),
    };

    // Extract session ID from roz's prompt
    let session_id = match extract_session_id(&input.subagent_prompt) {
        Some(id) => id,
        None => return HookOutput::block(
            "roz:roz completed but SESSION_ID not found in prompt."
        ),
    };

    // Get subagent execution window from hook input
    let subagent_started = input.subagent_started_at
        .unwrap_or_else(|| Utc::now() - Duration::hours(1));  // Fallback: 1 hour ago
    let subagent_ended = Utc::now();  // Hook runs immediately after subagent

    // Check if roz recorded a decision
    let state = match store.get_session(&session_id) {
        Ok(Some(s)) => s,
        _ => return HookOutput::approve_with_warning("Could not verify roz decision"),
    };

    match &state.review.decision {
        Decision::Pending => HookOutput::block(&format!(
            "roz:roz completed but did not record a decision.\n\n\
             Run: roz decide {} COMPLETE \"summary\"\n\
              or: roz decide {} ISSUES \"summary\" --message \"what to fix\"",
            session_id, session_id
        )),
        Decision::Complete { .. } | Decision::Issues { .. } => {
            // Verify decision was posted during subagent execution
            // This prevents main agent from running `roz decide` directly
            let decision_time = state.updated_at;

            if decision_time < subagent_started {
                return HookOutput::block(&format!(
                    "Decision timestamp {} is before roz started {}. \
                     Decision must be posted by roz:roz during its execution.",
                    decision_time, subagent_started
                ));
            }

            // Allow small buffer after subagent ends (clock skew tolerance)
            let buffer = Duration::seconds(5);
            if decision_time > subagent_ended + buffer {
                return HookOutput::block(&format!(
                    "Decision timestamp {} is after roz ended {}. \
                     Decision must be posted by roz:roz during its execution.",
                    decision_time, subagent_ended
                ));
            }

            HookOutput::approve()
        }
    }
}
```

### 4.3 Stop Hook (Block for Review)

```rust
fn block_for_review(
    session_id: &str,
    state: &mut SessionState,
    config: &Config,
) -> HookOutput {
    // Select template (random or pinned)
    let template_id = select_template(&config.templates);
    let template = load_template(&template_id);  // Falls back to default
    let message = template.replace("{{session_id}}", session_id);

    // Record attempt
    state.review.attempts.push(ReviewAttempt {
        template_id: template_id.clone(),
        timestamp: Utc::now(),
        outcome: AttemptOutcome::Pending,
    });
    state.review.block_count += 1;

    HookOutput {
        decision: HookDecision::Block,
        reason: Some(message),
        context: None,
    }
}
```

---

## 5. CLI Commands

### 5.1 Hook Runner

```bash
roz hook <name>              # Run hook, JSON stdin/stdout
```

### 5.2 Debugging & Context

```bash
roz debug <session_id>       # Show all state for session
roz trace <session_id>       # Show trace events
roz trace <session_id> -v    # Verbose with payloads
roz context <session_id>     # Show user prompts (for roz to review)
```

Example output for `roz context`:

```bash
$ roz context abc123-def456

Session: abc123-def456
Created: 2026-01-31T10:30:00Z

User prompts:
[1] 2026-01-31T10:30:00Z
    #roz Fix the authentication bug in login.ts

[2] 2026-01-31T10:35:00Z
    Also make sure the logout flow works correctly
```

### 5.3 Decision Posting

```bash
roz decide <session_id> COMPLETE "Summary of findings"
roz decide <session_id> ISSUES "Summary" --message "What needs to change"
```

Command specification:

```
roz decide <session_id> <decision> <summary> [--message <msg>]

Arguments:
  session_id    Session UUID (from SESSION_ID= in prompt)
  decision      COMPLETE or ISSUES
  summary       Brief summary of findings

Options:
  --message     Required for ISSUES - guidance for the agent
  --opinions    Optional - record of second opinions obtained
```

Implementation:

```rust
pub fn cmd_decide(
    session_id: &str,
    decision: &str,
    summary: &str,
    message: Option<&str>,
    opinions: Option<&str>,
    store: &dyn MessageStore,
) -> Result<()> {
    let mut state = store.get_session(session_id)?
        .ok_or_else(|| Error::SessionNotFound(session_id.into()))?;

    let new_decision = match decision.to_uppercase().as_str() {
        "COMPLETE" => Decision::Complete {
            summary: summary.into(),
            second_opinions: opinions.map(Into::into),
        },
        "ISSUES" => Decision::Issues {
            summary: summary.into(),
            message_to_agent: message.map(Into::into),
        },
        other => return Err(Error::InvalidDecision(other.into())),
    };

    // Preserve history
    state.review.decision_history.push(DecisionRecord {
        decision: state.review.decision.clone(),
        timestamp: Utc::now(),
    });

    state.review.decision = new_decision;
    state.updated_at = Utc::now();

    store.put_session(&state)?;

    println!("Decision recorded: {} for session {}", decision, session_id);
    Ok(())
}
```

### 5.4 Session Management

```bash
roz clean                    # Remove sessions older than 7 days (default)
roz clean --before 30d       # Remove sessions older than 30 days
roz clean --all              # Remove all sessions
roz stats                    # Template A/B test results (last 30 days)
roz stats --days 7           # Results from last 7 days
```

Clean implementation:

```rust
pub fn clean_sessions(store: &dyn MessageStore, before: Duration) -> Result<usize> {
    let cutoff = Utc::now() - before;
    let sessions = store.list_sessions(1000)?;
    let mut removed = 0;

    for summary in sessions {
        if summary.created_at >= cutoff {
            continue;  // Too recent
        }

        // Don't delete active sessions (review still pending)
        if let Some(session) = store.get_session(&summary.session_id)? {
            if session.review.enabled && matches!(session.review.decision, Decision::Pending) {
                continue;  // Still active, skip
            }
        }

        store.delete_session(&summary.session_id)?;
        removed += 1;
    }

    Ok(removed)
}
```

Stats implementation:

```rust
pub fn cmd_stats(store: &dyn MessageStore, days: u32) -> Result<()> {
    let cutoff = Utc::now() - Duration::days(days as i64);
    let sessions = store.list_sessions(10000)?;

    let mut stats: HashMap<String, TemplateStats> = HashMap::new();

    for summary in sessions {
        if summary.created_at < cutoff {
            continue;
        }
        let session = store.get_session(&summary.session_id)?;
        if let Some(session) = session {
            for attempt in &session.review.attempts {
                let entry = stats.entry(attempt.template_id.clone())
                    .or_default();
                entry.record(&attempt.outcome);
            }
        }
    }

    render_stats_table(&stats);
    Ok(())
}
```

Example stats output:

```
Template Performance (last 30 days):
┌──────────┬─────────┬─────────┬───────────┬─────────────┐
│ Template │ Success │ Failure │ Avg Blocks│ Success Rate│
├──────────┼─────────┼─────────┼───────────┼─────────────┤
│ v1       │ 142     │ 8       │ 1.2       │ 94.7%       │
│ v2       │ 45      │ 12      │ 1.8       │ 78.9%       │
└──────────┴─────────┴─────────┴───────────┴─────────────┘

Failure breakdown:
  NotSpawned:   5 (25%)
  NoDecision:   12 (60%)
  BadSessionId: 3 (15%)
```

---

## 6. Template System

### 6.1 Template Configuration

```rust
pub struct TemplateConfig {
    /// Which template: "v1", "v2", "v3", or "random"
    pub active: String,

    /// Weights for random selection
    /// e.g., {"v1": 50, "v2": 50} = 50/50 split
    pub weights: HashMap<String, u32>,
}

fn select_template(config: &TemplateConfig) -> String {
    match config.active.as_str() {
        "random" => weighted_random(&config.weights),
        specific => specific.to_string(),
    }
}
```

### 6.2 Template Loading with Fallback

```rust
const DEFAULT_BLOCK_TEMPLATE: &str = r#"Review required before exit.

Use the **Task** tool with these parameters:

- `subagent_type`: `"roz:roz"`
- `model`: `"opus"`

Prompt template:

```
SESSION_ID={{session_id}}

## Summary
[What you did and why]

## Files Changed
[List of modified files]
```
"#;

fn load_template(id: &str) -> String {
    let path = template_path(id);
    match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("roz: warning: template {} not found ({}), using default", id, e);
            DEFAULT_BLOCK_TEMPLATE.to_string()
        }
    }
}
```

---

## 7. Configuration

### 7.1 Config File

Optional `~/.roz/config.toml`:

```toml
[storage]
path = "~/.roz"

[review]
# always = review every prompt
# prompt (default) = review when #roz prefix used
# never = disable review entirely
mode = "prompt"

[circuit_breaker]
max_blocks = 3
cooldown_seconds = 300

[cleanup]
# Auto-cleanup sessions older than this
retention_days = 7

[external_models]
# Optional paths to external model CLIs
# Set to "" to disable
codex = "codex"
gemini = "gemini"

[templates]
# "v1", "v2", "v3" to pin a specific template
# "random" for A/B testing with weights
active = "random"

[templates.weights]
v1 = 70
v2 = 30
```

### 7.2 Configuration Precedence

1. Environment variables (`ROZ_STORAGE_PATH`, etc.)
2. Config file (`~/.roz/config.toml`)
3. Auto-discovery
4. Defaults

### 7.3 Second Opinion Fallback

Roz seeks second opinions to validate its review. The fallback order:

| Priority | Source | Command |
|----------|--------|---------|
| 1 | Codex | `codex exec -m gpt-5.2 -c reasoning=high ...` |
| 2 | Gemini | `gemini -m gemini-3-pro-preview ...` |
| 3 | Claude (Opus) | Task tool with `model: "opus"` |

**Detection in session-start hook:**

```rust
fn check_second_opinion_sources() -> SecondOpinionConfig {
    SecondOpinionConfig {
        codex_available: command_exists("codex"),
        gemini_available: command_exists("gemini"),
        // Claude Opus always available via Task tool
        fallback: "opus".to_string(),
    }
}
```

**Fallback logic in agent instructions:**

```markdown
## Second Opinion

You MUST seek at least one second opinion before deciding.

1. **Try Codex first** (if available):
   codex exec -s read-only -m gpt-5.2 -c reasoning=high "Review this: ..."

2. **Try Gemini** (if Codex unavailable):
   gemini -s -m gemini-3-pro-preview "Review this: ..."

3. **Fall back to Claude Opus** (if neither available):
   Use the Task tool with `model: "opus"` to spawn a separate reviewer.

Record which source provided the second opinion in your decision.
```

---

## 8. Error Handling

### 8.1 Fail-Open Philosophy

Infrastructure errors should not block the user:

```rust
pub fn run_hook(input: HookInput, store: &dyn MessageStore) -> HookOutput {
    match run_hook_inner(input, store) {
        Ok(output) => output,
        Err(e) => {
            eprintln!("roz: warning: {}", e);
            HookOutput::approve_with_warning(format!("Infrastructure error: {}", e))
        }
    }
}
```

### 8.2 Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Storage error: {0}")]
    Storage(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Circuit breaker tripped")]
    CircuitBreakerTripped,

    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Invalid decision: {0}")]
    InvalidDecision(String),
}
```

### 8.3 Known Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Agent runs `roz decide` directly** | Critical | Subagent-stop hook validates decision timestamp is within roz execution window |
| **Roz manipulated by misleading summary** | High | Agent instructions: never trust summary, always read actual files, use `git diff` |
| **Session ID as bearer token** | Medium | Acceptable for local use; session files have user permissions |
| **Same model family fallback** | Medium | Opus fallback has correlated blind spots; documented as known limitation |
| **Template file missing** | Medium | Hardcoded default template fallback |
| **Active session cleaned** | Medium | Skip sessions with `Decision::Pending` |
| **Second opinions disagree** | Low | Agent instructions: err on side of ISSUES when opinions conflict |
| **Disk/permission errors** | Low | Fail open with warning |

---

## 9. Plugin Integration

### 9.1 hooks.json

```json
{
  "hooks": {
    "Stop": [{
      "hooks": [{
        "type": "command",
        "command": "roz hook stop",
        "timeout": 30
      }]
    }]
  }
}
```

### 9.2 Plugin Structure

```
roz/
├── .claude-plugin/
│   └── plugin.json            # Required by Claude Code
├── agents/
│   └── roz.md                 # Quality gate agent
├── hooks/
│   └── hooks.json             # Required by Claude Code
├── templates/
│   └── block-v1.md            # Block message templates
└── bin/
    └── roz                    # Rust binary
```

### 9.3 Claude Code Plugin Compatibility

The plugin format is **dictated by Claude Code** - we must conform to it.

**Fixed (Claude Code Format):**

| Component | Format | Location |
|-----------|--------|----------|
| Plugin manifest | JSON | `.claude-plugin/plugin.json` |
| Hook configuration | JSON | `hooks/hooks.json` |
| Agents | Markdown + YAML frontmatter | `agents/*.md` |
| Skills | Markdown + YAML frontmatter | `skills/*/SKILL.md` |

**What We Control:**

| Component | Our Choice |
|-----------|------------|
| CLI binary | Rust |
| State storage | File-based JSON |
| Hook implementation | Rust functions |

Note: No skills directory. All quality gate logic is in `agents/roz.md`.

---

## 10. Roz Agent Instructions

Full agent instructions: `claude-doc/agents-roz.md`

**Summary:**

| Aspect | Specification |
|--------|---------------|
| Identity | Adversarial reviewer, works for user |
| Model | Opus |
| Tools | Read, Grep, Glob, Bash (read-only) |
| Context | `roz context <session_id>` |
| Decision | `roz decide <session_id> COMPLETE\|ISSUES "summary"` |
| Second opinions | Codex → Gemini → Claude Opus fallback |

**Core process:**
1. Extract SESSION_ID from prompt
2. Get user context with `roz context`
3. Study the actual changes (read files, don't trust summaries)
4. Apply deep reasoning (steel-man then attack)
5. Get second opinion
6. Post decision with `roz decide`

---

## Related Documents

- [Overview](./00-overview.md) - Vision, core concepts, design principles
- [Architecture](./01-architecture.md) - System diagrams, domain model, sequences
- [Test Plan](./03-test-plan.md) - Testing strategy
- [CI](./04-ci.md) - Version management and release workflow
- [Agent Instructions](./agents-roz.md) - Full roz agent behavioral specification
