//! Session state for an active agent in the native runtime.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32},
    },
};

use tokio::sync::{Notify, mpsc};

use crate::{
    proto,
    streaming::ChatResponseWriter,
    types::{ConversationMessage, UsageMetadata},
};

/// State for a single active agent session in the native runtime.
pub(crate) struct NativeAgentSession {
    pub(crate) event_tx: mpsc::Sender<proto::localharness::InputEvent>,
    pub(crate) history: Arc<Mutex<Vec<ConversationMessage>>>,
    pub(crate) total_usage: Arc<Mutex<UsageMetadata>>,
    pub(crate) last_turn_usage: Arc<Mutex<UsageMetadata>>,
    pub(crate) last_response_text: Arc<Mutex<Option<String>>>,
    pub(crate) compaction_indices: Arc<Mutex<Vec<u32>>>,
    pub(crate) turn_count: Arc<AtomicU32>,
    pub(crate) is_idle: Arc<AtomicBool>,
    pub(crate) idle_notify: Arc<Notify>,
    pub(crate) wakeup_notify: Arc<Notify>,
    pub(crate) active_writer: Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
    pub(crate) last_error: Arc<Mutex<Option<crate::streaming::StreamError>>>,
    pub(crate) produced_output: Arc<AtomicBool>,
    /// Set whenever the harness emits any activity (a step or tool call) during
    /// the current turn. Used to detect abnormal empty completions where a
    /// failed trajectory silently returns to idle without output or error.
    pub(crate) turn_activity: Arc<AtomicBool>,
    /// `false` once the harness WebSocket has closed. A disconnected session
    /// can no longer run turns; `chat` rejects new turns instead of hanging.
    pub(crate) connected: Arc<AtomicBool>,
    /// Storage directory associated with this session's localharness process.
    pub(crate) save_dir: PathBuf,
}
