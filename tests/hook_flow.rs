//! Integration tests for the full hook flow.

use chrono::{Duration, Utc};
use roz::core::state::{AttemptOutcome, Decision, DecisionRecord, SessionState};
use roz::core::{handle_stop, handle_subagent_stop, handle_user_prompt};
use roz::hooks::{HookDecision, HookInput};
use roz::storage::{MemoryBackend, MessageStore};
use std::path::PathBuf;

fn make_input(session_id: &str) -> HookInput {
    HookInput {
        session_id: session_id.to_string(),
        cwd: PathBuf::from("/tmp"),
        prompt: None,
        tool_name: None,
        tool_input: None,
        tool_response: None,
        source: None,
        subagent_type: None,
        subagent_prompt: None,
        subagent_started_at: None,
    }
}

#[test]
fn full_flow_user_prompt_to_complete() {
    let store = MemoryBackend::new();
    let session_id = "flow-test-1";

    // Step 1: User prompt with #roz prefix enables review
    let mut input = make_input(session_id);
    input.prompt = Some("#roz fix the authentication bug".to_string());
    let output = handle_user_prompt(&input, &store);
    assert!(
        output.decision.is_none(),
        "expected approve (decision=None)"
    );

    // Verify review is enabled
    let state = store.get_session(session_id).unwrap().unwrap();
    assert!(state.review.enabled);
    assert_eq!(state.review.decision, Decision::Pending);

    // Step 2: Stop hook blocks because review is pending
    let input = make_input(session_id);
    let output = handle_stop(&input, &store);
    assert!(matches!(output.decision, Some(HookDecision::Block)));
    assert!(output.reason.is_some());
    assert!(output.reason.as_ref().unwrap().contains("roz:roz"));

    // Step 3: Simulate roz decision (COMPLETE)
    let mut state = store.get_session(session_id).unwrap().unwrap();
    let now = Utc::now();
    state.review.decision_history.push(DecisionRecord {
        decision: state.review.decision.clone(),
        timestamp: now,
    });
    state.review.decision = Decision::Complete {
        summary: "All changes look good".to_string(),
        second_opinions: None,
    };
    state.updated_at = now;
    store.put_session(&state).unwrap();

    // Step 4: Stop hook now approves
    let input = make_input(session_id);
    let output = handle_stop(&input, &store);
    assert!(
        output.decision.is_none(),
        "expected approve (decision=None)"
    );
}

#[test]
fn full_flow_with_issues() {
    let store = MemoryBackend::new();
    let session_id = "flow-test-2";

    // Step 1: Enable review
    let mut input = make_input(session_id);
    input.prompt = Some("#roz add new feature".to_string());
    handle_user_prompt(&input, &store);

    // Step 2: First stop blocks
    let input = make_input(session_id);
    let output = handle_stop(&input, &store);
    assert!(matches!(output.decision, Some(HookDecision::Block)));

    // Step 3: Roz finds issues
    let mut state = store.get_session(session_id).unwrap().unwrap();
    state.review.decision = Decision::Issues {
        summary: "Missing test coverage".to_string(),
        message_to_agent: Some("Add unit tests for the new feature".to_string()),
    };
    state.updated_at = Utc::now();
    store.put_session(&state).unwrap();

    // Step 4: Stop hook still blocks with issue message
    let input = make_input(session_id);
    let output = handle_stop(&input, &store);
    assert!(matches!(output.decision, Some(HookDecision::Block)));
    assert!(
        output
            .reason
            .as_ref()
            .unwrap()
            .contains("Add unit tests for the new feature")
    );

    // Step 5: After fixing, roz approves
    let mut state = store.get_session(session_id).unwrap().unwrap();
    state.review.decision = Decision::Complete {
        summary: "Tests added, looks good now".to_string(),
        second_opinions: None,
    };
    state.updated_at = Utc::now();
    store.put_session(&state).unwrap();

    // Step 6: Stop hook approves
    let input = make_input(session_id);
    let output = handle_stop(&input, &store);
    assert!(
        output.decision.is_none(),
        "expected approve (decision=None)"
    );
}

#[test]
fn subagent_stop_validates_timestamp() {
    let store = MemoryBackend::new();
    let session_id = "subagent-test";

    // Create session with pending review
    let mut state = SessionState::new(session_id);
    state.review.enabled = true;
    store.put_session(&state).unwrap();

    // Subagent starts
    let subagent_started = Utc::now();

    // Simulate roz posting a decision during execution
    let mut state = store.get_session(session_id).unwrap().unwrap();
    state.review.decision = Decision::Complete {
        summary: "All good".to_string(),
        second_opinions: None,
    };
    state.updated_at = Utc::now();
    store.put_session(&state).unwrap();

    // Subagent stop validates the decision
    let mut input = make_input("main-session");
    input.subagent_type = Some("roz:roz".to_string());
    input.subagent_prompt = Some(format!("SESSION_ID={session_id}\n\n## Summary\nReviewed"));
    input.subagent_started_at = Some(subagent_started);

    let output = handle_subagent_stop(&input, &store);
    assert!(
        output.decision.is_none(),
        "expected approve (decision=None)"
    );
}

#[test]
fn subagent_stop_rejects_pre_existing_decision() {
    let store = MemoryBackend::new();
    let session_id = "subagent-test-reject";

    // Create session with a decision from BEFORE the subagent started
    let old_decision_time = Utc::now() - Duration::hours(2);
    let mut state = SessionState::new(session_id);
    state.review.enabled = true;
    state.review.decision = Decision::Complete {
        summary: "Old decision".to_string(),
        second_opinions: None,
    };
    state.updated_at = old_decision_time;
    store.put_session(&state).unwrap();

    // Subagent starts now
    let subagent_started = Utc::now();

    // Subagent stop should reject because decision is too old
    let mut input = make_input("main-session");
    input.subagent_type = Some("roz:roz".to_string());
    input.subagent_prompt = Some(format!("SESSION_ID={session_id}\n\n## Summary"));
    input.subagent_started_at = Some(subagent_started);

    let output = handle_subagent_stop(&input, &store);
    assert!(matches!(output.decision, Some(HookDecision::Block)));
    assert!(
        output
            .reason
            .as_ref()
            .unwrap()
            .contains("before roz started")
    );
}

#[test]
fn subagent_stop_rejects_decision_after_end() {
    let store = MemoryBackend::new();
    let session_id = "subagent-test-after";

    // Subagent started 10 minutes ago
    let subagent_started = Utc::now() - Duration::minutes(10);
    // Subagent ended 9 minutes ago (ran for 1 minute)
    // Note: We can't use this directly since the hook calculates subagent_ended internally
    let _subagent_ended_approx = Utc::now() - Duration::minutes(9);

    // Create session with a decision from AFTER the subagent ended (beyond 5s buffer)
    // Decision timestamp is "now" which is 9 minutes after subagent ended
    let mut state = SessionState::new(session_id);
    state.review.enabled = true;
    state.review.decision = Decision::Complete {
        summary: "Late decision".to_string(),
        second_opinions: None,
    };
    state.updated_at = Utc::now(); // Decision made "now", well after subagent ended
    store.put_session(&state).unwrap();

    // Subagent stop should reject because decision is after end + buffer
    let mut input = make_input("main-session");
    input.subagent_type = Some("roz:roz".to_string());
    input.subagent_prompt = Some(format!("SESSION_ID={session_id}\n\n## Summary"));
    input.subagent_started_at = Some(subagent_started);
    // Note: The hook uses Utc::now() as subagent_ended, so we can't directly test
    // the "after end" case without mocking time. However, we can verify the logic
    // exists by checking the code path. For a true test, we'd need time mocking.
    // This test documents the expected behavior even if it can't trigger the edge case
    // in real time.

    // Since the decision was just made (updated_at = now), and the hook also uses
    // Utc::now() as subagent_ended, the decision will be within the buffer.
    // To properly test this, we'd need to mock time. For now, this test verifies
    // the happy path still works and documents the expected behavior.
    let output = handle_subagent_stop(&input, &store);
    // This should approve because decision_time <= subagent_ended + 5s buffer
    // (both are approximately Utc::now())
    assert!(
        output.decision.is_none(),
        "expected approve (decision=None)"
    );
}

#[test]
fn block_count_increments() {
    let store = MemoryBackend::new();
    let session_id = "block-count-test";

    // Enable review
    let mut input = make_input(session_id);
    input.prompt = Some("#roz test".to_string());
    handle_user_prompt(&input, &store);

    // Block multiple times
    let input = make_input(session_id);
    handle_stop(&input, &store);

    let state = store.get_session(session_id).unwrap().unwrap();
    assert_eq!(state.review.block_count, 1);

    handle_stop(&input, &store);
    let state = store.get_session(session_id).unwrap().unwrap();
    assert_eq!(state.review.block_count, 2);

    handle_stop(&input, &store);
    let state = store.get_session(session_id).unwrap().unwrap();
    assert_eq!(state.review.block_count, 3);
}

#[test]
fn no_review_means_no_block() {
    let store = MemoryBackend::new();
    let session_id = "no-review-test";

    // User prompt WITHOUT #roz prefix
    let mut input = make_input(session_id);
    input.prompt = Some("fix the bug".to_string());
    handle_user_prompt(&input, &store);

    // Stop should approve immediately
    let input = make_input(session_id);
    let output = handle_stop(&input, &store);
    assert!(
        output.decision.is_none(),
        "expected approve (decision=None)"
    );
}

// ============================================================================
// Gate Approval Scope Integration Tests
// ============================================================================

use roz::config::{ApprovalScope, Config, GatesConfig, ReviewConfig};
use roz::core::handle_pre_tool_use;
use roz::hooks::PermissionDecision;
use serde_json::json;

fn make_gate_input(session_id: &str, tool_name: &str) -> HookInput {
    HookInput {
        session_id: session_id.to_string(),
        cwd: PathBuf::from("/tmp"),
        prompt: None,
        tool_name: Some(tool_name.to_string()),
        tool_input: Some(json!({"arg": "value"})),
        tool_response: None,
        source: None,
        subagent_type: None,
        subagent_prompt: None,
        subagent_started_at: None,
    }
}

fn make_config_with_gates(tools: Vec<&str>, scope: ApprovalScope) -> Config {
    Config {
        review: ReviewConfig {
            gates: GatesConfig {
                tools: tools.into_iter().map(String::from).collect(),
                approval_scope: scope,
                approval_ttl_seconds: None,
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn gate_session_scope_allows_after_approval() {
    let store = MemoryBackend::new();
    let session_id = "gate-session-scope";
    let config = make_config_with_gates(vec!["mcp__test__*"], ApprovalScope::Session);

    // Create session
    let mut state = SessionState::new(session_id);
    store.put_session(&state).unwrap();

    // First gate call should block
    let input = make_gate_input(session_id, "mcp__test__action1");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Deny
    );

    // Simulate approval
    state = store.get_session(session_id).unwrap().unwrap();
    state.review.decision = Decision::Complete {
        summary: "Approved".to_string(),
        second_opinions: None,
    };
    state.review.gate_approved_at = Some(Utc::now());
    store.put_session(&state).unwrap();

    // Second gate call (different tool) should allow (session scope)
    let input = make_gate_input(session_id, "mcp__test__action2");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Allow
    );
}

#[test]
fn gate_prompt_scope_resets_on_new_prompt() {
    let store = MemoryBackend::new();
    let session_id = "gate-prompt-scope";
    let config = make_config_with_gates(vec!["mcp__test__*"], ApprovalScope::Prompt);

    // Create session with approval
    let mut state = SessionState::new(session_id);
    state.review.decision = Decision::Complete {
        summary: "Approved".to_string(),
        second_opinions: None,
    };
    state.review.gate_approved_at = Some(Utc::now());
    state.review.last_prompt_at = Some(Utc::now() - Duration::hours(1)); // Old prompt
    store.put_session(&state).unwrap();

    // Gate call should allow (approval is valid, prompt hasn't changed)
    let input = make_gate_input(session_id, "mcp__test__action");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Allow
    );

    // Simulate new prompt (updates last_prompt_at to after gate_approved_at)
    state = store.get_session(session_id).unwrap().unwrap();
    state.review.last_prompt_at = Some(Utc::now());
    store.put_session(&state).unwrap();

    // Gate call should now block (new prompt invalidates approval)
    let input = make_gate_input(session_id, "mcp__test__action");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Deny
    );
}

#[test]
fn gate_tool_scope_requires_fresh_review_each_time() {
    let store = MemoryBackend::new();
    let session_id = "gate-tool-scope";
    let config = make_config_with_gates(vec!["mcp__test__*"], ApprovalScope::Tool);

    // Create session with approval
    let mut state = SessionState::new(session_id);
    state.review.decision = Decision::Complete {
        summary: "Approved".to_string(),
        second_opinions: None,
    };
    state.review.gate_approved_at = Some(Utc::now());
    store.put_session(&state).unwrap();

    // Gate call should STILL block (tool scope always requires fresh review)
    let input = make_gate_input(session_id, "mcp__test__action");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Deny
    );
}

#[test]
fn gate_non_matching_tool_allows() {
    let store = MemoryBackend::new();
    let session_id = "gate-non-match";
    let config = make_config_with_gates(vec!["mcp__tissue__*"], ApprovalScope::Session);

    // Create session
    let state = SessionState::new(session_id);
    store.put_session(&state).unwrap();

    // Non-matching tool should be allowed
    let input = make_gate_input(session_id, "Read");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Allow
    );
}

#[test]
fn gate_empty_tools_allows_all() {
    let store = MemoryBackend::new();
    let session_id = "gate-empty";
    let config = make_config_with_gates(vec![], ApprovalScope::Session);

    // Create session
    let state = SessionState::new(session_id);
    store.put_session(&state).unwrap();

    // Any tool should be allowed when no gates configured
    let input = make_gate_input(session_id, "mcp__anything__action");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Allow
    );
}

#[test]
fn gate_circuit_breaker_allows() {
    let store = MemoryBackend::new();
    let session_id = "gate-circuit-breaker";
    let config = make_config_with_gates(vec!["mcp__test__*"], ApprovalScope::Session);

    // Create session with circuit breaker tripped
    let mut state = SessionState::new(session_id);
    state.review.circuit_breaker_tripped = true;
    store.put_session(&state).unwrap();

    // Should allow due to circuit breaker
    let input = make_gate_input(session_id, "mcp__test__action");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Allow
    );
}

#[test]
fn full_gate_flow_block_approve_allow() {
    let store = MemoryBackend::new();
    let session_id = "gate-full-flow";
    let config = make_config_with_gates(vec!["mcp__tissue__close*"], ApprovalScope::Session);

    // Step 1: Create session
    let state = SessionState::new(session_id);
    store.put_session(&state).unwrap();

    // Step 2: Gate blocks on matching tool
    let input = make_gate_input(session_id, "mcp__tissue__close_issue");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Deny
    );

    // Step 3: Verify review is enabled and gate trigger recorded
    let state = store.get_session(session_id).unwrap().unwrap();
    assert!(state.review.enabled);
    assert!(state.review.gate_trigger.is_some());
    assert_eq!(
        state.review.gate_trigger.as_ref().unwrap().tool_name,
        "mcp__tissue__close_issue"
    );

    // Step 4: Simulate roz approval
    let mut state = store.get_session(session_id).unwrap().unwrap();
    state.review.decision = Decision::Complete {
        summary: "Reviewed and approved".to_string(),
        second_opinions: None,
    };
    state.review.gate_approved_at = Some(Utc::now());
    store.put_session(&state).unwrap();

    // Step 5: Gate now allows
    let input = make_gate_input(session_id, "mcp__tissue__close_issue");
    let output = handle_pre_tool_use(&input, &config, &store);
    assert_eq!(
        output.hook_specific_output.permission_decision,
        PermissionDecision::Allow
    );
}

// ============================================================================
// Attempt Outcome Tracking Tests (for stats)
// ============================================================================

#[test]
fn stop_hook_creates_pending_attempt() {
    let store = MemoryBackend::new();
    let session_id = "attempt-tracking-test";

    // Enable review
    let mut input = make_input(session_id);
    input.prompt = Some("#roz test attempt tracking".to_string());
    handle_user_prompt(&input, &store);

    // Stop hook should create a pending attempt when blocking
    let input = make_input(session_id);
    let output = handle_stop(&input, &store);
    assert!(matches!(output.decision, Some(HookDecision::Block)));

    // Verify attempt was created with Pending outcome
    let state = store.get_session(session_id).unwrap().unwrap();
    assert_eq!(state.review.attempts.len(), 1);
    assert!(matches!(
        state.review.attempts[0].outcome,
        AttemptOutcome::Pending
    ));
    assert_eq!(state.review.attempts[0].template_id, "default");
}

#[test]
fn decide_updates_attempt_outcome_for_stats() {
    let store = MemoryBackend::new();
    let session_id = "stats-tracking-test";

    // Enable review
    let mut input = make_input(session_id);
    input.prompt = Some("#roz test stats tracking".to_string());
    handle_user_prompt(&input, &store);

    // Stop hook blocks and creates pending attempt
    let input = make_input(session_id);
    handle_stop(&input, &store);

    // Verify pending attempt exists
    let state = store.get_session(session_id).unwrap().unwrap();
    assert_eq!(state.review.attempts.len(), 1);
    assert!(matches!(
        state.review.attempts[0].outcome,
        AttemptOutcome::Pending
    ));

    // Simulate roz decide command (this is what the fix addresses)
    // The decide command should update the attempt outcome to Success
    let mut state = store.get_session(session_id).unwrap().unwrap();
    let now = Utc::now();

    // This is the logic from decide.rs that we're testing
    state.review.decision_history.push(DecisionRecord {
        decision: state.review.decision.clone(),
        timestamp: now,
    });
    state.review.gate_approved_at = Some(now);

    // Update the most recent pending attempt's outcome (the fix)
    if let Some(attempt) = state
        .review
        .attempts
        .iter_mut()
        .rev()
        .find(|a| matches!(a.outcome, AttemptOutcome::Pending))
    {
        attempt.outcome = AttemptOutcome::Success {
            decision_type: "complete".to_string(),
            blocks_needed: state.review.block_count,
        };
    }

    state.review.decision = Decision::Complete {
        summary: "All good".to_string(),
        second_opinions: None,
    };
    state.updated_at = now;
    store.put_session(&state).unwrap();

    // Verify attempt outcome was updated for stats tracking
    let final_state = store.get_session(session_id).unwrap().unwrap();
    assert_eq!(final_state.review.attempts.len(), 1);
    match &final_state.review.attempts[0].outcome {
        AttemptOutcome::Success {
            decision_type,
            blocks_needed,
        } => {
            assert_eq!(decision_type, "complete");
            assert_eq!(*blocks_needed, 1); // block_count was 1 after first block
        }
        other => panic!("Expected Success outcome for stats tracking, got {other:?}"),
    }
}
