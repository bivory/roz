//! Hook handler implementations.

use crate::config::{ApprovalScope, Config, GatesConfig};
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
use regex::Regex;
use serde_json::{Value, json};
use std::process::Command;
use uuid::Uuid;

/// Generate a unique ID for trace events.
fn generate_id() -> String {
    Uuid::new_v4().to_string()
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
    let context = detect_second_opinion_context();

    HookOutput {
        decision: crate::hooks::HookDecision::Approve,
        reason: None,
        context,
    }
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

    // Check for #roz prefix
    if prompt.trim_start().starts_with("#roz") {
        state.review.enabled = true;
        state.review.user_prompts.push(prompt.to_string());
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

    // Log the stop hook call
    state.trace.push(TraceEvent {
        id: generate_id(),
        timestamp: now,
        event_type: EventType::StopHookCalled,
        payload: json!({}),
    });

    // Check if review is enabled
    if !state.review.enabled {
        state.updated_at = now;
        let _ = store.put_session(&state);
        return HookOutput::approve();
    }

    // Check circuit breaker BEFORE incrementing block count
    if circuit_breaker::should_trip(&state, &config.circuit_breaker) {
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
            if circuit_breaker::should_trip(&state, &config.circuit_breaker) {
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
            message_to_agent, ..
        } => {
            // Clone the message before mutable operations
            let msg = message_to_agent.clone().unwrap_or_else(|| {
                "Issues were found. Please address them and try again.".to_string()
            });

            state.review.block_count += 1;

            // Check circuit breaker AFTER incrementing
            if circuit_breaker::should_trip(&state, &config.circuit_breaker) {
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
pub fn handle_subagent_stop(input: &HookInput, store: &dyn MessageStore) -> HookOutput {
    // Only validate roz:roz subagent
    let subagent_type = match &input.subagent_type {
        Some(t) if t == "roz:roz" => t,
        _ => return HookOutput::approve(),
    };

    // Extract session ID from roz's prompt
    let Some(session_id) = extract_session_id(input.subagent_prompt.as_deref()) else {
        return HookOutput::block(
            "roz:roz completed but SESSION_ID not found in prompt. \
             The prompt must include SESSION_ID=<id>.",
        );
    };

    // Get subagent execution window from hook input
    // Fallback: assume subagent started 1 hour ago if not provided
    let subagent_started = input
        .subagent_started_at
        .unwrap_or_else(|| Utc::now() - Duration::hours(1));
    let subagent_ended = Utc::now(); // Hook runs immediately after subagent

    // Check if roz recorded a decision
    let state = match store.get_session(&session_id) {
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

    match &state.review.decision {
        Decision::Pending => HookOutput::block(&format!(
            "roz:roz ({subagent_type}) completed but did not record a decision.\n\n\
             Run: roz decide {session_id} COMPLETE \"summary\"\n\
              or: roz decide {session_id} ISSUES \"summary\" --message \"what to fix\""
        )),
        Decision::Complete { .. } | Decision::Issues { .. } => {
            // Verify decision was posted during subagent execution
            // This prevents main agent from running `roz decide` directly
            let decision_time = state.updated_at;

            if decision_time < subagent_started {
                return HookOutput::block(&format!(
                    "Decision timestamp ({}) is before roz started ({}). \
                     Decision must be posted by roz:roz during its execution.",
                    decision_time.format("%Y-%m-%dT%H:%M:%SZ"),
                    subagent_started.format("%Y-%m-%dT%H:%M:%SZ")
                ));
            }

            // Allow small buffer after subagent ends (clock skew tolerance)
            let buffer = Duration::seconds(5);
            if decision_time > subagent_ended + buffer {
                return HookOutput::block(&format!(
                    "Decision timestamp ({}) is after roz ended ({}). \
                     Decision must be posted by roz:roz during its execution.",
                    decision_time.format("%Y-%m-%dT%H:%M:%SZ"),
                    subagent_ended.format("%Y-%m-%dT%H:%M:%SZ")
                ));
            }

            HookOutput::approve()
        }
    }
}

/// Extract `SESSION_ID` from a subagent prompt.
///
/// Looks for patterns like `SESSION_ID=abc123` or `SESSION_ID: abc123`.
fn extract_session_id(prompt: Option<&str>) -> Option<String> {
    let prompt = prompt?;

    // Try SESSION_ID=xxx pattern first
    let re = Regex::new(r"SESSION_ID[=:]\s*([a-zA-Z0-9_-]+)").ok()?;
    re.captures(prompt)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
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

    // Handle nested shells: bash -c "cmd" or sh -c "cmd"
    let cmd = extract_nested_shell_command(cmd).unwrap_or(cmd);

    // Strip leading environment variable assignments
    let cmd = strip_env_vars(cmd);

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
            // Extract quoted command
            if rest.starts_with('"') {
                return rest.get(1..rest.len() - 1); // Strip quotes
            } else if rest.starts_with('\'') {
                return rest.get(1..rest.len() - 1);
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

    #[test]
    fn extract_session_id_equals() {
        let prompt = "SESSION_ID=test-123\n\n## Summary\nDid stuff";
        assert_eq!(
            extract_session_id(Some(prompt)),
            Some("test-123".to_string())
        );
    }

    #[test]
    fn extract_session_id_colon() {
        let prompt = "SESSION_ID: test-456\n\n## Summary";
        assert_eq!(
            extract_session_id(Some(prompt)),
            Some("test-456".to_string())
        );
    }

    #[test]
    fn extract_session_id_with_spaces() {
        let prompt = "SESSION_ID= abc-789 \n\nContent";
        assert_eq!(
            extract_session_id(Some(prompt)),
            Some("abc-789".to_string())
        );
    }

    #[test]
    fn extract_session_id_missing() {
        let prompt = "No session ID here";
        assert_eq!(extract_session_id(Some(prompt)), None);
    }

    #[test]
    fn extract_session_id_none() {
        assert_eq!(extract_session_id(None), None);
    }

    // User prompt hook tests

    #[test]
    fn user_prompt_roz_prefix_enables_review() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "test-123".to_string(),
            cwd: "/tmp".into(),
            prompt: Some("#roz fix the bug".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_user_prompt(&input, &store);

        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));

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
            prompt: Some("fix the bug".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_user_prompt(&input, &store);

        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));

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
            prompt: Some("#roz fix the bug".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };
        handle_user_prompt(&input1, &store);

        // Second prompt without #roz
        let input2 = HookInput {
            session_id: "test-789".to_string(),
            cwd: "/tmp".into(),
            prompt: Some("also fix tests".to_string()),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
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
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_stop(&input, &store);
        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));
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
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_stop(&input, &store);
        assert!(matches!(output.decision, crate::hooks::HookDecision::Block));
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
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_stop(&input, &store);
        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));
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
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_stop(&input, &store);
        assert!(matches!(output.decision, crate::hooks::HookDecision::Block));
        assert!(output.reason.unwrap().contains("Fix the tests"));
    }

    // Subagent stop hook tests

    #[test]
    fn subagent_stop_non_roz_approves() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "test-123".to_string(),
            cwd: "/tmp".into(),
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: Some("other:agent".to_string()),
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));
    }

    #[test]
    fn subagent_stop_missing_session_id_blocks() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "test-123".to_string(),
            cwd: "/tmp".into(),
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: Some("roz:roz".to_string()),
            subagent_prompt: Some("No session ID here".to_string()),
            subagent_started_at: None,
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(output.decision, crate::hooks::HookDecision::Block));
        assert!(output.reason.unwrap().contains("SESSION_ID not found"));
    }

    #[test]
    fn subagent_stop_decision_pending_blocks() {
        let store = MemoryBackend::new();

        // Create session with pending decision
        let mut state = SessionState::new("test-pending");
        state.review.enabled = true;
        state.review.decision = Decision::Pending;
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "main-session".to_string(),
            cwd: "/tmp".into(),
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: Some("roz:roz".to_string()),
            subagent_prompt: Some("SESSION_ID=test-pending\n\n## Summary".to_string()),
            subagent_started_at: Some(Utc::now() - Duration::minutes(5)),
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(output.decision, crate::hooks::HookDecision::Block));
        assert!(output.reason.unwrap().contains("did not record a decision"));
    }

    #[test]
    fn subagent_stop_valid_decision_approves() {
        let store = MemoryBackend::new();

        // Create session with valid decision timestamp
        let now = Utc::now();
        let mut state = SessionState::new("test-valid");
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        state.updated_at = now; // Decision made now
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "main-session".to_string(),
            cwd: "/tmp".into(),
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: Some("roz:roz".to_string()),
            subagent_prompt: Some("SESSION_ID=test-valid\n\n## Summary".to_string()),
            subagent_started_at: Some(now - Duration::minutes(5)),
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));
    }

    #[test]
    fn subagent_stop_decision_before_start_blocks() {
        let store = MemoryBackend::new();

        // Create session with decision before subagent started
        let now = Utc::now();
        let mut state = SessionState::new("test-before");
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        state.updated_at = now - Duration::hours(2); // Decision made 2 hours ago
        store.put_session(&state).unwrap();

        let input = HookInput {
            session_id: "main-session".to_string(),
            cwd: "/tmp".into(),
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: Some("roz:roz".to_string()),
            subagent_prompt: Some("SESSION_ID=test-before\n\n## Summary".to_string()),
            subagent_started_at: Some(now - Duration::minutes(5)), // Subagent started 5 min ago
        };

        let output = handle_subagent_stop(&input, &store);
        assert!(matches!(output.decision, crate::hooks::HookDecision::Block));
        assert!(output.reason.unwrap().contains("before roz started"));
    }

    // Session start hook tests

    #[test]
    fn session_start_creates_new_session() {
        let store = MemoryBackend::new();
        let input = HookInput {
            session_id: "new-session".to_string(),
            cwd: "/tmp".into(),
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: Some("startup".to_string()),
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_session_start(&input, &store);
        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));

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
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: Some("resume".to_string()),
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_session_start(&input, &store);
        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));

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
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let config = Config::default(); // max_blocks = 3
        let output = handle_stop_with_config(&input, &store, &config);

        // Should approve because circuit breaker tripped
        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));

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
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let config = Config::default();
        let output = handle_stop_with_config(&input, &store, &config);

        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));
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
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        };

        let output = handle_stop_with_config(&input, &store, &config);

        // Should approve because circuit breaker tripped after increment
        assert!(matches!(
            output.decision,
            crate::hooks::HookDecision::Approve
        ));

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
            prompt: None,
            tool_name: Some("mcp__tissue__close_issue".to_string()),
            tool_input: Some(json!({"issue_id": "123"})),
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
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
            prompt: None,
            tool_name: Some("mcp__tissue__list_issues".to_string()), // Not matching pattern
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
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
            prompt: None,
            tool_name: Some("mcp__tissue__close_issue".to_string()),
            tool_input: Some(json!({"issue_id": "123"})),
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
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
            prompt: None,
            tool_name: Some("mcp__tissue__close_issue".to_string()),
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
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
            prompt: None,
            tool_name: Some("mcp__tissue__close_issue".to_string()),
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
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
            prompt: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({"command": "gh issue close 123"})),
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
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
            prompt: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({"command": "echo 'y' | gh issue close 123"})),
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
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
}
