# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.6] - 2026-03-13

### Added

- Session lifecycle tracking via `SessionEnd` hook
- Parse common input fields from Claude Code hooks
  (`transcript_path`, `permission_mode`, `hook_event_name`, `model`)
- Defense-in-depth: `stop_hook_active` signal accelerates circuit
  breaker trips to prevent unnecessary block-continue loops

### Fixed

- Align `SubagentStop` hook fields with Claude Code schema
- Rename `PreToolUse` field `reason` to `permissionDecisionReason`
- Wrap `additionalContext` in `hookSpecificOutput` per Claude Code
  schema
- Use `CLAUDE_PLUGIN_ROOT` for hook commands in plugin manifest
- Remove redundant agents array from plugin manifest

### Changed

- Updated README with `stats` command and environment variables
- Updated DEVELOPMENT project structure to match current codebase

## [0.1.5] - 2026-02-19

### Fixed

- Enhanced circuit breaker with atomic operations and overflow protection
- Added input validation for session IDs, paths, and tool names
- Improved error handling with rate limiting for trace errors
- Added file locking and atomic writes for storage operations
- Better handling of edge cases in hooks and state management

## [0.1.4] - 2026-02-11

### Changed

- Pinned Roz and Grove plugin versions
- Updated installation instructions to use bivory/claude-plugin-marketplace

### Fixed

- Stats command not tracking successful review attempts

## [0.1.3] - 2026-02-10

### Fixed

- Claude Code plugin configuration
- Improved documentation

## [0.1.2] - 2026-02-09

### Changed

- Updated dependencies (dependabot PRs #1-4)
- Added roz plugin installation to devcontainer

## [0.1.1] - 2026-02-08

### Added

- Initial public release
- Rust CLI binary for hook management
- File-based session storage
- Circuit breaker for infinite loop prevention
- Review gates for tool patterns
- Support for Linux and macOS (x86_64 and ARM64)

[Unreleased]: https://github.com/anthropics/roz/compare/v0.1.6...HEAD
[0.1.6]: https://github.com/anthropics/roz/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/anthropics/roz/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/anthropics/roz/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/anthropics/roz/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/anthropics/roz/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/anthropics/roz/releases/tag/v0.1.1
