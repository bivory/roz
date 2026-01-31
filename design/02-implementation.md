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

/// Output specifically for PreToolUse hooks (different schema from Stop hooks)
#[derive(Debug, Serialize)]
pub struct PreToolUseOutput {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: PreToolUseDecision,
}

#[derive(Debug, Serialize)]
pub struct PreToolUseDecision {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,  // Always "PreToolUse"
    #[serde(rename = "permissionDecision")]
    pub permission_decision: PermissionDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "updatedInput")]
    pub updated_input: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
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
    pub gate_trigger: Option<GateTrigger>,  // Tool that triggered gate
    pub gate_approved_at: Option<DateTime<Utc>>,  // When gate was last approved (for scope tracking)
    pub last_prompt_at: Option<DateTime<Utc>>,    // When last user prompt received
    pub review_started_at: Option<DateTime<Utc>>, // When current review cycle started (for prompt isolation)
    pub block_count: u32,
    pub circuit_breaker_tripped: bool,
    pub attempts: Vec<ReviewAttempt>,  // Track each block attempt for A/B testing
}

/// Context about what triggered the gate (stored for roz to review)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateTrigger {
    pub tool_name: String,           // e.g., "mcp__tissue__close_issue"
    pub tool_input: TruncatedInput,  // Tool input (truncated if large)
    pub triggered_at: DateTime<Utc>,
    pub pattern_matched: String,     // Which config pattern matched
}

/// Tool input with truncation for large payloads
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruncatedInput {
    /// The input value (truncated to max_size if needed)
    pub value: Value,
    /// True if the original input was truncated
    pub truncated: bool,
    /// SHA-256 hash of the original full input (for verification)
    pub original_hash: Option<String>,
    /// Original size in bytes (if truncated)
    pub original_size: Option<usize>,
}

impl TruncatedInput {
    const MAX_SIZE: usize = 10 * 1024;  // 10KB limit

    pub fn from_value(input: Value) -> Self {
        let serialized = serde_json::to_string(&input).unwrap_or_default();

        if serialized.len() <= Self::MAX_SIZE {
            return Self {
                value: input,
                truncated: false,
                original_hash: None,
                original_size: None,
            };
        }

        // Truncate: keep structure but limit string values
        let hash = sha256_hex(&serialized);
        let truncated_value = truncate_json_value(&input, Self::MAX_SIZE);

        Self {
            value: truncated_value,
            truncated: true,
            original_hash: Some(hash),
            original_size: Some(serialized.len()),
        }
    }
}

fn truncate_json_value(value: &Value, budget: usize) -> Value {
    match value {
        Value::String(s) if s.len() > budget => {
            Value::String(format!("{}... [truncated, {} bytes total]",
                &s[..budget.min(200)], s.len()))
        }
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            let per_key = budget / map.len().max(1);
            for (k, v) in map {
                new_map.insert(k.clone(), truncate_json_value(v, per_key));
            }
            Value::Object(new_map)
        }
        Value::Array(arr) if arr.len() > 10 => {
            let mut new_arr: Vec<Value> = arr.iter().take(10)
                .map(|v| truncate_json_value(v, budget / 10))
                .collect();
            new_arr.push(Value::String(format!("... [{} more items]", arr.len() - 10)));
            Value::Array(new_arr)
        }
        other => other.clone(),
    }
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
    GateBlocked,      // Pre-tool-use hook blocked a gated tool
    GateAllowed,      // Pre-tool-use hook allowed a gated tool (for debugging)
    ToolCompleted,
    StopHookCalled,
    RozDecision,
    TraceCompacted,   // Trace was truncated due to size limit
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
        let now = Utc::now();

        // Always track last prompt time (for approval_scope = "prompt")
        self.review.last_prompt_at = Some(now);

        // Check for #roz prefix
        if prompt.trim_start().starts_with("#roz") {
            self.review.enabled = true;
            self.review.user_prompts.push(prompt.to_string());
            self.review.decision = Decision::Pending;  // Reset for new review
            self.trace.push(TraceEvent {
                id: generate_id(),
                timestamp: now,
                event_type: EventType::PromptReceived,
                payload: json!({ "prompt": prompt }),
            });
        }

        self.updated_at = now;
        HookOutput::approve()
    }

    pub fn handle_stop(&mut self) -> HookOutput {
        // Check review state
        // Check decision
        // Apply circuit breaker
        // Return approve/block
    }

    pub fn handle_reviewer_decision(&mut self, decision: Decision) -> HookOutput {
        // Update decision
        // Set gate_approved_at if COMPLETE (for approval scope tracking)
        if matches!(decision, Decision::Complete { .. }) {
            self.review.gate_approved_at = Some(Utc::now());
        }
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

### 4.4 PreToolUse Hook (Gate)

The pre-tool-use hook enables automatic review before configured tools execute.

```rust
pub fn handle_pre_tool_use(
    input: &HookInput,
    config: &Config,
    store: &dyn MessageStore,
) -> PreToolUseOutput {
    // Check if gates are enabled (non-empty tools array)
    if !config.review.gates.is_enabled() {
        return PreToolUseOutput::allow();
    }

    // Format tool key for matching
    let tool_key = format_tool_key(&input.tool_name, &input.tool_input);

    // Check if tool matches any gate pattern
    let matched_pattern = find_matching_pattern(&tool_key, &config.review.gates.tools);
    if matched_pattern.is_none() {
        return PreToolUseOutput::allow();
    }
    let matched_pattern = matched_pattern.unwrap();

    // Get or create session state
    let mut state = match store.get_session(&input.session_id) {
        Ok(Some(s)) => s,
        Ok(None) => SessionState::new(&input.session_id),
        Err(e) => {
            eprintln!("roz: warning: storage error: {}", e);
            return PreToolUseOutput::allow();  // Fail open
        }
    };

    // Check circuit breaker - if tripped, allow through
    if state.review.circuit_breaker_tripped {
        trace_gate_allowed(&mut state, &tool_key, "circuit_breaker", config.trace.max_events);
        let _ = store.put_session(&state);
        return PreToolUseOutput::allow();
    }

    // Check approval based on configured scope
    if is_gate_approved(&state, &config.review.gates) {
        trace_gate_allowed(&mut state, &tool_key, "approved", config.trace.max_events);
        let _ = store.put_session(&state);
        return PreToolUseOutput::allow();
    }

    // Store full gate context for roz to review
    let now = Utc::now();
    state.review.enabled = true;
    state.review.review_started_at = Some(now);  // Mark review cycle start (for prompt isolation)
    state.review.gate_trigger = Some(GateTrigger {
        tool_name: tool_key.clone(),
        tool_input: TruncatedInput::from_value(input.tool_input.clone().unwrap_or(json!({}))),
        triggered_at: now,
        pattern_matched: matched_pattern.clone(),
    });

    // Add trace event (with limiting)
    add_trace_event(&mut state, TraceEvent {
        id: generate_id(),
        timestamp: now,
        event_type: EventType::GateBlocked,
        payload: json!({
            "tool": tool_key,
            "pattern": matched_pattern,
        }),
    }, config.trace.max_events);

    state.updated_at = now;

    if let Err(e) = store.put_session(&state) {
        eprintln!("roz: warning: failed to save state: {}", e);
        return PreToolUseOutput::allow();  // Fail open
    }

    PreToolUseOutput::deny(&format!(
        "Review required before this action.\n\n\
         Spawn **roz:roz** to review this session:\n\n\
         ```\n\
         SESSION_ID={}\n\n\
         ## Summary\n\
         [What you did and why]\n\n\
         ## Files Changed\n\
         [List of modified files]\n\
         ```\n\n\
         Triggered by: `{}`",
        input.session_id, tool_key
    ))
}

/// Check if gate is approved based on configured scope
fn is_gate_approved(state: &SessionState, gates: &GatesConfig) -> bool {
    // Must have a Complete decision
    if !matches!(state.review.decision, Decision::Complete { .. }) {
        return false;
    }

    let Some(approved_at) = state.review.gate_approved_at else {
        return false;  // Never approved
    };

    // Check TTL expiry (applies to all scopes)
    if let Some(ttl_secs) = gates.approval_ttl_seconds {
        let expiry = approved_at + Duration::seconds(ttl_secs as i64);
        if Utc::now() > expiry {
            return false;  // Approval expired
        }
    }

    match gates.approval_scope {
        ApprovalScope::Session => true,  // Any non-expired approval is valid

        ApprovalScope::Prompt => {
            // Approval must be after the last user prompt
            // BUT: ignore prompts that arrived during an active review cycle
            let effective_prompt_at = match (state.review.last_prompt_at, state.review.review_started_at) {
                (Some(prompt), Some(review_start)) if prompt > review_start => {
                    // Prompt arrived during review - use review_started_at instead
                    // This prevents "hurry up" prompts from invalidating pending approval
                    Some(review_start)
                }
                (prompt_at, _) => prompt_at,
            };

            match effective_prompt_at {
                Some(prompt) => approved_at > prompt,
                None => true,  // No prompts yet, approval valid
            }
        }

        ApprovalScope::Tool => false,  // Every tool call needs fresh review
    }
}

/// Trace when gate allows (for debugging visibility)
fn trace_gate_allowed(state: &mut SessionState, tool: &str, reason: &str, max_events: usize) {
    add_trace_event(state, TraceEvent {
        id: generate_id(),
        timestamp: Utc::now(),
        event_type: EventType::GateAllowed,
        payload: json!({
            "tool": tool,
            "reason": reason,
        }),
    }, max_events);
}

/// Add trace event with size limiting (drops oldest events if over limit)
fn add_trace_event(state: &mut SessionState, event: TraceEvent, max_events: usize) {
    state.trace.push(event);

    // Enforce limit by dropping oldest events (but keep first 10 for context)
    if state.trace.len() > max_events {
        let keep_start = 10.min(max_events / 2);
        let keep_end = max_events - keep_start;

        // Keep first `keep_start` and last `keep_end` events
        let total = state.trace.len();
        let mut new_trace = Vec::with_capacity(max_events);
        new_trace.extend(state.trace.drain(..keep_start));
        new_trace.push(TraceEvent {
            id: generate_id(),
            timestamp: Utc::now(),
            event_type: EventType::TraceCompacted,
            payload: json!({
                "dropped_events": total - max_events,
                "kept_start": keep_start,
                "kept_end": keep_end,
            }),
        });
        new_trace.extend(state.trace.drain((total - keep_start - keep_end)..));
        state.trace = new_trace;
    }
}

/// Improved Bash command parsing
fn format_tool_key(tool_name: &Option<String>, tool_input: &Option<Value>) -> String {
    let name = tool_name.as_deref().unwrap_or("unknown");

    if name == "Bash" {
        if let Some(input) = tool_input {
            if let Some(cmd) = input.get("command").and_then(|c| c.as_str()) {
                let normalized = normalize_bash_command(cmd);
                return format!("Bash:{}", normalized);
            }
        }
    }

    name.to_string()
}

/// Normalize Bash command for matching:
/// - Handle pipes: match against rightmost command (the "sink")
/// - Strip leading env vars (VAR=value patterns)
/// - Handle `env` command prefix
/// - Handle nested shells (bash -c, sh -c)
/// - Take meaningful prefix for matching
fn normalize_bash_command(cmd: &str) -> String {
    let cmd = cmd.trim();

    // HIGH FIX #1: Handle pipes - take rightmost command (the sink)
    // This handles: echo "y" | gh issue close 123
    // We want to match against "gh issue close 123"
    let cmd = if let Some(last_pipe) = find_last_unquoted_pipe(cmd) {
        cmd[last_pipe + 1..].trim()
    } else {
        cmd
    };

    // Handle `env` command prefix: env VAR=x cmd -> cmd
    let cmd = if cmd.starts_with("env ") {
        skip_env_command(&cmd[4..])
    } else {
        cmd
    };

    // Handle nested shells: bash -c "cmd" or sh -c "cmd"
    let cmd = extract_nested_shell_command(cmd).unwrap_or(cmd);

    // Strip leading environment variable assignments
    let cmd = strip_env_vars(cmd);

    // Take first 80 chars for matching
    cmd.chars().take(80).collect()
}

/// Find the last pipe character that's not inside quotes
fn find_last_unquoted_pipe(cmd: &str) -> Option<usize> {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut last_pipe = None;
    let mut prev_char = None;

    for (i, c) in cmd.char_indices() {
        match c {
            '\'' if !in_double_quote && prev_char != Some('\\') => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote && prev_char != Some('\\') => {
                in_double_quote = !in_double_quote;
            }
            '|' if !in_single_quote && !in_double_quote => {
                // Make sure it's not || (logical or)
                if prev_char != Some('|') {
                    last_pipe = Some(i);
                }
            }
            _ => {}
        }
        prev_char = Some(c);
    }

    last_pipe
}

/// Skip env command and its VAR=value arguments
fn skip_env_command(cmd: &str) -> &str {
    let mut rest = cmd.trim();
    // Skip VAR=value pairs after env
    while let Some(eq_pos) = rest.find('=') {
        let before_eq = &rest[..eq_pos];
        // Check if it's a valid VAR name (alphanumeric + underscore, no spaces)
        if before_eq.chars().all(|c| c.is_alphanumeric() || c == '_') {
            // Skip past the value
            rest = skip_value_str(&rest[eq_pos + 1..]).trim();
        } else {
            break;
        }
    }
    rest
}

/// Extract command from nested shell: "bash -c 'cmd'" -> "cmd"
fn extract_nested_shell_command(cmd: &str) -> Option<&str> {
    let shells = ["bash -c ", "sh -c ", "/bin/bash -c ", "/bin/sh -c "];

    for shell in shells {
        if cmd.starts_with(shell) {
            let rest = cmd[shell.len()..].trim();
            // Extract quoted command
            if rest.starts_with('"') {
                return rest.get(1..rest.len()-1); // Strip quotes
            } else if rest.starts_with('\'') {
                return rest.get(1..rest.len()-1);
            }
            return Some(rest);
        }
    }
    None
}

/// Strip leading VAR=value assignments from command
fn strip_env_vars(cmd: &str) -> &str {
    let mut rest = cmd.trim();

    loop {
        // Look for pattern: WORD= at start
        let word_end = rest.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(rest.len());
        if word_end == 0 {
            break;
        }

        let after_word = &rest[word_end..];
        if after_word.starts_with('=') {
            // It's VAR=, skip the value
            rest = skip_value_str(&after_word[1..]).trim();
        } else {
            // It's the actual command
            break;
        }
    }

    rest
}

/// Skip a value (quoted or unquoted) and return the rest
fn skip_value_str(s: &str) -> &str {
    let s = s.trim_start();
    if s.starts_with('"') {
        // Find closing quote (handling escapes)
        let mut prev = None;
        for (i, c) in s[1..].char_indices() {
            if c == '"' && prev != Some('\\') {
                return &s[i + 2..];
            }
            prev = Some(c);
        }
        s // Unclosed quote, return as-is
    } else if s.starts_with('\'') {
        // Find closing quote (no escapes in single quotes)
        if let Some(end) = s[1..].find('\'') {
            return &s[end + 2..];
        }
        s
    } else {
        // Unquoted: ends at whitespace
        s.split_whitespace().skip(1).next().map(|_| {
            let space = s.find(char::is_whitespace).unwrap_or(s.len());
            &s[space..]
        }).unwrap_or("")
    }
}

/// Find matching pattern, returns first match.
/// NOTE: Pattern order matters! More specific patterns should come first.
fn find_matching_pattern(tool_key: &str, patterns: &[String]) -> Option<String> {
    patterns.iter()
        .find(|p| glob_match(p, tool_key))
        .cloned()
}
```

**Pattern matching notes:**

| Pattern | Matches | Does NOT match |
|---------|---------|----------------|
| `mcp__tissue__close*` | `mcp__tissue__close_issue` | `mcp__tissue__archive` |
| `mcp__tissue__*` | All tissue tools | Other MCP servers |
| `Bash:gh issue close*` | `gh issue close 123` | `gh pr merge` |
| `Bash:gh *` | All gh commands | Other bash commands |

**Pattern order example:**

```toml
# Correct: specific patterns first
tools = [
    "mcp__tissue__close*",   # Specific: only close operations
    "mcp__tissue__*",        # Fallback: catch other tissue ops
]

# Wrong: general pattern shadows specific
tools = [
    "mcp__tissue__*",        # Matches everything - specific never reached
    "mcp__tissue__close*",   # Never matched!
]
```

**Helper constructors:**

```rust
impl PreToolUseOutput {
    pub fn allow() -> Self {
        Self {
            hook_specific_output: PreToolUseDecision {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: PermissionDecision::Allow,
                reason: None,
                updated_input: None,
            },
        }
    }

    pub fn deny(reason: &str) -> Self {
        Self {
            hook_specific_output: PreToolUseDecision {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: PermissionDecision::Deny,
                reason: Some(reason.to_string()),
                updated_input: None,
            },
        }
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

When triggered by a gate (no user prompts with `#roz`):

```bash
$ roz context xyz789-uvw012

Session: xyz789-uvw012
Created: 2026-01-31T14:00:00Z

Gate trigger:
  Tool: mcp__tissue__close_issue
  Pattern: mcp__tissue__close*
  Time: 2026-01-31T14:05:00Z
  Input:
    {
      "issue_id": "PROJ-123",
      "resolution": "fixed"
    }

User prompts:
[1] 2026-01-31T14:00:00Z
    Close PROJ-123, the auth bug is fixed
```

This gives roz full context about what tool the agent was trying to use and why.

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

    let now = Utc::now();
    let is_complete = decision.to_uppercase() == "COMPLETE";

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
        timestamp: now,
    });

    state.review.decision = new_decision;

    // Track when gate was approved (for approval_scope tracking)
    if is_complete {
        state.review.gate_approved_at = Some(now);
    }

    state.updated_at = now;

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

---
SESSION_ID={{session_id}}

## Summary
[What you did and why]

## Files Changed
[List of modified files]
---
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

# Gate configuration: auto-trigger review before specific tools
# Gates are enabled when tools array is non-empty
[review.gates]
# Tool patterns to gate (glob syntax)
# Examples:
#   "mcp__tissue__close*"     - tissue close/resolve tools
#   "mcp__beads__complete*"   - beads completion tools
#   "Bash:gh issue close*"    - GitHub CLI issue close
#   "Bash:gh pr merge*"       - GitHub CLI PR merge
tools = [
    "mcp__tissue__close*",
    "mcp__tissue__resolve*",
    "mcp__beads__close*",
    "mcp__beads__complete*",
]

# How long does gate approval last?
#   "session" - once approved, all gated tools allowed until session ends
#   "prompt"  - approval resets when user sends a new prompt (recommended)
#   "tool"    - every gated tool call requires fresh review
approval_scope = "prompt"

# Optional: approval expires after this many seconds (applies to all scopes)
# Useful to ensure stale approvals don't persist across session resumes
# approval_ttl_seconds = 3600  # 1 hour

[trace]
# Maximum trace events per session (oldest dropped when exceeded)
max_events = 500

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

### 7.3 Configuration Types

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub storage: StorageConfig,
    pub review: ReviewConfig,
    pub circuit_breaker: CircuitBreakerConfig,
    pub cleanup: CleanupConfig,
    pub external_models: ExternalModelsConfig,
    pub templates: TemplateConfig,
    pub trace: TraceConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TraceConfig {
    /// Maximum trace events per session (default: 500)
    /// When exceeded, oldest events are dropped (keeping first 10 for context)
    #[serde(default = "default_max_events")]
    pub max_events: usize,
}

fn default_max_events() -> usize { 500 }

impl Default for TraceConfig {
    fn default() -> Self {
        Self { max_events: default_max_events() }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewConfig {
    pub mode: ReviewMode,
    pub gates: GatesConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GatesConfig {
    /// Tool patterns to gate (glob syntax)
    /// Gates are enabled when this array is non-empty.
    /// NOTE: Order matters! First matching pattern wins.
    /// More specific patterns should come before general ones.
    #[serde(default)]
    pub tools: Vec<String>,

    /// How long does gate approval last?
    #[serde(default)]
    pub approval_scope: ApprovalScope,

    /// Optional TTL for approvals in seconds (applies to all scopes)
    /// When set, approvals expire after this duration regardless of scope
    #[serde(default)]
    pub approval_ttl_seconds: Option<u64>,
}

impl GatesConfig {
    /// Gates are enabled when tools array is non-empty
    pub fn is_enabled(&self) -> bool {
        !self.tools.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalScope {
    /// Once approved, all gated tools allowed until session ends
    Session,

    /// Approval resets when user sends a new prompt (recommended)
    #[default]
    Prompt,

    /// Every gated tool call requires fresh review
    Tool,
}

```

### 7.4 Second Opinion Fallback

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
| **Gate bypass via tool rename** | Medium | Gate patterns should be broad; document that custom MCP tools need explicit patterns |
| **Session ID as bearer token** | Medium | Acceptable for local use; session files have user permissions |
| **Same model family fallback** | Medium | Opus fallback has correlated blind spots; documented as known limitation |
| **Template file missing** | Medium | Hardcoded default template fallback |
| **Active session cleaned** | Medium | Skip sessions with `Decision::Pending` |
| **Gate approval persists too long** | Low | `skip_if_approved` can be disabled; new `#roz` prompt resets to Pending |
| **Second opinions disagree** | Low | Agent instructions: err on side of ISSUES when opinions conflict |
| **Disk/permission errors** | Low | Fail open with warning |

---

## 9. Plugin Integration

### 9.1 hooks.json

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "mcp__tissue__.*",
        "hooks": [{
          "type": "command",
          "command": "roz hook pre-tool-use",
          "timeout": 5
        }]
      },
      {
        "matcher": "mcp__beads__.*",
        "hooks": [{
          "type": "command",
          "command": "roz hook pre-tool-use",
          "timeout": 5
        }]
      },
      {
        "matcher": "Bash",
        "hooks": [{
          "type": "command",
          "command": "roz hook pre-tool-use",
          "timeout": 5
        }]
      }
    ],
    "Stop": [{
      "hooks": [{
        "type": "command",
        "command": "roz hook stop",
        "timeout": 30
      }]
    }],
    "SubagentStop": [{
      "matcher": "roz:roz",
      "hooks": [{
        "type": "command",
        "command": "roz hook subagent-stop",
        "timeout": 5
      }]
    }],
    "UserPromptSubmit": [{
      "hooks": [{
        "type": "command",
        "command": "roz hook user-prompt",
        "timeout": 5
      }]
    }]
  }
}
```

**Note:** The `PreToolUse` matchers are broad (matching all tissue/beads tools and all Bash commands). The actual filtering is done in the hook handler based on `config.review.gates.tools` patterns. This allows runtime configuration without plugin reinstallation.

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
