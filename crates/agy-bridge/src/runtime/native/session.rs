//! Session state for an active agent in the native runtime.

use std::{
    collections::HashSet,
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
    /// Custom tool names that terminate the active turn on a successful call.
    pub(crate) finish_on_tools: Arc<HashSet<String>>,
    /// Set when a tool in `finish_on_tools` completes with `Ok` during the turn.
    pub(crate) finished_by_tool: Arc<AtomicBool>,
    /// Storage directory associated with this session's localharness process.
    pub(crate) save_dir: PathBuf,
    /// OS command sandbox status reported by the harness.
    pub(crate) sandbox_status: Option<crate::types::SandboxStatus>,
}

/// Initial state returned by `initialize_harness` when starting or resuming a conversation.
pub(crate) struct InitialHarnessState {
    pub(crate) cascade_id: Option<String>,
    pub(crate) usage: UsageMetadata,
    pub(crate) sandbox_status: Option<crate::types::SandboxStatus>,
    pub(crate) history: Vec<ConversationMessage>,
    pub(crate) last_response_text: Option<String>,
    pub(crate) compaction_indices: Vec<u32>,
    pub(crate) turn_count: u32,
}

impl NativeAgentSession {
    pub(crate) fn new(
        event_tx: mpsc::Sender<proto::localharness::InputEvent>,
        init: InitialHarnessState,
        save_dir: PathBuf,
        finish_on_tools: HashSet<String>,
    ) -> Self {
        Self {
            event_tx,
            history: Arc::new(Mutex::new(init.history)),
            total_usage: Arc::new(Mutex::new(init.usage)),
            last_turn_usage: Arc::new(Mutex::new(UsageMetadata::default())),
            last_response_text: Arc::new(Mutex::new(init.last_response_text)),
            compaction_indices: Arc::new(Mutex::new(init.compaction_indices)),
            turn_count: Arc::new(AtomicU32::new(init.turn_count)),
            is_idle: Arc::new(AtomicBool::new(true)),
            idle_notify: Arc::new(Notify::new()),
            wakeup_notify: Arc::new(Notify::new()),
            active_writer: Arc::new(tokio::sync::Mutex::new(None)),
            last_error: Arc::new(Mutex::new(None)),
            produced_output: Arc::new(AtomicBool::new(false)),
            turn_activity: Arc::new(AtomicBool::new(false)),
            connected: Arc::new(AtomicBool::new(true)),
            finish_on_tools: Arc::new(finish_on_tools),
            finished_by_tool: Arc::new(AtomicBool::new(false)),
            save_dir,
            sandbox_status: init.sandbox_status,
        }
    }
}
