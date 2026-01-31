//! `roz list` command implementation.

use crate::error::Result;
use crate::storage::MessageStore;
use crate::storage::file::{FileBackend, get_roz_home};
use chrono::{DateTime, Local, Utc};

/// Default number of sessions to show.
const DEFAULT_LIMIT: usize = 20;

/// Maximum length for prompt preview.
const PROMPT_PREVIEW_LEN: usize = 50;

/// Run the list command.
///
/// Shows recent sessions with their IDs, creation time, and first prompt.
///
/// # Errors
///
/// Returns an error if the storage backend fails.
pub fn run(limit: Option<usize>) -> Result<()> {
    let store = FileBackend::new(get_roz_home())?;
    let limit = limit.unwrap_or(DEFAULT_LIMIT);

    let sessions = store.list_sessions(limit)?;

    if sessions.is_empty() {
        println!("No sessions found.");
        println!("\nSessions are stored in: {}", get_roz_home().display());
        return Ok(());
    }

    println!("{:<38} {:<20} First Prompt", "Session ID", "Created");
    println!("{}", "─".repeat(90));

    for summary in &sessions {
        let created = format_local_time(summary.created_at);
        let prompt = format_prompt_preview(summary.first_prompt.as_deref());

        println!("{:<38} {:<20} {}", summary.session_id, created, prompt);
    }

    println!("{}", "─".repeat(90));
    println!("Showing {} session(s)", sessions.len());

    Ok(())
}

/// Format UTC time as local time for display.
fn format_local_time(utc: DateTime<Utc>) -> String {
    let local: DateTime<Local> = utc.into();
    local.format("%Y-%m-%d %H:%M").to_string()
}

/// Format prompt preview, truncating if needed.
fn format_prompt_preview(prompt: Option<&str>) -> String {
    match prompt {
        Some(p) => {
            // Take first line only
            let first_line = p.lines().next().unwrap_or(p);
            if first_line.len() > PROMPT_PREVIEW_LEN {
                format!("{}...", &first_line[..PROMPT_PREVIEW_LEN])
            } else {
                first_line.to_string()
            }
        }
        None => "(no prompt)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SessionState;
    use crate::storage::MemoryBackend;

    #[test]
    fn list_empty_store() {
        let store = MemoryBackend::new();
        let sessions = store.list_sessions(10).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_returns_sessions() {
        let store = MemoryBackend::new();

        // Create some sessions
        let mut state1 = SessionState::new("session-1");
        state1
            .review
            .user_prompts
            .push("#roz first prompt".to_string());
        store.put_session(&state1).unwrap();

        let mut state2 = SessionState::new("session-2");
        state2
            .review
            .user_prompts
            .push("#roz second prompt".to_string());
        store.put_session(&state2).unwrap();

        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn list_respects_limit() {
        let store = MemoryBackend::new();

        // Create 5 sessions
        for i in 0..5 {
            let state = SessionState::new(&format!("session-{i}"));
            store.put_session(&state).unwrap();
        }

        let sessions = store.list_sessions(3).unwrap();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn format_prompt_preview_truncates_long_prompts() {
        let long_prompt = "x".repeat(100);
        let preview = format_prompt_preview(Some(&long_prompt));
        assert!(preview.len() < 60);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn format_prompt_preview_handles_none() {
        let preview = format_prompt_preview(None);
        assert_eq!(preview, "(no prompt)");
    }

    #[test]
    fn format_prompt_preview_takes_first_line() {
        let multiline = "first line\nsecond line\nthird line";
        let preview = format_prompt_preview(Some(multiline));
        assert_eq!(preview, "first line");
    }
}
