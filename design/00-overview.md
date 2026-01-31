# roz - Quality Gate Design Document: Overview

*"I'm watching you, Wazowski. Always watching."*

## 1. Vision

A quality gate for Claude Code that enforces independent review before task
completion.

This project is inspired by alice. It takes the core concept (mechanical
enforcement of independent review) and implements it cleanly in Rust with
lessons learned.

## 2. Core Concepts

| Concept | Description |
|---------|-------------|
| **Quality gate** | Mechanical enforcement via hooks, not prompts |
| **Adversarial review** | Separate agent reviews work, loyal to user |
| **Consensus review** | Optional second opinions from external models |
| **Session state** | Track review status across hook invocations |

## 3. Design Principles

### 3.1 File-Based Storage

Session state is stored as JSON files in `~/.roz/sessions/`. The storage
interface supports get, put, list, and delete operations. Writes are atomic
via temp file + rename pattern.

### 3.2 Simplified State Model

A single state object per session replaces the multiple topics approach. Each
session tracks:

- Session ID and timestamps
- Review enabled/disabled status
- Current decision (Pending, Complete, or Issues)
- Decision history for debugging
- User prompts that triggered review
- Block count for circuit breaker

### 3.3 Storage Tradeoffs

We use JSON files instead of a database. Key considerations:

| Aspect | JSON Files | Database (SQLite) |
|--------|------------|-------------------|
| Dependencies | None | SQLite library |
| Simplicity | High | Medium |
| Atomic writes | Needs care | Built-in |
| Cross-session queries | List files | SQL queries |

For roz's use case (single-session state with occasional debugging), database
complexity isn't justified.

**Concurrency**: There is no concurrent access within a session. Claude Code
hooks run synchronously. Multiple Claude Code instances get different session
IDs (UUIDs), writing to separate files.

**Risks and mitigations**:

- **Crash mid-write**: Atomic write pattern (write to temp file, then rename)
- **Lost history**: Decision history array preserves all decisions
- **Orphaned files**: Cleanup command removes old sessions
  (`roz clean --before 7d`)

### 3.4 Minimal External Dependencies

Core functionality requires only:

- Rust standard library
- serde (serialization)
- chrono (timestamps)

Single binary includes all backends - no feature flags needed.

## Related Documents

- [Architecture](./01-architecture.md) - System diagrams, domain model,
  sequences
- [Implementation](./02-implementation.md) - Rust types, storage, hooks, CLI
  commands
- [Test Plan](./03-test-plan.md) - Testing strategy
- [CI](./04-ci.md) - Version management and release workflow
- [Agent Instructions](./agents-roz.md) - Full roz agent behavioral
  specification
