//! Hook bridge for the Antigravity SDK.
//!
//! Defines Rust-side hook types that wrap callbacks for agent lifecycle
//! hook points: pre-turn, post-turn, pre-tool-call-decide, post-tool-call,
//! compaction, session start/end, tool errors, user interactions, and
//! tool-input transformation.
//!
//! The actual Python wrapping (creating `PyO3` classes that the SDK dispatches to)
//! requires the Python runtime and is gated behind integration tests.

mod interactive;
mod runner;
mod types;

pub use interactive::*;
pub use runner::*;
pub use types::*;
