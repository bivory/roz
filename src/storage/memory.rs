//! In-memory storage backend for testing.

use crate::core::SessionState;
use crate::error::Result;
use crate::storage::traits::{MessageStore, SessionSummary};
use std::collections::HashMap;
use std::sync::RwLock;

/// In-memory storage backend for testing.
#[derive(Debug, Default)]
pub struct MemoryBackend {
    sessions: RwLock<HashMap<String, SessionState>>,
}

impl MemoryBackend {
    /// Create a new in-memory backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl MessageStore for MemoryBackend {
    fn get_session(&self, session_id: &str) -> Result<Option<SessionState>> {
        let sessions = self.sessions.read().unwrap();
        Ok(sessions.get(session_id).cloned())
    }

    fn put_session(&self, state: &SessionState) -> Result<()> {
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(state.session_id.clone(), state.clone());
        Ok(())
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let sessions = self.sessions.read().unwrap();
        let mut summaries: Vec<SessionSummary> = sessions
            .values()
            .map(|state| SessionSummary {
                session_id: state.session_id.clone(),
                first_prompt: state.review.user_prompts.first().cloned(),
                created_at: state.created_at,
                event_count: state.trace.len(),
            })
            .collect();

        // Sort by created_at descending (most recent first)
        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        summaries.truncate(limit);
        Ok(summaries)
    }

    fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().unwrap();
        sessions.remove(session_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_missing_session() {
        let store = MemoryBackend::new();
        let result = store.get_session("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn put_and_get_session() {
        let store = MemoryBackend::new();
        let state = SessionState::new("test-123");

        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-123").unwrap().unwrap();
        assert_eq!(retrieved.session_id, "test-123");
    }

    #[test]
    fn list_sessions_empty() {
        let store = MemoryBackend::new();
        let sessions = store.list_sessions(10).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_with_data() {
        let store = MemoryBackend::new();

        store.put_session(&SessionState::new("session-1")).unwrap();
        store.put_session(&SessionState::new("session-2")).unwrap();

        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn list_sessions_respects_limit() {
        let store = MemoryBackend::new();

        for i in 0..5 {
            store
                .put_session(&SessionState::new(&format!("session-{i}")))
                .unwrap();
        }

        let sessions = store.list_sessions(3).unwrap();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn delete_session_removes_session() {
        let store = MemoryBackend::new();
        let state = SessionState::new("test-123");

        store.put_session(&state).unwrap();
        assert!(store.get_session("test-123").unwrap().is_some());

        store.delete_session("test-123").unwrap();
        assert!(store.get_session("test-123").unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_session_succeeds() {
        let store = MemoryBackend::new();
        // Should not error when deleting non-existent session
        store.delete_session("nonexistent").unwrap();
    }

    #[test]
    fn concurrent_reads() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(MemoryBackend::new());

        // Create a session first
        store
            .put_session(&SessionState::new("concurrent-read"))
            .unwrap();

        // Spawn multiple reader threads
        let mut handles = vec![];
        for _ in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    let result = store_clone.get_session("concurrent-read").unwrap();
                    assert!(result.is_some());
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("Thread panicked");
        }
    }

    #[test]
    fn concurrent_writes() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(MemoryBackend::new());

        // Spawn multiple writer threads
        let mut handles = vec![];
        for i in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                for j in 0..10 {
                    let session_id = format!("concurrent-write-{i}-{j}");
                    let state = SessionState::new(&session_id);
                    store_clone.put_session(&state).unwrap();
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // Verify all sessions were created
        let sessions = store.list_sessions(1000).unwrap();
        assert_eq!(sessions.len(), 100); // 10 threads * 10 sessions each
    }

    #[test]
    fn concurrent_read_write() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(MemoryBackend::new());

        // Create initial session
        store
            .put_session(&SessionState::new("concurrent-rw"))
            .unwrap();

        let mut handles = vec![];

        // Spawn reader threads
        for _ in 0..5 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    let _ = store_clone.get_session("concurrent-rw");
                    let _ = store_clone.list_sessions(10);
                }
            });
            handles.push(handle);
        }

        // Spawn writer threads
        for i in 0..5 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                for j in 0..20 {
                    let session_id = format!("concurrent-rw-write-{i}-{j}");
                    let state = SessionState::new(&session_id);
                    store_clone.put_session(&state).unwrap();
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // Verify sessions exist (original + writes)
        let sessions = store.list_sessions(1000).unwrap();
        assert_eq!(sessions.len(), 101); // 1 initial + 5*20 written
    }

    #[test]
    fn concurrent_delete_and_read() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(MemoryBackend::new());

        // Create sessions to delete
        for i in 0..50 {
            store
                .put_session(&SessionState::new(&format!("delete-{i}")))
                .unwrap();
        }

        let mut handles = vec![];

        // Spawn deleter threads
        for i in 0..5 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                for j in 0..10 {
                    let session_id = format!("delete-{}", i * 10 + j);
                    store_clone.delete_session(&session_id).unwrap();
                }
            });
            handles.push(handle);
        }

        // Spawn reader threads that try to read the same sessions
        for _ in 0..5 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                for i in 0..50 {
                    // This may or may not find the session (race condition)
                    let _ = store_clone.get_session(&format!("delete-{i}"));
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // All sessions should be deleted
        let sessions = store.list_sessions(1000).unwrap();
        assert_eq!(sessions.len(), 0);
    }
}
