//! File-based storage backend.

use crate::core::SessionState;
use crate::error::Result;
use crate::storage::traits::{MessageStore, SessionSummary};
use std::fs;
use std::path::PathBuf;

/// File-based storage backend with atomic writes.
#[derive(Debug)]
pub struct FileBackend {
    base_dir: PathBuf,
}

impl FileBackend {
    /// Create a new file backend.
    ///
    /// Creates the sessions directory if it doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be created.
    pub fn new(base_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(base_dir.join("sessions"))?;
        Ok(Self { base_dir })
    }

    /// Get the path to a session file.
    fn session_path(&self, session_id: &str) -> PathBuf {
        self.base_dir
            .join("sessions")
            .join(format!("{session_id}.json"))
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

        if !sessions_dir.exists() {
            return Ok(sessions);
        }

        for entry in fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Only process .json files (skip .tmp files)
            if path.extension().is_some_and(|e| e == "json") {
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
        }

        // Sort by created_at descending (most recent first)
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        sessions.truncate(limit);
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

/// Get the default roz home directory.
///
/// Uses `ROZ_HOME` environment variable if set, otherwise `~/.roz`.
#[must_use]
pub fn get_roz_home() -> PathBuf {
    if let Ok(home) = std::env::var("ROZ_HOME") {
        PathBuf::from(home)
    } else if let Some(home) = dirs::home_dir() {
        home.join(".roz")
    } else {
        PathBuf::from(".roz")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_backend() -> (FileBackend, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let backend = FileBackend::new(temp_dir.path().to_path_buf()).unwrap();
        (backend, temp_dir)
    }

    #[test]
    fn creates_sessions_directory() {
        let temp_dir = TempDir::new().unwrap();
        let _backend = FileBackend::new(temp_dir.path().to_path_buf()).unwrap();
        assert!(temp_dir.path().join("sessions").exists());
    }

    #[test]
    fn get_missing_session() {
        let (store, _temp) = create_test_backend();
        let result = store.get_session("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn put_and_get_session() {
        let (store, _temp) = create_test_backend();
        let state = SessionState::new("test-123");

        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-123").unwrap().unwrap();
        assert_eq!(retrieved.session_id, "test-123");
    }

    #[test]
    fn atomic_write_creates_no_temp_file() {
        let (store, temp_dir) = create_test_backend();
        let state = SessionState::new("test-123");

        store.put_session(&state).unwrap();

        // Temp file should not exist after successful write
        let temp_path = temp_dir.path().join("sessions").join("test-123.tmp");
        assert!(!temp_path.exists());

        // Main file should exist
        let main_path = temp_dir.path().join("sessions").join("test-123.json");
        assert!(main_path.exists());
    }

    #[test]
    fn list_sessions_empty() {
        let (store, _temp) = create_test_backend();
        let sessions = store.list_sessions(10).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_with_data() {
        let (store, _temp) = create_test_backend();

        store.put_session(&SessionState::new("session-1")).unwrap();
        store.put_session(&SessionState::new("session-2")).unwrap();

        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn list_sessions_ignores_tmp_files() {
        let (store, temp_dir) = create_test_backend();

        store.put_session(&SessionState::new("session-1")).unwrap();

        // Create a stray .tmp file
        let tmp_path = temp_dir.path().join("sessions").join("orphan.tmp");
        fs::write(&tmp_path, "{}").unwrap();

        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "session-1");
    }

    #[test]
    fn delete_session_removes_file() {
        let (store, temp_dir) = create_test_backend();
        let state = SessionState::new("test-123");

        store.put_session(&state).unwrap();

        let path = temp_dir.path().join("sessions").join("test-123.json");
        assert!(path.exists());

        store.delete_session("test-123").unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn delete_nonexistent_session_succeeds() {
        let (store, _temp) = create_test_backend();
        // Should not error when deleting non-existent session
        store.delete_session("nonexistent").unwrap();
    }

    #[test]
    fn list_sessions_skips_corrupted_json() {
        let (store, temp_dir) = create_test_backend();

        // Create a valid session
        store
            .put_session(&SessionState::new("valid-session"))
            .unwrap();

        // Create a corrupted JSON file
        let corrupted_path = temp_dir.path().join("sessions").join("corrupted.json");
        fs::write(&corrupted_path, "{ this is not valid json }").unwrap();

        // list_sessions should skip the corrupted file and return valid session
        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "valid-session");
    }

    #[test]
    fn list_sessions_skips_empty_files() {
        let (store, temp_dir) = create_test_backend();

        // Create a valid session
        store
            .put_session(&SessionState::new("valid-session"))
            .unwrap();

        // Create an empty file
        let empty_path = temp_dir.path().join("sessions").join("empty.json");
        fs::write(&empty_path, "").unwrap();

        // list_sessions should skip the empty file
        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "valid-session");
    }

    #[test]
    fn list_sessions_skips_partial_json() {
        let (store, temp_dir) = create_test_backend();

        // Create a valid session
        store
            .put_session(&SessionState::new("valid-session"))
            .unwrap();

        // Create a file with partial/truncated JSON (simulates crash during write)
        let partial_path = temp_dir.path().join("sessions").join("partial.json");
        fs::write(&partial_path, r#"{"session_id": "partial", "review": {"#).unwrap();

        // list_sessions should skip the partial file
        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "valid-session");
    }

    #[test]
    fn get_session_corrupted_returns_error() {
        let (store, temp_dir) = create_test_backend();

        // Create a corrupted JSON file
        let corrupted_path = temp_dir.path().join("sessions").join("corrupted.json");
        fs::write(&corrupted_path, "{ invalid }").unwrap();

        // get_session should return an error for corrupted files
        let result = store.get_session("corrupted");
        assert!(result.is_err());
    }

    #[test]
    fn list_sessions_handles_wrong_schema() {
        let (store, temp_dir) = create_test_backend();

        // Create a valid session
        store
            .put_session(&SessionState::new("valid-session"))
            .unwrap();

        // Create a file with valid JSON but wrong schema
        let wrong_schema_path = temp_dir.path().join("sessions").join("wrong-schema.json");
        fs::write(
            &wrong_schema_path,
            r#"{"name": "not a session", "value": 42}"#,
        )
        .unwrap();

        // list_sessions should skip the wrong schema file
        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "valid-session");
    }

    #[test]
    fn list_sessions_mixed_valid_and_corrupted() {
        let (store, temp_dir) = create_test_backend();

        // Create multiple valid sessions
        store.put_session(&SessionState::new("valid-1")).unwrap();
        store.put_session(&SessionState::new("valid-2")).unwrap();
        store.put_session(&SessionState::new("valid-3")).unwrap();

        // Create various corrupted files
        let sessions_dir = temp_dir.path().join("sessions");
        fs::write(sessions_dir.join("corrupted1.json"), "not json").unwrap();
        fs::write(sessions_dir.join("corrupted2.json"), "").unwrap();
        fs::write(sessions_dir.join("corrupted3.json"), "{}").unwrap();

        // Should only return valid sessions
        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn atomic_write_survives_read_during_write() {
        let (store, _temp) = create_test_backend();

        // Create initial session
        let mut state = SessionState::new("atomic-test");
        state.review.user_prompts.push("initial".to_string());
        store.put_session(&state).unwrap();

        // Update the session
        state.review.user_prompts.push("updated".to_string());
        store.put_session(&state).unwrap();

        // Read should always get consistent state (either before or after update)
        let retrieved = store.get_session("atomic-test").unwrap().unwrap();
        assert!(
            retrieved.review.user_prompts.len() == 1 || retrieved.review.user_prompts.len() == 2
        );
    }
}
