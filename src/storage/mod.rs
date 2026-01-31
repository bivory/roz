//! Storage backends for session state.

pub mod file;
pub mod memory;
pub mod traits;

pub use file::FileBackend;
pub use memory::MemoryBackend;
pub use traits::{MessageStore, SessionSummary};
