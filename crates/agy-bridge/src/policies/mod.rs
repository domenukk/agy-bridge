//! Policy bridge for the Antigravity SDK.
//!
//! Maps Rust policy rules to the SDK's Python `policy.allow()` / `policy.deny()`
//! / ``policy.workspace_only()`` calls and `localharness` `PolicyConfig`.

pub mod path;
mod rules;

pub use path::*;
pub use rules::*;
