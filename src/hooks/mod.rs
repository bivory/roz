//! Hook input/output types and dispatch.

pub mod input;
pub mod output;
pub mod runner;

pub use input::HookInput;
pub use output::{HookDecision, HookOutput, PermissionDecision, PreToolUseOutput};
pub use runner::dispatch_hook;
