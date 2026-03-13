//! Hook handler implementations.

use crate::config::{ApprovalScope, CircuitBreakerConfig, Config, GatesConfig};
use crate::core::circuit_breaker;
use crate::core::state::{
    AttemptOutcome, Decision, EventType, GateTrigger, ReviewAttempt, SessionState, TraceEvent,
    TruncatedInput,
};
use crate::hooks::{HookInput, HookOutput, PreToolUseOutput};
use crate::storage::MessageStore;
use crate::template::{load_template, select_template};
use chrono::{Duration, Utc};
use glob::Pattern;
use serde_json::{Value, json};
use std::process::Command;
use uuid::Uuid;

/// Generate a unique ID for trace events.
fn generate_id() -> String {
    Uuid::new_v4().to_string()
}

/// Maximum size for stored user prompts (10KB).
const MAX_PROMPT_SIZE: usize = 10 * 1024;

/// Truncate a prompt to avoid storing excessively large strings.
fn truncate_prompt(prompt: &str) -> String {
    if prompt.len() <= MAX_PROMPT_SIZE {
        prompt.to_string()
    } else {
        // Truncate at a character boundary
        let truncate_at = prompt
            .char_indices()
            .take_while(|(i, _)| *i < MAX_PROMPT_SIZE)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        format!(
            "{}... [truncated, original {} bytes]",
            &prompt[..truncate_at],
            prompt.len()
        )
    }
}

/// Handle the session-start hook.
///
/// Initializes session state and detects available second opinion sources.
pub fn handle_session_start(input: &HookInput, store: &dyn MessageStore) -> HookOutput {
    let session_id = &input.session_id;

    // Get or create session state
    let state = match store.get_session(session_id) {
        Ok(Some(s)) => s, // Resume existing session
        Ok(None) => {
            // New session
            let mut state = SessionState::new(session_id);
            state.trace.push(TraceEvent {
                id: generate_id(),
                timestamp: Utc::now(),
                event_type: EventType::SessionStart,
                payload: json!({
                    "source": input.source,
                    "cwd": input.cwd,
                }),
            });
            state
        }
        Err(e) => {
            eprintln!("roz: warning: storage error: {e}");
            return HookOutput::approve(); // Fail open
        }
    };

    // Save state
    if let Err(e) = store.put_session(&state) {
        eprintln!("roz: warning: failed to save state: {e}");
    }

    // Optionally inject context about available second opinion sources
    match detect_second_opinion_context() {
        Some(ctx) => HookOutput::approve_with_context("SessionStart", &ctx),
        None => HookOutput::approve(),
    }
}

/// Handle the session-end hook.
///
/// Records a `SessionEnd` trace event and saves state.
/// This hook has no decision control - it always approves.
pub fn handle_session_end(input: &HookInput, store: &dyn MessageStore) -> HookOutput {
    let session_id = &input.session_id;

    // Get existing session - if not found, nothing to do
    let mut state = match store.get_session(session_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            eprintln!("roz: warning: session-end for unknown session: {session_id}");
            return HookOutput::approve(); // Fail open
        }
        Err(e) => {
            eprintln!("roz: warning: storage error: {e}");
            return HookOutput::approve(); // Fail open
        }
    };

    let reason = input.reason.as_deref().unwrap_or("unknown");

    // Add SessionEnd trace event
    state.trace.push(TraceEvent {
        id: generate_id(),
        timestamp: Utc::now(),
        event_type: EventType::SessionEnd,
        payload: json!({
            "reason": reason,
            "cwd": input.cwd,
        }),
    });

    // Save state
    if let Err(e) = store.put_session(&state) {
        eprintln!("roz: warning: failed to save state: {e}");
    }

    HookOutput::approve()
}

/// Detect available second opinion sources.
fn detect_second_opinion_context() -> Option<String> {
    let codex = command_exists("codex");
    let gemini = command_exists("gemini");

    if codex || gemini {
        Some(format!(
            "roz second opinion sources: {}{}",
            if codex { "codex " } else { "" },
            if gemini { "gemini" } else { "" }
        ))
    } else {
        None // Fall back to Claude Opus (always available)
    }
}

/// Check if a command exists in PATH.
fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Handle the user-prompt hook.
///
/// Detects `#roz` prefix to enable review and stores the prompt.
pub fn handle_user_prompt(input: &HookInput, store: &dyn MessageStore) -> HookOutput {
    handle_user_prompt_with_config(input, store, &Config::default())
}

/// Handle the user-prompt hook with explicit config.
///
/// Detects `#roz` prefix or `ReviewMode::Always` to enable review and stores the prompt.
pub fn handle_user_prompt_with_config(
    input: &HookInput,
    store: &dyn MessageStore,
    config: &Config,
) -> HookOutput {
    use crate::config::ReviewMode;

    let session_id = &input.session_id;
    let prompt = input.prompt.as_deref().unwrap_or("");

    // Get or create session state
    let mut state = match store.get_session(session_id) {
        Ok(Some(s)) => s,
        Ok(None) => SessionState::new(session_id),
        Err(e) => {
            eprintln!("roz: warning: storage error: {e}");
            return HookOutput::approve(); // Fail open
        }
    };

    let now = Utc::now();

    // Always track last prompt time
    state.review.last_prompt_at = Some(now);

    // Check if review should be enabled:
    // 1. ReviewMode::Always enables review for all prompts
    // 2. #roz prefix enables review for this prompt
    // 3. ReviewMode::Never disables all review (skip even #roz prefix)
    let should_enable = match config.review.mode {
        ReviewMode::Always => true,
        ReviewMode::Never => false,
        ReviewMode::Prompt => prompt.trim_start().starts_with("#roz"),
    };

    if should_enable {
        state.review.enabled = true;
        state.review.user_prompts.push(truncate_prompt(prompt));
        state.review.decision = Decision::Pending; // Reset for new review

        state.trace.push(TraceEvent {
            id: generate_id(),
            timestamp: now,
            event_type: EventType::PromptReceived,
            payload: json!({ "prompt": prompt }),
        });
    }

    state.updated_at = now;

    // Save state
    if let Err(e) = store.put_session(&state) {
        eprintln!("roz: warning: failed to save state: {e}");
    }

    HookOutput::approve()
}

/// Handle the stop hook.
///
/// Blocks if review is enabled and pending.
/// Includes circuit breaker logic to prevent infinite loops.
pub fn handle_stop(input: &HookInput, store: &dyn MessageStore) -> HookOutput {
    handle_stop_with_config(input, store, &Config::default())
}

/// Handle the stop hook with explicit config.
///
/// Blocks if review is enabled and pending.
/// Includes circuit breaker logic to prevent infinite loops.
pub fn handle_stop_with_config(
    input: &HookInput,
    store: &dyn MessageStore,
    config: &Config,
) -> HookOutput {
    let session_id = &input.session_id;

    // Get session state
    let mut state = match store.get_session(session_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            // No session state - review not enabled
            return HookOutput::approve();
        }
        Err(e) => {
            eprintln!("roz: warning: storage error: {e}");
            return HookOutput::approve(); // Fail open
        }
    };

    let now = Utc::now();

    // Defense-in-depth: when stop_hook_active is true, Claude Code is telling us
    // we're in a block-continue loop. Reduce effective max_blocks by 1 (floor 1)
    // to trip the circuit breaker one block sooner.
    let stop_hook_active = input.stop_hook_active.unwrap_or(false);
    let effective_cb = if stop_hook_active {
        CircuitBreakerConfig {
            max_blocks: config.circuit_breaker.max_blocks.saturating_sub(1).max(1),
            ..config.circuit_breaker.clone()
        }
    } else {
        config.circuit_breaker.clone()
    };

    // Log the stop hook call (include stop_hook_active and effective_max_blocks)
    state.trace.push(TraceEvent {
        id: generate_id(),
        timestamp: now,
        event_type: EventType::StopHookCalled,
        payload: json!({
            "stop_hook_active": stop_hook_active,
            "effective_max_blocks": effective_cb.max_blocks,
        }),
    });

    // Check if review is enabled
    if !state.review.enabled {
        state.updated_at = now;
        let _ = store.put_session(&state);
        return HookOutput::approve();
    }

    // Check circuit breaker BEFORE incrementing block count
    // If previously tripped but cooldown elapsed, reset the circuit breaker
    if state.review.circuit_breaker_tripped && !circuit_breaker::should_trip(&state, &effective_cb)
    {
        circuit_breaker::reset(&mut state);
    } else if circuit_breaker::should_trip(&state, &effective_cb) {
        circuit_breaker::trip(&mut state);
        state.updated_at = now;
        let _ = store.put_session(&state);
        return HookOutput::approve();
    }

    // Check decision - clone any needed data before mutable operations
    let output = match &state.review.decision {
        Decision::Pending => {
            // Block and request review
            state.review.block_count += 1;

            // Check circuit breaker AFTER incrementing
            if circuit_breaker::should_trip(&state, &effective_cb) {
                circuit_breaker::trip(&mut state);
                state.updated_at = now;
                let _ = store.put_session(&state);
                return HookOutput::approve();
            }

            // Select template (supports A/B testing via random selection)
            let template_id = select_template(&config.templates);
            record_review_attempt(&mut state, &template_id);

            let template = load_template(&template_id);
            let message = template.replace("{{session_id}}", session_id);

            HookOutput::block(&message)
        }
        Decision::Complete { .. } => {
            // Work approved
            HookOutput::approve()
        }
        Decision::Issues {
            summary,
            message_to_agent,
        } => {
            // Use message_to_agent if provided, otherwise fall back to summary
            let msg = message_to_agent.clone().unwrap_or_else(|| summary.clone());

            state.review.block_count += 1;

            // Check circuit breaker AFTER incrementing
            if circuit_breaker::should_trip(&state, &effective_cb) {
                circuit_breaker::trip(&mut state);
                state.updated_at = now;
                let _ = store.put_session(&state);
                return HookOutput::approve();
            }

            // Record attempt for issues re-review
            let template_id = select_template(&config.templates);
            record_review_attempt(&mut state, &template_id);

            HookOutput::block(&format!(
                "Review found issues that need to be addressed:\n\n{msg}\n\n\
                 After fixing, spawn roz:roz again to re-review."
            ))
        }
    };

    state.updated_at = now;
    let _ = store.put_session(&state);

    output
}

/// Handle the subagent-stop hook.
///
/// Validates that roz:roz posted a decision during its execution.
/// Uses the parent session's `session_id` from the hook input directly,
/// since `SubagentStop` fires in the context of the parent session.
pub fn handle_subagent_stop(input: &HookInput, store: &dyn MessageStore) -> HookOutput {
    // Only validate roz:roz subagent
    match &input.agent_type {
        Some(t) if t == "roz:roz" => {}
        _ => return HookOutput::approve(),
    }

    let session_id = &input.session_id;

    // Check if roz recorded a decision
    let state = match store.get_session(session_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            eprintln!("roz: warning: session {session_id} not found");
            return HookOutput::approve(); // Fail open - can't verify
        }
        Err(e) => {
            eprintln!("roz: warning: storage error: {e}");
            return HookOutput::approve(); // Fail open
        }
    };

    if input.stop_hook_active.unwrap_or(false) {
        eprintln!("roz: info: subagent-stop for {session_id} with stop_hook_active=true");
    }

    match &state.review.decision {
        Decision::Pending => HookOutput::block(&format!(
            "roz:roz completed but did not record a decision.\n\n\
             Run: roz decide {session_id} COMPLETE \"summary\"\n\
              or: roz decide {session_id} ISSUES \"summary\" --message \"what to fix\""
        )),
        Decision::Complete { .. } | Decision::Issues { .. } => {
            // Verify decision was posted during the current review cycle.
            // Lower bound: most recent block attempt (stop hook) or review start (gate).
            // Upper bound: now + 5s clock-skew buffer.
            let decision_time = state.updated_at;

            let lower_bound = state
                .review
                .attempts
                .last()
                .map(|a| a.timestamp)
                .or(state.review.review_started_at);

            if let Some(lower) = lower_bound {
                if decision_time < lower {
                    return HookOutput::block(&format!(
                        "Decision timestamp ({}) is before the current review cycle ({}). \
                         Decision must be posted by roz:roz during its execution.",
                        decision_time.format("%Y-%m-%dT%H:%M:%SZ"),
                        lower.format("%Y-%m-%dT%H:%M:%SZ")
                    ));
                }
            }

            // Upper bound: now + clock skew buffer
            let buffer = Duration::seconds(5);
            let now = Utc::now();
            if decision_time > now + buffer {
                return HookOutput::block(&format!(
                    "Decision timestamp ({}) is in the future (now: {}). \
                     Decision must be posted by roz:roz during its execution.",
                    decision_time.format("%Y-%m-%dT%H:%M:%SZ"),
                    now.format("%Y-%m-%dT%H:%M:%SZ")
                ));
            }

            HookOutput::approve()
        }
    }
}

// ============================================================================
// Pre-Tool-Use Hook (Gates)
// ============================================================================

/// Handle the pre-tool-use hook.
///
/// Checks if the tool matches a gate pattern and blocks if review is needed.
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
    let tool_key = format_tool_key(input.tool_name.as_deref(), input.tool_input.as_ref());

    // Check if tool matches any gate pattern
    let Some(matched_pattern) = find_matching_pattern(&tool_key, &config.review.gates.tools) else {
        return PreToolUseOutput::allow();
    };

    // Get or create session state
    let mut state = match store.get_session(&input.session_id) {
        Ok(Some(s)) => s,
        Ok(None) => SessionState::new(&input.session_id),
        Err(e) => {
            eprintln!("roz: warning: storage error: {e}");
            return PreToolUseOutput::allow(); // Fail open
        }
    };

    // Check circuit breaker - if tripped, allow through
    if state.review.circuit_breaker_tripped {
        trace_gate_allowed(
            &mut state,
            &tool_key,
            "circuit_breaker",
            config.trace.max_events,
        );
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
    state.review.review_started_at = Some(now); // Mark review cycle start
    state.review.gate_trigger = Some(GateTrigger {
        tool_name: tool_key.clone(),
        tool_input: TruncatedInput::from_value(input.tool_input.clone().unwrap_or(Value::Null)),
        triggered_at: now,
        pattern_matched: matched_pattern.clone(),
    });

    // Add trace event (with limiting)
    add_trace_event(
        &mut state,
        TraceEvent {
            id: generate_id(),
            timestamp: now,
            event_type: EventType::GateBlocked,
            payload: json!({
                "tool": tool_key,
                "pattern": matched_pattern,
            }),
        },
        config.trace.max_events,
    );

    state.updated_at = now;

    if let Err(e) = store.put_session(&state) {
        eprintln!("roz: warning: failed to save state: {e}");
        return PreToolUseOutput::allow(); // Fail open
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

/// Check if gate is approved based on configured scope.
fn is_gate_approved(state: &SessionState, gates: &GatesConfig) -> bool {
    // Must have a Complete decision
    if !matches!(state.review.decision, Decision::Complete { .. }) {
        return false;
    }

    let Some(approved_at) = state.review.gate_approved_at else {
        return false; // Never approved
    };

    // Check TTL expiry (applies to all scopes)
    if let Some(ttl_secs) = gates.approval_ttl_seconds {
        let ttl_secs_i64 = i64::try_from(ttl_secs).unwrap_or(i64::MAX);
        let expiry = approved_at + Duration::seconds(ttl_secs_i64);
        if Utc::now() > expiry {
            return false; // Approval expired
        }
    }

    match gates.approval_scope {
        ApprovalScope::Session => true, // Any non-expired approval is valid

        ApprovalScope::Prompt => {
            // Approval must be after the last user prompt
            // BUT: ignore prompts that arrived during an active review cycle
            let effective_prompt_at =
                match (state.review.last_prompt_at, state.review.review_started_at) {
                    (Some(prompt), Some(review_start)) if prompt > review_start => {
                        // Prompt arrived during review - use review_started_at instead
                        // This prevents "hurry up" prompts from invalidating pending approval
                        Some(review_start)
                    }
                    (prompt_at, _) => prompt_at,
                };

            match effective_prompt_at {
                Some(prompt) => approved_at > prompt,
                None => true, // No prompts yet, approval valid
            }
        }

        ApprovalScope::Tool => false, // Every tool call needs fresh review
    }
}

/// Trace when gate allows (for debugging visibility).
fn trace_gate_allowed(state: &mut SessionState, tool: &str, reason: &str, max_events: usize) {
    add_trace_event(
        state,
        TraceEvent {
            id: generate_id(),
            timestamp: Utc::now(),
            event_type: EventType::GateAllowed,
            payload: json!({
                "tool": tool,
                "reason": reason,
            }),
        },
        max_events,
    );
}

/// Add trace event with size limiting (drops oldest events if over limit).
fn add_trace_event(state: &mut SessionState, event: TraceEvent, max_events: usize) {
    state.trace.push(event);

    // Enforce limit by dropping oldest events (but keep first 10 for context)
    if state.trace.len() > max_events {
        let keep_start = 10.min(max_events / 2);
        let keep_end = max_events - keep_start - 1; // -1 for compaction marker

        // Keep first `keep_start` and last `keep_end` events
        let total = state.trace.len();
        let dropped = total - max_events;

        let mut new_trace = Vec::with_capacity(max_events);
        new_trace.extend(state.trace.drain(..keep_start));
        new_trace.push(TraceEvent {
            id: generate_id(),
            timestamp: Utc::now(),
            event_type: EventType::TraceCompacted,
            payload: json!({
                "dropped_events": dropped,
                "kept_start": keep_start,
                "kept_end": keep_end,
            }),
        });

        // Drain remaining events, keeping only the last `keep_end`
        let remaining = state.trace.len();
        if remaining > keep_end {
            state.trace.drain(..remaining - keep_end);
        }
        new_trace.append(&mut state.trace);

        state.trace = new_trace;
    }
}

// ============================================================================
// Tool Key Formatting and Bash Command Normalization
// ============================================================================

/// Format a tool key for pattern matching.
///
/// For Bash tools, normalizes the command and prefixes with `Bash:`.
fn format_tool_key(tool_name: Option<&str>, tool_input: Option<&Value>) -> String {
    let name = tool_name.unwrap_or("unknown");

    if name == "Bash" {
        if let Some(input) = tool_input {
            if let Some(cmd) = input.get("command").and_then(|c| c.as_str()) {
                let normalized = normalize_bash_command(cmd);
                return format!("Bash:{normalized}");
            }
        }
    }

    name.to_string()
}

/// Normalize a Bash command for matching.
///
/// - Handle pipes: match against rightmost command (the "sink")
/// - Strip leading env vars (`VAR=value` patterns)
/// - Handle `env` command prefix
/// - Handle nested shells (`bash -c`, `sh -c`)
/// - Take meaningful prefix for matching
fn normalize_bash_command(cmd: &str) -> String {
    let cmd = cmd.trim();

    // Handle pipes - take rightmost command (the sink)
    // This handles: echo "y" | gh issue close 123
    // We want to match against "gh issue close 123"
    let cmd = if let Some(last_pipe) = find_last_unquoted_pipe(cmd) {
        cmd[last_pipe + 1..].trim()
    } else {
        cmd
    };

    // Handle `env` command prefix: env VAR=x cmd -> cmd
    let cmd = if let Some(stripped) = cmd.strip_prefix("env ") {
        skip_env_command(stripped)
    } else {
        cmd
    };

    // Strip leading environment variable assignments (before nested shell check)
    // This handles: FOO=1 bash -c "cmd" -> bash -c "cmd"
    let cmd = strip_env_vars(cmd);

    // Handle nested shells: bash -c "cmd" or sh -c "cmd"
    let cmd = extract_nested_shell_command(cmd).unwrap_or(cmd);

    // Take first 80 chars for matching
    cmd.chars().take(80).collect()
}

/// Find the last pipe character that's not inside quotes.
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

/// Skip `env` command and its `VAR=value` arguments.
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

/// Extract command from nested shell: `bash -c 'cmd'` -> `cmd`.
fn extract_nested_shell_command(cmd: &str) -> Option<&str> {
    let shells = ["bash -c ", "sh -c ", "/bin/bash -c ", "/bin/sh -c "];

    for shell in shells {
        if let Some(stripped) = cmd.strip_prefix(shell) {
            let rest = stripped.trim();
            // Extract quoted command - find matching closing quote
            if let Some(inner) = rest.strip_prefix('"') {
                // Find closing double quote
                if let Some(close_pos) = inner.find('"') {
                    return Some(&inner[..close_pos]);
                }
                // No closing quote found, return as-is without the opening quote
                return Some(inner);
            } else if let Some(inner) = rest.strip_prefix('\'') {
                // Find closing single quote
                if let Some(close_pos) = inner.find('\'') {
                    return Some(&inner[..close_pos]);
                }
                // No closing quote found, return as-is without the opening quote
                return Some(inner);
            }
            return Some(rest);
        }
    }
    None
}

/// Strip leading `VAR=value` assignments from command.
fn strip_env_vars(cmd: &str) -> &str {
    let mut rest = cmd.trim();

    loop {
        // Look for pattern: WORD= at start
        let word_end = rest
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        if word_end == 0 {
            break;
        }

        let after_word = &rest[word_end..];
        if let Some(stripped) = after_word.strip_prefix('=') {
            // It's VAR=, skip the value
            rest = skip_value_str(stripped).trim();
        } else {
            // It's the actual command
            break;
        }
    }

    rest
}

/// Skip a value (quoted or unquoted) and return the rest.
fn skip_value_str(s: &str) -> &str {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('"') {
        // Find closing quote (handling escapes)
        let mut prev = None;
        for (i, c) in rest.char_indices() {
            if c == '"' && prev != Some('\\') {
                return &rest[i + 1..];
            }
            prev = Some(c);
        }
        s // Unclosed quote, return as-is
    } else if let Some(rest) = s.strip_prefix('\'') {
        // Find closing quote (no escapes in single quotes)
        if let Some(end) = rest.find('\'') {
            return &rest[end + 1..];
        }
        s
    } else {
        // Unquoted: ends at whitespace
        match s.find(char::is_whitespace) {
            Some(space) => &s[space..],
            None => "",
        }
    }
}

/// Find matching pattern, returns first match.
///
/// NOTE: Pattern order matters! More specific patterns should come first.
fn find_matching_pattern(tool_key: &str, patterns: &[String]) -> Option<String> {
    patterns.iter().find(|p| glob_match(p, tool_key)).cloned()
}

/// Match a tool key against a glob pattern.
fn glob_match(pattern: &str, tool_key: &str) -> bool {
    match Pattern::new(pattern) {
        Ok(p) => p.matches(tool_key),
        Err(_) => {
            // Fall back to simple prefix match if glob parsing fails
            tool_key.starts_with(pattern.trim_end_matches('*'))
        }
    }
}

// ============================================================================
// Review Attempts (for future A/B testing)
// ============================================================================

/// Record a review attempt when blocking.
fn record_review_attempt(state: &mut SessionState, template_id: &str) {
    state.review.attempts.push(ReviewAttempt {
        template_id: template_id.to_string(),
        timestamp: Utc::now(),
        outcome: AttemptOutcome::Pending,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryBackend;
    use std::path::PathBuf;

    // User prompt hook tests

    #[test]
    fn user_prompt_roz_prefix_enables_review() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "test-123".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("#roz fix the bug".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_user_prompt(&input, &store);

        assert!(output.decision.is_none());

        let state = store.get_session("test-123").unwrap().unwrap();
        assert!(state.review.enabled);
        assert_eq!(state.review.user_prompts.len(), 1);
        assert_eq!(state.review.decision, Decision::Pending);
    }

    #[test]
    fn user_prompt_no_prefix_no_review() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "test-456".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("fix the bug".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_user_prompt(&input, &store);

        assert!(output.decision.is_none());

        let state = store.get_session("test-456").unwrap().unwrap();
        assert!(!state.review.enabled);
        assert!(state.review.user_prompts.is_empty());
    }

    #[test]
    fn user_prompt_already_enabled_stays_enabled() {
        let store = MemoryBackend::new();

        // First prompt enables review
        let input1 = HookInput {
            session_id: "test-789".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("#roz fix the bug".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };
        handle_user_prompt(&input1, &store);

        // Second prompt without #roz
        let input2 = HookInput {
            session_id: "test-789".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("also fix tests".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };
        handle_user_prompt(&input2, &store);

        let state = store.get_session("test-789").unwrap().unwrap();
        assert!(state.review.enabled);
        // Only the #roz prompt is stored
        assert_eq!(state.review.user_prompts.len(), 1);
    }

    // Stop hook tests

    #[test]
    fn stop_not_enabled_approves() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "test-no-review".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_stop(&input, &store);
        assert!(output.decision.is_none());
    }

    #[test]
    fn stop_enabled_pending_blocks() {
        let store = MemoryBackend::new();

        // Enable review
        let mut state = SessionState::new("test-block");
        state.review.enabled = true;
        state.review.decision = Decision::Pending;
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "test-block".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_stop(&input, &store);
        assert!(matches!(
            output.decision,
            Some(crate::hooks::HookDecision::Block)
        ));
        assert!(output.reason.is_some());

        // Check block_count incremented
        let updated = store.get_session("test-block").unwrap().unwrap();
        assert_eq!(updated.review.block_count, 1);
    }

    #[test]
    fn stop_enabled_complete_approves() {
        let store = MemoryBackend::new();

        // Set up session with Complete decision
        let mut state = SessionState::new("test-complete");
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "test-complete".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_stop(&input, &store);
        assert!(output.decision.is_none());
    }

    #[test]
    fn stop_enabled_issues_blocks() {
        let store = MemoryBackend::new();

        // Set up session with Issues decision
        let mut state = SessionState::new("test-issues");
        state.review.enabled = true;
        state.review.decision = Decision::Issues {
            summary: "Found bugs".to_string(),
            message_to_agent: Some("Fix the tests".to_string()),
        };
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "test-issues".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_stop(&input, &store);
        assert!(matches!(
            output.decision,
            Some(crate::hooks::HookDecision::Block)
        ));
        assert!(output.reason.unwrap().contains("Fix the tests"));
    }

    #[test]
    fn stop_hook_records_stop_hook_active_in_trace() {
        let store = MemoryBackend::new();

        // Create session with pending review
        let mut state = SessionState::new("test-trace-active");
        state.review.enabled = true;
        store.put_session(&state).unwrap();

        // Call stop with stop_hook_active = true
        let input = HookInput {
            session_id: "test-trace-active".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: Some(true),
            reason: None,
        };

        handle_stop(&input, &store);

        // Verify stop_hook_active is recorded in the trace event
        let state = store.get_session("test-trace-active").unwrap().unwrap();
        let stop_event = state
            .trace
            .iter()
            .find(|e| e.event_type == EventType::StopHookCalled)
            .expect("StopHookCalled trace event should exist");
        assert_eq!(stop_event.payload["stop_hook_active"], true);
    }

    #[test]
    fn stop_hook_records_stop_hook_active_false_when_not_set() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-trace-inactive");
        state.review.enabled = true;
        store.put_session(&state).unwrap();

        // Call stop with stop_hook_active = None (defaults to false in trace)
        let input = HookInput {
            session_id: "test-trace-inactive".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        handle_stop(&input, &store);

        let state = store.get_session("test-trace-inactive").unwrap().unwrap();
        let stop_event = state
            .trace
            .iter()
            .find(|e| e.event_type == EventType::StopHookCalled)
            .expect("StopHookCalled trace event should exist");
        assert_eq!(stop_event.payload["stop_hook_active"], false);
    }

    // Subagent stop hook tests

    #[test]
    fn subagent_stop_non_roz_approves() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "test-123".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("other:agent".to_string()),
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(output.decision.is_none());
    }

    #[test]
    fn subagent_stop_no_agent_type_approves() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "test-123".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(output.decision.is_none());
    }

    #[test]
    fn subagent_stop_session_not_found_approves() {
        let store = MemoryBackend::new();
        // Session "nonexistent" doesn't exist in store
        let input = HookInput {
            session_id: "nonexistent".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("roz:roz".to_string()),
            agent_id: Some("agent-abc".to_string()),
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        // Fail open when session not found
        let output = handle_subagent_stop(&input, &store);
        assert!(output.decision.is_none());
    }

    #[test]
    fn subagent_stop_decision_pending_blocks() {
        let store = MemoryBackend::new();
        let session_id = "test-pending";

        // Create session with pending decision
        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        state.review.decision = Decision::Pending;
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: session_id.to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("roz:roz".to_string()),
            agent_id: Some("agent-abc".to_string()),
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(
            output.decision,
            Some(crate::hooks::HookDecision::Block)
        ));
        assert!(output.reason.unwrap().contains("did not record a decision"));
    }

    #[test]
    fn subagent_stop_valid_decision_approves() {
        let store = MemoryBackend::new();
        let session_id = "test-valid";

        // Create session with valid decision timestamp
        let now = Utc::now();
        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        state.updated_at = now; // Decision made now
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: session_id.to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("roz:roz".to_string()),
            agent_id: Some("agent-abc".to_string()),
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(output.decision.is_none());
    }

    #[test]
    fn subagent_stop_stale_decision_blocks() {
        let store = MemoryBackend::new();
        let session_id = "test-stale";

        // Create session with a stale decision that predates the review cycle
        let now = Utc::now();
        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "Old decision".to_string(),
            second_opinions: None,
        };
        state.updated_at = now - Duration::hours(2); // Decision made 2 hours ago

        // The stop hook blocked 5 minutes ago (creating a ReviewAttempt)
        state.review.attempts.push(ReviewAttempt {
            timestamp: now - Duration::minutes(5),
            template_id: "default".to_string(),
            outcome: AttemptOutcome::Pending,
        });
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: session_id.to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("roz:roz".to_string()),
            agent_id: Some("agent-abc".to_string()),
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(
            output.decision,
            Some(crate::hooks::HookDecision::Block)
        ));
        assert!(
            output
                .reason
                .unwrap()
                .contains("before the current review cycle")
        );
    }

    #[test]
    fn subagent_stop_decision_after_block_approves() {
        let store = MemoryBackend::new();
        let session_id = "test-after-block";

        // Create session where decision was posted after the review cycle started
        let now = Utc::now();
        let block_time = now - Duration::minutes(5);
        let decide_time = now - Duration::minutes(1);

        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        state.updated_at = decide_time; // Decision posted after block

        // Stop hook blocked at block_time
        state.review.attempts.push(ReviewAttempt {
            timestamp: block_time,
            template_id: "default".to_string(),
            outcome: AttemptOutcome::Pending,
        });
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: session_id.to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("roz:roz".to_string()),
            agent_id: Some("agent-abc".to_string()),
            agent_transcript_path: None,
            last_assistant_message: Some("Review complete.".to_string()),
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(output.decision.is_none());
    }

    #[test]
    fn subagent_stop_gate_review_started_as_lower_bound() {
        let store = MemoryBackend::new();
        let session_id = "test-gate-lower";

        // Gate flow: review_started_at is set but no ReviewAttempts
        let now = Utc::now();
        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        state.review.review_started_at = Some(now - Duration::minutes(3));
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        // Decision posted BEFORE review started → should be rejected
        state.updated_at = now - Duration::minutes(10);
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: session_id.to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("roz:roz".to_string()),
            agent_id: Some("agent-abc".to_string()),
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(
            output.decision,
            Some(crate::hooks::HookDecision::Block)
        ));
        assert!(
            output
                .reason
                .unwrap()
                .contains("before the current review cycle")
        );
    }

    #[test]
    fn subagent_stop_future_decision_blocks() {
        let store = MemoryBackend::new();
        let session_id = "test-future";

        // Decision timestamp is far in the future (suspicious)
        let now = Utc::now();
        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "Future decision".to_string(),
            second_opinions: None,
        };
        state.updated_at = now + Duration::hours(1); // 1 hour in the future
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: session_id.to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("roz:roz".to_string()),
            agent_id: Some("agent-abc".to_string()),
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(
            output.decision,
            Some(crate::hooks::HookDecision::Block)
        ));
        assert!(output.reason.unwrap().contains("in the future"));
    }

    #[test]
    fn subagent_stop_no_lower_bound_approves_valid_decision() {
        let store = MemoryBackend::new();
        let session_id = "test-no-bound";

        // Session has a decision but no review attempts and no review_started_at
        // (edge case: decision posted without going through normal flow)
        let now = Utc::now();
        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        state.updated_at = now;
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: session_id.to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: Some("roz:roz".to_string()),
            agent_id: Some("agent-abc".to_string()),
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        // Without a lower bound, approves if decision exists and isn't in the future
        let output = handle_subagent_stop(&input, &store);
        assert!(output.decision.is_none());
    }

    // Session start hook tests

    #[test]
    fn session_start_creates_new_session() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "new-session".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: Some("startup".to_string()),
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_session_start(&input, &store);
        assert!(output.decision.is_none());

        let state = store.get_session("new-session").unwrap().unwrap();
        assert_eq!(state.session_id, "new-session");
        assert!(!state.trace.is_empty());
        assert_eq!(state.trace[0].event_type, EventType::SessionStart);
    }

    #[test]
    fn session_start_resumes_existing() {
        let store = MemoryBackend::new();

        // Create existing session
        let mut existing = SessionState::new("existing-session");
        existing.review.enabled = true;
        existing.review.user_prompts.push("#roz test".to_string());
        store.put_session(&existing).unwrap();

        let input = HookInput {
            session_id: "existing-session".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: Some("resume".to_string()),
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_session_start(&input, &store);
        assert!(output.decision.is_none());

        // Existing state should be preserved
        let state = store.get_session("existing-session").unwrap().unwrap();
        assert!(state.review.enabled);
        assert_eq!(state.review.user_prompts.len(), 1);
    }

    // Circuit breaker integration tests

    #[test]
    fn stop_circuit_breaker_trips_after_max_blocks() {
        let store = MemoryBackend::new();

        // Set up session with block_count at limit
        let mut state = SessionState::new("circuit-test");
        state.review.enabled = true;
        state.review.decision = Decision::Pending;
        state.review.block_count = 3; // At max_blocks
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "circuit-test".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let config = Config::default(); // max_blocks = 3
        let output = handle_stop_with_config(&input, &store, &config);

        // Should approve because circuit breaker tripped
        assert!(output.decision.is_none());

        // Check state was updated
        let updated = store.get_session("circuit-test").unwrap().unwrap();
        assert!(updated.review.circuit_breaker_tripped);
        assert!(!updated.review.enabled);
    }

    #[test]
    fn stop_circuit_breaker_already_tripped_approves() {
        let store = MemoryBackend::new();

        // Set up session with circuit breaker already tripped
        let mut state = SessionState::new("already-tripped");
        state.review.enabled = true;
        state.review.decision = Decision::Pending;
        state.review.circuit_breaker_tripped = true;
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "already-tripped".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let config = Config::default();
        let output = handle_stop_with_config(&input, &store, &config);

        assert!(output.decision.is_none());
    }

    #[test]
    fn stop_circuit_breaker_trips_on_issues() {
        let store = MemoryBackend::new();

        // Set up session with Issues decision at block limit
        let mut state = SessionState::new("issues-circuit");
        state.review.enabled = true;
        state.review.decision = Decision::Issues {
            summary: "Found bugs".to_string(),
            message_to_agent: Some("Fix tests".to_string()),
        };
        state.review.block_count = 2; // One below max, will hit on increment
        store.put_session(&state).unwrap();

        let mut config = Config::default();
        config.circuit_breaker.max_blocks = 3;

        let input = HookInput {
            session_id: "issues-circuit".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_stop_with_config(&input, &store, &config);

        // Should approve because circuit breaker tripped after increment
        assert!(output.decision.is_none());

        let updated = store.get_session("issues-circuit").unwrap().unwrap();
        assert!(updated.review.circuit_breaker_tripped);
    }

    // Pre-tool-use hook tests

    fn make_gate_config() -> Config {
        let mut config = Config::default();
        config.review.gates.tools = vec![
            "mcp__tissue__close*".to_string(),
            "Bash:gh issue close*".to_string(),
            "Bash:gh pr merge*".to_string(),
        ];
        config
    }

    #[test]
    fn pre_tool_use_no_gates_allows() {
        let store = MemoryBackend::new();
        let config = Config::default(); // No gates configured

        let input = HookInput {
            session_id: "test-no-gates".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: Some("mcp__tissue__close_issue".to_string()),
            tool_input: Some(json!({"issue_id": "123"})),
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_pre_tool_use(&input, &config, &store);
        assert_eq!(
            output.hook_specific_output.permission_decision,
            crate::hooks::PermissionDecision::Allow
        );
    }

    #[test]
    fn pre_tool_use_non_matching_tool_allows() {
        let store = MemoryBackend::new();
        let config = make_gate_config();

        let input = HookInput {
            session_id: "test-non-match".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: Some("mcp__tissue__list_issues".to_string()), // Not matching pattern
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_pre_tool_use(&input, &config, &store);
        assert_eq!(
            output.hook_specific_output.permission_decision,
            crate::hooks::PermissionDecision::Allow
        );
    }

    #[test]
    fn pre_tool_use_matching_tool_denies() {
        let store = MemoryBackend::new();
        let config = make_gate_config();

        let input = HookInput {
            session_id: "test-match".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: Some("mcp__tissue__close_issue".to_string()),
            tool_input: Some(json!({"issue_id": "123"})),
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_pre_tool_use(&input, &config, &store);
        assert_eq!(
            output.hook_specific_output.permission_decision,
            crate::hooks::PermissionDecision::Deny
        );
        assert!(output.hook_specific_output.reason.is_some());
        assert!(
            output
                .hook_specific_output
                .reason
                .unwrap()
                .contains("Review required")
        );

        // Check state was updated
        let state = store.get_session("test-match").unwrap().unwrap();
        assert!(state.review.enabled);
        assert!(state.review.gate_trigger.is_some());
        let trigger = state.review.gate_trigger.unwrap();
        assert_eq!(trigger.tool_name, "mcp__tissue__close_issue");
        assert_eq!(trigger.pattern_matched, "mcp__tissue__close*");
    }

    #[test]
    fn pre_tool_use_circuit_breaker_tripped_allows() {
        let store = MemoryBackend::new();
        let config = make_gate_config();

        // Set up session with circuit breaker tripped
        let mut state = SessionState::new("test-tripped");
        state.review.circuit_breaker_tripped = true;
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "test-tripped".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: Some("mcp__tissue__close_issue".to_string()),
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_pre_tool_use(&input, &config, &store);
        assert_eq!(
            output.hook_specific_output.permission_decision,
            crate::hooks::PermissionDecision::Allow
        );
    }

    #[test]
    fn pre_tool_use_approved_session_allows() {
        let store = MemoryBackend::new();
        let config = make_gate_config();

        // Set up session with approval
        let mut state = SessionState::new("test-approved");
        state.review.decision = Decision::Complete {
            summary: "Approved".to_string(),
            second_opinions: None,
        };
        state.review.gate_approved_at = Some(Utc::now());
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "test-approved".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: Some("mcp__tissue__close_issue".to_string()),
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_pre_tool_use(&input, &config, &store);
        assert_eq!(
            output.hook_specific_output.permission_decision,
            crate::hooks::PermissionDecision::Allow
        );
    }

    #[test]
    fn pre_tool_use_bash_command_normalized() {
        let store = MemoryBackend::new();
        let config = make_gate_config();

        let input = HookInput {
            session_id: "test-bash".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({"command": "gh issue close 123"})),
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_pre_tool_use(&input, &config, &store);
        assert_eq!(
            output.hook_specific_output.permission_decision,
            crate::hooks::PermissionDecision::Deny
        );
    }

    #[test]
    fn pre_tool_use_bash_piped_command() {
        let store = MemoryBackend::new();
        let config = make_gate_config();

        // echo "y" | gh issue close 123 -> should match "gh issue close"
        let input = HookInput {
            session_id: "test-pipe".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({"command": "echo 'y' | gh issue close 123"})),
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_pre_tool_use(&input, &config, &store);
        assert_eq!(
            output.hook_specific_output.permission_decision,
            crate::hooks::PermissionDecision::Deny
        );
    }

    // Bash normalization tests

    #[test]
    fn normalize_simple_command() {
        let normalized = normalize_bash_command("gh issue close 123");
        assert_eq!(normalized, "gh issue close 123");
    }

    #[test]
    fn normalize_piped_command() {
        let normalized = normalize_bash_command("echo 'y' | gh issue close 123");
        assert_eq!(normalized, "gh issue close 123");
    }

    #[test]
    fn normalize_env_prefix() {
        let normalized = normalize_bash_command("GH_TOKEN=abc gh issue close 123");
        assert_eq!(normalized, "gh issue close 123");
    }

    #[test]
    fn normalize_env_command() {
        let normalized = normalize_bash_command("env GH_TOKEN=abc gh issue close 123");
        assert_eq!(normalized, "gh issue close 123");
    }

    #[test]
    fn normalize_bash_c() {
        let normalized = normalize_bash_command("bash -c \"gh issue close 123\"");
        assert_eq!(normalized, "gh issue close 123");
    }

    #[test]
    fn normalize_multiple_pipes() {
        // With multiple pipes, take rightmost command
        let normalized = normalize_bash_command("cat file | grep foo | head -5");
        assert_eq!(normalized, "head -5");
    }

    #[test]
    fn normalize_pipe_with_env() {
        let normalized = normalize_bash_command("echo y | GH_TOKEN=abc gh issue close 123");
        assert_eq!(normalized, "gh issue close 123");
    }

    #[test]
    fn normalize_quoted_pipe_preserved() {
        // Pipe inside quotes should not be split
        let normalized = normalize_bash_command("echo 'hello | world'");
        assert_eq!(normalized, "echo 'hello | world'");
    }

    #[test]
    fn normalize_double_quoted_pipe_preserved() {
        // Pipe inside double quotes should not be split
        let normalized = normalize_bash_command("echo \"foo | bar\"");
        assert_eq!(normalized, "echo \"foo | bar\"");
    }

    #[test]
    fn normalize_sh_c_variant() {
        let normalized = normalize_bash_command("sh -c 'ls -la'");
        assert_eq!(normalized, "ls -la");
    }

    #[test]
    fn normalize_complex_env_chain() {
        // Multiple env vars
        let normalized = normalize_bash_command("FOO=1 BAR=2 BAZ=3 mycommand arg");
        assert_eq!(normalized, "mycommand arg");
    }

    #[test]
    fn normalize_env_with_nested_shell() {
        let normalized = normalize_bash_command("FOO=1 bash -c \"inner cmd\"");
        assert_eq!(normalized, "inner cmd");
    }

    // Glob matching tests

    #[test]
    fn glob_match_exact() {
        assert!(glob_match(
            "mcp__tissue__close_issue",
            "mcp__tissue__close_issue"
        ));
    }

    #[test]
    fn glob_match_wildcard() {
        assert!(glob_match(
            "mcp__tissue__close*",
            "mcp__tissue__close_issue"
        ));
        assert!(glob_match("mcp__tissue__close*", "mcp__tissue__close_all"));
        assert!(!glob_match(
            "mcp__tissue__close*",
            "mcp__tissue__list_issues"
        ));
    }

    #[test]
    fn glob_match_bash_prefix() {
        assert!(glob_match(
            "Bash:gh issue close*",
            "Bash:gh issue close 123"
        ));
        assert!(!glob_match("Bash:gh issue close*", "Bash:gh pr merge 123"));
    }

    // Approval scope tests

    #[test]
    fn is_gate_approved_session_scope() {
        let mut state = SessionState::new("test-session-scope");
        state.review.decision = Decision::Complete {
            summary: "Done".to_string(),
            second_opinions: None,
        };
        state.review.gate_approved_at = Some(Utc::now() - Duration::hours(1));

        let gates = GatesConfig {
            tools: vec!["test*".to_string()],
            approval_scope: ApprovalScope::Session,
            approval_ttl_seconds: None,
        };

        assert!(is_gate_approved(&state, &gates));
    }

    #[test]
    fn is_gate_approved_prompt_scope_valid() {
        let mut state = SessionState::new("test-prompt-scope");
        state.review.decision = Decision::Complete {
            summary: "Done".to_string(),
            second_opinions: None,
        };
        // Approval is after the last prompt
        state.review.last_prompt_at = Some(Utc::now() - Duration::hours(2));
        state.review.gate_approved_at = Some(Utc::now() - Duration::hours(1));

        let gates = GatesConfig {
            tools: vec!["test*".to_string()],
            approval_scope: ApprovalScope::Prompt,
            approval_ttl_seconds: None,
        };

        assert!(is_gate_approved(&state, &gates));
    }

    #[test]
    fn is_gate_approved_prompt_scope_invalid() {
        let mut state = SessionState::new("test-prompt-scope");
        state.review.decision = Decision::Complete {
            summary: "Done".to_string(),
            second_opinions: None,
        };
        // Approval is before the last prompt (new prompt came in)
        state.review.gate_approved_at = Some(Utc::now() - Duration::hours(2));
        state.review.last_prompt_at = Some(Utc::now() - Duration::hours(1));

        let gates = GatesConfig {
            tools: vec!["test*".to_string()],
            approval_scope: ApprovalScope::Prompt,
            approval_ttl_seconds: None,
        };

        assert!(!is_gate_approved(&state, &gates));
    }

    #[test]
    fn is_gate_approved_tool_scope() {
        let mut state = SessionState::new("test-tool-scope");
        state.review.decision = Decision::Complete {
            summary: "Done".to_string(),
            second_opinions: None,
        };
        state.review.gate_approved_at = Some(Utc::now());

        let gates = GatesConfig {
            tools: vec!["test*".to_string()],
            approval_scope: ApprovalScope::Tool,
            approval_ttl_seconds: None,
        };

        // Tool scope always returns false (requires fresh review)
        assert!(!is_gate_approved(&state, &gates));
    }

    #[test]
    fn is_gate_approved_ttl_expired() {
        let mut state = SessionState::new("test-ttl");
        state.review.decision = Decision::Complete {
            summary: "Done".to_string(),
            second_opinions: None,
        };
        // Approval was 2 hours ago
        state.review.gate_approved_at = Some(Utc::now() - Duration::hours(2));

        let gates = GatesConfig {
            tools: vec!["test*".to_string()],
            approval_scope: ApprovalScope::Session,
            approval_ttl_seconds: Some(3600), // 1 hour TTL
        };

        assert!(!is_gate_approved(&state, &gates));
    }

    #[test]
    fn is_gate_approved_ttl_valid() {
        let mut state = SessionState::new("test-ttl");
        state.review.decision = Decision::Complete {
            summary: "Done".to_string(),
            second_opinions: None,
        };
        // Approval was 30 minutes ago
        state.review.gate_approved_at = Some(Utc::now() - Duration::minutes(30));

        let gates = GatesConfig {
            tools: vec!["test*".to_string()],
            approval_scope: ApprovalScope::Session,
            approval_ttl_seconds: Some(3600), // 1 hour TTL
        };

        assert!(is_gate_approved(&state, &gates));
    }

    // ========================================================================
    // Trace Compaction Tests
    // ========================================================================

    #[test]
    fn trace_compaction_under_limit() {
        let mut state = SessionState::new("trace-test");
        let max_events = 100;

        // Add events under the limit
        for i in 0..50 {
            add_trace_event(
                &mut state,
                TraceEvent {
                    id: format!("evt-{i}"),
                    timestamp: Utc::now(),
                    event_type: EventType::PromptReceived,
                    payload: json!({"index": i}),
                },
                max_events,
            );
        }

        // No compaction should occur
        assert_eq!(state.trace.len(), 50);
        assert!(
            !state
                .trace
                .iter()
                .any(|e| e.event_type == EventType::TraceCompacted)
        );
    }

    #[test]
    fn trace_compaction_at_limit() {
        let mut state = SessionState::new("trace-test");
        let max_events = 50;

        // Add exactly max_events
        for i in 0..50 {
            add_trace_event(
                &mut state,
                TraceEvent {
                    id: format!("evt-{i}"),
                    timestamp: Utc::now(),
                    event_type: EventType::PromptReceived,
                    payload: json!({"index": i}),
                },
                max_events,
            );
        }

        // No compaction yet (exactly at limit)
        assert_eq!(state.trace.len(), 50);
        assert!(
            !state
                .trace
                .iter()
                .any(|e| e.event_type == EventType::TraceCompacted)
        );
    }

    #[test]
    fn trace_compaction_over_limit() {
        let mut state = SessionState::new("trace-test");
        let max_events = 50;

        // Add more than max_events
        for i in 0..60 {
            add_trace_event(
                &mut state,
                TraceEvent {
                    id: format!("evt-{i}"),
                    timestamp: Utc::now(),
                    event_type: EventType::PromptReceived,
                    payload: json!({"index": i}),
                },
                max_events,
            );
        }

        // Compaction should have occurred
        assert_eq!(state.trace.len(), max_events);
        assert!(
            state
                .trace
                .iter()
                .any(|e| e.event_type == EventType::TraceCompacted)
        );
    }

    #[test]
    fn trace_compaction_preserves_first_events() {
        let mut state = SessionState::new("trace-test");
        let max_events = 30;

        // Add more than max_events with identifiable first events
        for i in 0..50 {
            add_trace_event(
                &mut state,
                TraceEvent {
                    id: format!("evt-{i}"),
                    timestamp: Utc::now(),
                    event_type: if i == 0 {
                        EventType::SessionStart
                    } else {
                        EventType::PromptReceived
                    },
                    payload: json!({"index": i}),
                },
                max_events,
            );
        }

        // First event should still be SessionStart
        assert_eq!(state.trace[0].event_type, EventType::SessionStart);
        assert_eq!(state.trace[0].payload["index"], 0);
    }

    #[test]
    fn trace_compaction_preserves_last_events() {
        let mut state = SessionState::new("trace-test");
        let max_events = 30;

        // Add more than max_events
        for i in 0..50 {
            add_trace_event(
                &mut state,
                TraceEvent {
                    id: format!("evt-{i}"),
                    timestamp: Utc::now(),
                    event_type: EventType::PromptReceived,
                    payload: json!({"index": i}),
                },
                max_events,
            );
        }

        // Last event should be the most recent one added (index 49)
        let last = state.trace.last().unwrap();
        assert_eq!(last.payload["index"], 49);
    }

    #[test]
    fn trace_compaction_marker_contains_stats() {
        let mut state = SessionState::new("trace-test");
        let max_events = 30;

        // Add more than max_events
        for i in 0..50 {
            add_trace_event(
                &mut state,
                TraceEvent {
                    id: format!("evt-{i}"),
                    timestamp: Utc::now(),
                    event_type: EventType::PromptReceived,
                    payload: json!({"index": i}),
                },
                max_events,
            );
        }

        // Find compaction marker
        let marker = state
            .trace
            .iter()
            .find(|e| e.event_type == EventType::TraceCompacted)
            .expect("Should have compaction marker");

        // Verify marker has stats
        assert!(marker.payload.get("dropped_events").is_some());
        assert!(marker.payload.get("kept_start").is_some());
        assert!(marker.payload.get("kept_end").is_some());
    }

    #[test]
    fn trace_compaction_small_max_events() {
        let mut state = SessionState::new("trace-test");
        let max_events = 15; // Small limit

        // Add more events
        for i in 0..30 {
            add_trace_event(
                &mut state,
                TraceEvent {
                    id: format!("evt-{i}"),
                    timestamp: Utc::now(),
                    event_type: EventType::PromptReceived,
                    payload: json!({"index": i}),
                },
                max_events,
            );
        }

        // Should be at max_events
        assert_eq!(state.trace.len(), max_events);
    }

    #[test]
    fn trace_compaction_very_large_overflow() {
        let mut state = SessionState::new("trace-test");
        let max_events = 50;

        // Add many events to trigger multiple compactions
        for i in 0..200 {
            add_trace_event(
                &mut state,
                TraceEvent {
                    id: format!("evt-{i}"),
                    timestamp: Utc::now(),
                    event_type: EventType::PromptReceived,
                    payload: json!({"index": i}),
                },
                max_events,
            );
        }

        // Should be at max_events
        assert_eq!(state.trace.len(), max_events);

        // Last event should be index 199
        let last = state.trace.last().unwrap();
        assert_eq!(last.payload["index"], 199);
    }

    // ========================================================================
    // ReviewMode Tests
    // ========================================================================

    #[test]
    fn review_mode_always_enables_review_without_prefix() {
        use crate::config::ReviewMode;

        let store = MemoryBackend::new();
        let mut config = Config::default();
        config.review.mode = ReviewMode::Always;

        let input = HookInput {
            session_id: "test-always-mode".to_string(),
            cwd: PathBuf::from("/tmp"),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("fix the bug".to_string()), // No #roz prefix
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        handle_user_prompt_with_config(&input, &store, &config);

        let state = store.get_session("test-always-mode").unwrap().unwrap();
        assert!(state.review.enabled);
        assert_eq!(state.review.user_prompts.len(), 1);
    }

    #[test]
    fn review_mode_never_disables_review_even_with_prefix() {
        use crate::config::ReviewMode;

        let store = MemoryBackend::new();
        let mut config = Config::default();
        config.review.mode = ReviewMode::Never;

        let input = HookInput {
            session_id: "test-never-mode".to_string(),
            cwd: PathBuf::from("/tmp"),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("#roz fix the bug".to_string()), // Has #roz prefix
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        handle_user_prompt_with_config(&input, &store, &config);

        let state = store.get_session("test-never-mode").unwrap().unwrap();
        assert!(!state.review.enabled);
        assert!(state.review.user_prompts.is_empty());
    }

    #[test]
    fn review_mode_prompt_requires_prefix() {
        use crate::config::ReviewMode;

        let store = MemoryBackend::new();
        let mut config = Config::default();
        config.review.mode = ReviewMode::Prompt; // Default mode

        // Without prefix - no review
        let input = HookInput {
            session_id: "test-prompt-mode-1".to_string(),
            cwd: PathBuf::from("/tmp"),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("fix the bug".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        handle_user_prompt_with_config(&input, &store, &config);
        let state = store.get_session("test-prompt-mode-1").unwrap().unwrap();
        assert!(!state.review.enabled);

        // With prefix - review enabled
        let input = HookInput {
            session_id: "test-prompt-mode-2".to_string(),
            cwd: PathBuf::from("/tmp"),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("#roz fix the bug".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        handle_user_prompt_with_config(&input, &store, &config);
        let state = store.get_session("test-prompt-mode-2").unwrap().unwrap();
        assert!(state.review.enabled);
    }

    // ========================================================================
    // extract_nested_shell_command Tests
    // ========================================================================

    #[test]
    fn extract_nested_shell_double_quotes() {
        let cmd = "bash -c \"echo hello\"";
        assert_eq!(extract_nested_shell_command(cmd), Some("echo hello"));
    }

    #[test]
    fn extract_nested_shell_single_quotes() {
        let cmd = "bash -c 'echo hello'";
        assert_eq!(extract_nested_shell_command(cmd), Some("echo hello"));
    }

    #[test]
    fn extract_nested_shell_with_trailing_content() {
        // This was the bug: extra content after the quoted command
        let cmd = "bash -c \"echo hello\" extra_stuff";
        assert_eq!(extract_nested_shell_command(cmd), Some("echo hello"));
    }

    #[test]
    fn extract_nested_shell_sh_variant() {
        let cmd = "sh -c \"ls -la\"";
        assert_eq!(extract_nested_shell_command(cmd), Some("ls -la"));
    }

    #[test]
    fn extract_nested_shell_full_path() {
        let cmd = "/bin/bash -c 'pwd'";
        assert_eq!(extract_nested_shell_command(cmd), Some("pwd"));
    }

    #[test]
    fn extract_nested_shell_no_quotes() {
        let cmd = "bash -c pwd";
        assert_eq!(extract_nested_shell_command(cmd), Some("pwd"));
    }

    #[test]
    fn extract_nested_shell_not_a_shell() {
        let cmd = "echo hello";
        assert_eq!(extract_nested_shell_command(cmd), None);
    }

    #[test]
    fn extract_nested_shell_unclosed_quote() {
        // Edge case: unclosed quote should return rest without opening quote
        let cmd = "bash -c \"echo hello";
        let result = extract_nested_shell_command(cmd);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "echo hello");
    }

    // ========================================================================
    // Prompt Truncation Tests
    // ========================================================================

    #[test]
    fn truncate_prompt_short() {
        let prompt = "Short prompt";
        assert_eq!(truncate_prompt(prompt), "Short prompt");
    }

    #[test]
    fn truncate_prompt_at_limit() {
        let prompt = "x".repeat(MAX_PROMPT_SIZE);
        let result = truncate_prompt(&prompt);
        assert_eq!(result.len(), MAX_PROMPT_SIZE);
        assert!(!result.contains("truncated"));
    }

    #[test]
    fn truncate_prompt_over_limit() {
        let prompt = "x".repeat(MAX_PROMPT_SIZE + 100);
        let result = truncate_prompt(&prompt);
        assert!(result.contains("truncated"));
        assert!(result.contains(&format!("{}", MAX_PROMPT_SIZE + 100)));
    }

    #[test]
    fn truncate_prompt_unicode_boundary() {
        // Create a prompt with multi-byte characters that would be truncated
        // Each emoji is 4 bytes, so we need enough to exceed the limit
        let emoji_count = MAX_PROMPT_SIZE / 4 + 10;
        let prompt: String = std::iter::repeat_n('🎉', emoji_count).collect();
        let result = truncate_prompt(&prompt);
        // Should truncate at a valid character boundary
        assert!(result.is_char_boundary(result.len() - 1) || result.ends_with(']'));
    }

    // SessionEnd hook tests

    #[test]
    fn session_end_records_trace_event() {
        let store = MemoryBackend::new();

        // Create existing session first
        let input_start = HookInput {
            session_id: "end-test-1".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: Some("startup".to_string()),
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };
        handle_session_start(&input_start, &store);

        // Now send session-end
        let input = HookInput {
            session_id: "end-test-1".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: Some("logout".to_string()),
        };

        let output = handle_session_end(&input, &store);
        assert!(output.decision.is_none(), "session-end always approves");

        let state = store.get_session("end-test-1").unwrap().unwrap();
        let end_events: Vec<_> = state
            .trace
            .iter()
            .filter(|e| e.event_type == EventType::SessionEnd)
            .collect();
        assert_eq!(end_events.len(), 1);
        assert_eq!(end_events[0].payload["reason"], "logout");
    }

    #[test]
    fn session_end_unknown_session_approves() {
        let store = MemoryBackend::new();

        let input = HookInput {
            session_id: "nonexistent".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: Some("clear".to_string()),
        };

        let output = handle_session_end(&input, &store);
        assert!(output.decision.is_none(), "fail-open for unknown session");
    }

    #[test]
    fn session_end_missing_reason_defaults_to_unknown() {
        let store = MemoryBackend::new();

        // Create session
        let input_start = HookInput {
            session_id: "end-test-reason".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: Some("startup".to_string()),
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };
        handle_session_start(&input_start, &store);

        // session-end with no reason
        let input = HookInput {
            session_id: "end-test-reason".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_session_end(&input, &store);
        assert!(output.decision.is_none());

        let state = store.get_session("end-test-reason").unwrap().unwrap();
        let end_event = state
            .trace
            .iter()
            .find(|e| e.event_type == EventType::SessionEnd)
            .expect("should have SessionEnd event");
        assert_eq!(end_event.payload["reason"], "unknown");
    }

    #[test]
    fn session_end_records_cwd_in_payload() {
        let store = MemoryBackend::new();

        // Create session
        let input_start = HookInput {
            session_id: "end-test-cwd".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: Some("startup".to_string()),
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };
        handle_session_start(&input_start, &store);

        let input = HookInput {
            session_id: "end-test-cwd".to_string(),
            cwd: "/home/user/project".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: Some("prompt_input_exit".to_string()),
        };

        handle_session_end(&input, &store);

        let state = store.get_session("end-test-cwd").unwrap().unwrap();
        let end_event = state
            .trace
            .iter()
            .find(|e| e.event_type == EventType::SessionEnd)
            .expect("should have SessionEnd event");
        assert_eq!(end_event.payload["cwd"], "/home/user/project");
        assert_eq!(end_event.payload["reason"], "prompt_input_exit");
    }

    #[test]
    fn session_end_all_reason_values() {
        // Test all documented reason values from Claude Code
        let reasons = [
            "clear",
            "logout",
            "prompt_input_exit",
            "bypass_permissions_disabled",
            "other",
        ];

        for reason in reasons {
            let store = MemoryBackend::new();
            let sid = format!("end-reason-{reason}");

            // Create session
            let input_start = HookInput {
                session_id: sid.clone(),
                cwd: "/tmp".into(),
                transcript_path: None,
                permission_mode: None,
                hook_event_name: None,
                prompt: None,
                tool_name: None,
                tool_input: None,
                tool_response: None,
                source: Some("startup".to_string()),
                model: None,
                agent_type: None,
                agent_id: None,
                agent_transcript_path: None,
                last_assistant_message: None,
                stop_hook_active: None,
                reason: None,
            };
            handle_session_start(&input_start, &store);

            let input = HookInput {
                session_id: sid.clone(),
                cwd: "/tmp".into(),
                transcript_path: None,
                permission_mode: None,
                hook_event_name: None,
                prompt: None,
                tool_name: None,
                tool_input: None,
                tool_response: None,
                source: None,
                model: None,
                agent_type: None,
                agent_id: None,
                agent_transcript_path: None,
                last_assistant_message: None,
                stop_hook_active: None,
                reason: Some(reason.to_string()),
            };

            let output = handle_session_end(&input, &store);
            assert!(output.decision.is_none(), "session-end always approves");

            let state = store.get_session(&sid).unwrap().unwrap();
            let end_event = state
                .trace
                .iter()
                .find(|e| e.event_type == EventType::SessionEnd)
                .expect("should have SessionEnd event");
            assert_eq!(
                end_event.payload["reason"], reason,
                "reason mismatch for {reason}"
            );
        }
    }

    #[test]
    fn session_end_preserves_existing_trace() {
        let store = MemoryBackend::new();

        // Create session with some history
        let input_start = HookInput {
            session_id: "end-test-trace".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: Some("startup".to_string()),
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };
        handle_session_start(&input_start, &store);

        // Add a user prompt event
        let input_prompt = HookInput {
            session_id: "end-test-trace".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: Some("#roz test".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };
        handle_user_prompt(&input_prompt, &store);

        let state_before = store.get_session("end-test-trace").unwrap().unwrap();
        let trace_count_before = state_before.trace.len();

        // End the session
        let input = HookInput {
            session_id: "end-test-trace".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: Some("logout".to_string()),
        };
        handle_session_end(&input, &store);

        let state = store.get_session("end-test-trace").unwrap().unwrap();
        // Should have all previous events plus SessionEnd
        assert_eq!(state.trace.len(), trace_count_before + 1);
        assert_eq!(
            state.trace.last().unwrap().event_type,
            EventType::SessionEnd
        );
    }

    #[test]
    fn session_end_storage_error_approves() {
        // Use a store that will fail on get - we test the fail-open behavior
        // by checking with a nonexistent session (similar pattern to unknown session test)
        let store = MemoryBackend::new();

        let input = HookInput {
            session_id: "storage-fail".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: Some("other".to_string()),
        };

        // No session exists - should fail open
        let output = handle_session_end(&input, &store);
        assert!(output.decision.is_none(), "fail-open on missing session");
    }

    // stop_hook_active defense-in-depth tests

    #[test]
    fn stop_hook_active_true_accelerates_circuit_breaker() {
        let store = MemoryBackend::new();

        // Create session with review enabled and block_count=1
        let mut state = SessionState::new("sha-accel");
        state.review.enabled = true;
        state.review.block_count = 1;
        store.put_session(&state).unwrap();

        // Config with max_blocks=3; stop_hook_active=true → effective=2
        // After increment block_count becomes 2 >= 2 → trips
        let config = Config {
            circuit_breaker: crate::config::CircuitBreakerConfig {
                max_blocks: 3,
                cooldown_seconds: 300,
            },
            ..Config::default()
        };

        let input = HookInput {
            session_id: "sha-accel".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: Some(true),
            reason: None,
        };

        let output = handle_stop_with_config(&input, &store, &config);

        // Should approve because circuit breaker tripped early
        assert!(output.decision.is_none(), "expected approve (CB tripped)");

        let state = store.get_session("sha-accel").unwrap().unwrap();
        assert!(state.review.circuit_breaker_tripped);
    }

    #[test]
    fn stop_hook_active_false_normal_circuit_breaker() {
        let store = MemoryBackend::new();

        // Same setup: block_count=1, max_blocks=3
        let mut state = SessionState::new("sha-normal");
        state.review.enabled = true;
        state.review.block_count = 1;
        store.put_session(&state).unwrap();

        let config = Config {
            circuit_breaker: crate::config::CircuitBreakerConfig {
                max_blocks: 3,
                cooldown_seconds: 300,
            },
            ..Config::default()
        };

        let input = HookInput {
            session_id: "sha-normal".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: Some(false),
            reason: None,
        };

        let output = handle_stop_with_config(&input, &store, &config);

        // Should block because effective_max_blocks=3, block_count=2 < 3
        assert!(
            matches!(output.decision, Some(crate::hooks::HookDecision::Block)),
            "expected block (CB not tripped)"
        );

        let state = store.get_session("sha-normal").unwrap().unwrap();
        assert!(!state.review.circuit_breaker_tripped);
        assert_eq!(state.review.block_count, 2);
    }

    #[test]
    fn stop_hook_active_none_treated_as_false() {
        let store = MemoryBackend::new();

        // Same setup: block_count=1, max_blocks=3
        let mut state = SessionState::new("sha-none");
        state.review.enabled = true;
        state.review.block_count = 1;
        store.put_session(&state).unwrap();

        let config = Config {
            circuit_breaker: crate::config::CircuitBreakerConfig {
                max_blocks: 3,
                cooldown_seconds: 300,
            },
            ..Config::default()
        };

        let input = HookInput {
            session_id: "sha-none".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: None,
            reason: None,
        };

        let output = handle_stop_with_config(&input, &store, &config);

        // Should block same as active=false
        assert!(
            matches!(output.decision, Some(crate::hooks::HookDecision::Block)),
            "expected block (None treated as false)"
        );

        let state = store.get_session("sha-none").unwrap().unwrap();
        assert!(!state.review.circuit_breaker_tripped);
        assert_eq!(state.review.block_count, 2);
    }

    #[test]
    fn stop_hook_active_true_with_max_blocks_one() {
        let store = MemoryBackend::new();

        // max_blocks=1, block_count=0, active=true → floor(1)
        // After increment block_count=1 >= 1 → trips immediately
        let mut state = SessionState::new("sha-floor");
        state.review.enabled = true;
        state.review.block_count = 0;
        store.put_session(&state).unwrap();

        let config = Config {
            circuit_breaker: crate::config::CircuitBreakerConfig {
                max_blocks: 1,
                cooldown_seconds: 300,
            },
            ..Config::default()
        };

        let input = HookInput {
            session_id: "sha-floor".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: Some(true),
            reason: None,
        };

        let output = handle_stop_with_config(&input, &store, &config);

        // Should approve because circuit breaker trips (floor of 1)
        assert!(
            output.decision.is_none(),
            "expected approve (CB tripped at floor)"
        );

        let state = store.get_session("sha-floor").unwrap().unwrap();
        assert!(state.review.circuit_breaker_tripped);
    }

    #[test]
    fn stop_hook_active_trace_includes_effective_max_blocks() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("sha-trace");
        state.review.enabled = true;
        store.put_session(&state).unwrap();

        let config = Config {
            circuit_breaker: crate::config::CircuitBreakerConfig {
                max_blocks: 5,
                cooldown_seconds: 300,
            },
            ..Config::default()
        };

        let input = HookInput {
            session_id: "sha-trace".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: Some(true),
            reason: None,
        };

        handle_stop_with_config(&input, &store, &config);

        let state = store.get_session("sha-trace").unwrap().unwrap();
        let stop_event = state
            .trace
            .iter()
            .find(|e| e.event_type == EventType::StopHookCalled)
            .expect("StopHookCalled trace event should exist");
        assert_eq!(stop_event.payload["stop_hook_active"], true);
        assert_eq!(stop_event.payload["effective_max_blocks"], 4);
    }

    #[test]
    fn stop_hook_active_true_on_issues_accelerates_circuit_breaker() {
        let store = MemoryBackend::new();

        // Create session with Issues decision and block_count=1, max_blocks=3
        let mut state = SessionState::new("sha-issues");
        state.review.enabled = true;
        state.review.block_count = 1;
        state.review.decision = Decision::Issues {
            summary: "Fix tests".to_string(),
            message_to_agent: Some("Add more tests".to_string()),
        };
        store.put_session(&state).unwrap();

        let config = Config {
            circuit_breaker: crate::config::CircuitBreakerConfig {
                max_blocks: 3,
                cooldown_seconds: 300,
            },
            ..Config::default()
        };

        let input = HookInput {
            session_id: "sha-issues".to_string(),
            cwd: "/tmp".into(),
            transcript_path: None,
            permission_mode: None,
            hook_event_name: None,
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            model: None,
            agent_type: None,
            agent_id: None,
            agent_transcript_path: None,
            last_assistant_message: None,
            stop_hook_active: Some(true),
            reason: None,
        };

        let output = handle_stop_with_config(&input, &store, &config);

        // effective_max_blocks=2, after increment block_count=2 >= 2 → trips
        assert!(
            output.decision.is_none(),
            "expected approve (CB tripped on Issues path)"
        );

        let state = store.get_session("sha-issues").unwrap().unwrap();
        assert!(state.review.circuit_breaker_tripped);
    }
}
