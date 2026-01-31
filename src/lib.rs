//! roz - Quality gate for Claude Code.
//!
//! Mechanically enforces independent review before task completion.
//! Named after Monsters Inc.'s Roz ("I'm watching you, Wazowski. Always watching.").

pub mod cli;
pub mod config;
pub mod core;
pub mod error;
pub mod hooks;
pub mod storage;
pub mod template;

pub use config::Config;
pub use error::{Error, Result};
