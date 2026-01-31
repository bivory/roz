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
