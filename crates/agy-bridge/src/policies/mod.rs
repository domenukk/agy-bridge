//! Policy bridge for the Antigravity SDK.
//!
//! Maps Rust policy rules to the SDK's Python `policy.allow()` / `policy.deny()`
//! / ``policy.workspace_only()`` calls.

pub mod path;
pub(crate) mod pyhook;
mod rules;

pub use path::*;
pub(crate) use pyhook::*;
pub use rules::*;
