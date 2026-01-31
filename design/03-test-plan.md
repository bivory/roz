# roz - Test Plan

---

## 14. Testing Strategy

### 14.1 Phase 1: With Implementation

**Unit tests** for core logic:
- Hook I/O parsing (malformed JSON, missing fields)
- State machine transitions
- Circuit breaker edge cases
- `#roz` command parsing

**Integration tests** for file backend:
- Atomic write / read cycle
- Corruption recovery (temp file cleanup)
- Session listing and cleanup

**In-memory backend** for unit tests:
```rust
struct MemoryBackend {
    sessions: RefCell<HashMap<String, SessionState>>,
}
```

### 14.2 Phase 2: After MVP

**Property-based tests** using `proptest`:
- State machine: random event sequences never panic, always valid state
- Serialization: round-trip through JSON

**Hook simulation harness**:
```bash
# Simulate Claude Code invoking hooks
echo '{"session_id":"test","prompt":"#roz fix"}' | roz hook user-prompt
echo '{"session_id":"test"}' | roz hook stop
```

### 14.3 Out of Scope

- Mocking Claude Code itself
- Testing external model calls (codex/gemini)

---

## Test Categories

### Unit Tests

| Category | Tests | Priority |
|----------|-------|----------|
| Hook I/O parsing | Valid JSON, malformed JSON, missing required fields, extra fields | High |
| State transitions | Idle→Pending, Pending→Blocked, Blocked→Approved, circuit breaker | High |
| `#roz` command parsing | `#roz`, `#roz:stop`, `#roz message`, edge cases | High |
| Decision serialization | Round-trip Complete, Issues, Pending through JSON | Medium |
| Template loading | File exists, file missing (fallback), invalid template | Medium |
| **Gate pattern matching** | Glob patterns, order precedence, no match | High |
| **Bash normalization** | Env vars, pipes, nested shells, edge cases | High |
| **Approval scope** | Session/prompt/tool scopes, TTL expiry | High |
| **TruncatedInput** | Under limit, over limit, hash verification | Medium |
| **Trace limiting** | Under limit, at limit, compaction | Medium |

### Integration Tests

| Category | Tests | Priority |
|----------|-------|----------|
| File backend | Write/read cycle, atomic rename, temp file cleanup | High |
| Session listing | Empty dir, single session, multiple sessions, sort order | Medium |
| Session cleanup | Age filter, skip active sessions, remove completed | Medium |
| CLI commands | `roz decide`, `roz context`, `roz debug` | Medium |

### Property-Based Tests (Phase 2)

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn state_machine_never_panics(events: Vec<HookEvent>) {
        let mut state = SessionState::default();
        for event in events {
            let _ = state.handle(event);
        }
        // Should never panic, state should always be valid
        assert!(state.is_valid());
    }

    #[test]
    fn session_roundtrip(state: SessionState) {
        let json = serde_json::to_string(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }
}
```

### Gate Unit Tests

```rust
#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn bash_normalization_strips_env_vars() {
        assert_eq!(
            normalize_bash_command("GH_TOKEN=abc gh issue close 123"),
            "gh issue close 123"
        );
    }

    #[test]
    fn bash_normalization_handles_pipes() {
        assert_eq!(
            normalize_bash_command("echo 'y' | gh issue close 123"),
            "gh issue close 123"
        );
    }

    #[test]
    fn bash_normalization_extracts_nested_shell() {
        assert_eq!(
            normalize_bash_command("bash -c \"gh issue close 123\""),
            "gh issue close 123"
        );
    }

    #[test]
    fn bash_normalization_handles_env_command() {
        assert_eq!(
            normalize_bash_command("env GH_TOKEN=x gh issue close"),
            "gh issue close"
        );
    }

    #[test]
    fn approval_scope_session_always_valid() {
        let mut state = SessionState::new("test");
        state.review.decision = Decision::Complete { summary: "ok".into(), second_opinions: None };
        state.review.gate_approved_at = Some(Utc::now() - Duration::hours(1));

        let gates = GatesConfig {
            tools: vec!["test".into()],
            approval_scope: ApprovalScope::Session,
            approval_ttl_seconds: None,
        };

        assert!(is_gate_approved(&state, &gates));
    }

    #[test]
    fn approval_scope_prompt_invalidated_by_new_prompt() {
        let mut state = SessionState::new("test");
        state.review.decision = Decision::Complete { summary: "ok".into(), second_opinions: None };
        state.review.gate_approved_at = Some(Utc::now() - Duration::minutes(5));
        state.review.last_prompt_at = Some(Utc::now());  // After approval

        let gates = GatesConfig {
            tools: vec!["test".into()],
            approval_scope: ApprovalScope::Prompt,
            approval_ttl_seconds: None,
        };

        assert!(!is_gate_approved(&state, &gates));
    }

    #[test]
    fn approval_ttl_expires() {
        let mut state = SessionState::new("test");
        state.review.decision = Decision::Complete { summary: "ok".into(), second_opinions: None };
        state.review.gate_approved_at = Some(Utc::now() - Duration::hours(2));

        let gates = GatesConfig {
            tools: vec!["test".into()],
            approval_scope: ApprovalScope::Session,
            approval_ttl_seconds: Some(3600),  // 1 hour TTL, approval is 2 hours old
        };

        assert!(!is_gate_approved(&state, &gates));
    }

    #[test]
    fn truncated_input_under_limit() {
        let small = json!({"key": "value"});
        let truncated = TruncatedInput::from_value(small.clone());
        assert!(!truncated.truncated);
        assert!(truncated.original_hash.is_none());
    }

    #[test]
    fn truncated_input_over_limit() {
        let large = json!({"data": "x".repeat(20000)});
        let truncated = TruncatedInput::from_value(large);
        assert!(truncated.truncated);
        assert!(truncated.original_hash.is_some());
        assert!(truncated.original_size.unwrap() > 10000);
    }
}
```

### Hook Simulation Tests

```bash
#!/bin/bash
# test/integration/hook_flow.sh

# Setup
export ROZ_HOME=$(mktemp -d)
SESSION_ID="test-$(uuidgen)"

# Test user-prompt hook enables review
echo "{\"session_id\":\"$SESSION_ID\",\"prompt\":\"#roz fix the bug\"}" \
  | roz hook user-prompt \
  | jq -e '.decision == "approve"'

# Verify state was created
roz debug $SESSION_ID | grep -q "review_enabled: true"

# Test stop hook blocks
echo "{\"session_id\":\"$SESSION_ID\"}" \
  | roz hook stop \
  | jq -e '.decision == "block"'

# Post decision
roz decide $SESSION_ID COMPLETE "All good"

# Test stop hook now approves
echo "{\"session_id\":\"$SESSION_ID\"}" \
  | roz hook stop \
  | jq -e '.decision == "approve"'

# Cleanup
rm -rf $ROZ_HOME
```

### Gate Simulation Tests

```bash
#!/bin/bash
# test/integration/gate_flow.sh

# Setup with gates enabled
export ROZ_HOME=$(mktemp -d)
cat > $ROZ_HOME/config.toml << 'EOF'
[review.gates]
tools = ["mcp__tissue__close*"]
approval_scope = "prompt"
EOF

SESSION_ID="test-$(uuidgen)"

# Initialize session
echo "{\"session_id\":\"$SESSION_ID\",\"source\":\"startup\"}" \
  | roz hook session-start

# Test pre-tool-use blocks gated tool
echo "{\"session_id\":\"$SESSION_ID\",\"tool_name\":\"mcp__tissue__close_issue\",\"tool_input\":{\"id\":123}}" \
  | roz hook pre-tool-use \
  | jq -e '.hookSpecificOutput.permissionDecision == "deny"'

# Post approval
roz decide $SESSION_ID COMPLETE "Reviewed"

# Test pre-tool-use now allows
echo "{\"session_id\":\"$SESSION_ID\",\"tool_name\":\"mcp__tissue__close_issue\",\"tool_input\":{\"id\":123}}" \
  | roz hook pre-tool-use \
  | jq -e '.hookSpecificOutput.permissionDecision == "allow"'

# Simulate new prompt (should invalidate approval for scope=prompt)
echo "{\"session_id\":\"$SESSION_ID\",\"prompt\":\"close another issue\"}" \
  | roz hook user-prompt

# Test pre-tool-use blocks again
echo "{\"session_id\":\"$SESSION_ID\",\"tool_name\":\"mcp__tissue__close_issue\",\"tool_input\":{\"id\":456}}" \
  | roz hook pre-tool-use \
  | jq -e '.hookSpecificOutput.permissionDecision == "deny"'

# Cleanup
rm -rf $ROZ_HOME
```

---

## Test Matrix

### Platforms

| Platform | CI | Local |
|----------|-----|-------|
| Linux x86_64 | GitHub Actions | Required |
| Linux ARM64 | GitHub Actions | Required |
| macOS x86_64 | GitHub Actions | Optional |
| macOS ARM64 | GitHub Actions | Required |
| Windows | Not supported | Not supported |

### Rust Versions

| Version | Status |
|---------|--------|
| Stable (latest) | Required |
| Beta | Optional |
| Nightly | Not tested |
| MSRV | 1.70.0 (tentative) |

---
