//! Runtime configuration types: [`RuntimeConfig`] and [`BackendLogLevel`].

use std::time::Duration;

use super::{DEFAULT_CHANNEL_CAPACITY, DEFAULT_INTER_AGENT_DELAY, DEFAULT_SHUTDOWN_TIMEOUT};

/// Log verbosity for the agent backend runtime.
///
/// Controls the logging level of the underlying agent runtime. This is
/// intentionally backend-agnostic — consumers should not need to know
/// the implementation details of the runtime layer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendLogLevel {
    /// Errors only.
    Error,
    /// Warnings and errors (default — matches upstream SDK behavior).
    #[default]
    Warn,
    /// Informational messages (verbose — includes raw protocol traffic).
    Info,
    /// Full debug output.
    Debug,
}

impl BackendLogLevel {
    /// Return the lowercase string representation used by the Python side.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

impl std::fmt::Display for BackendLogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Configuration for the bridge runtime.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// Channel buffer size for the command channel.
    pub channel_capacity: usize,
    /// Timeout for joining the Python thread on shutdown.
    pub shutdown_timeout: Duration,
    /// Delay injected between successive chat commands to prevent burst requests.
    pub inter_agent_delay: Duration,
    /// Backend runtime log verbosity.
    ///
    /// Defaults to `Warn`, matching the upstream SDK's default behavior.
    /// Set to `Info` or `Debug` for verbose protocol-level diagnostics.
    pub backend_log_level: BackendLogLevel,
    /// Maximum number of consecutive model-quality error steps before the
    /// bridge aborts the stream. `None` uses the built-in default (3).
    /// Set to `0` to disable the limit entirely (pure SDK pass-through).
    pub max_consecutive_model_errors: Option<u32>,
    /// Maximum number of consecutive thinking-only/empty steps before the
    /// bridge aborts the stream. `None` uses the built-in default (500).
    /// Set to `0` to disable the limit entirely (pure SDK pass-through).
    pub max_consecutive_empty_steps: Option<u32>,
    /// Buffer size for streaming response channels.
    /// `None` uses the built-in default (256). Each chat call creates
    /// ~7 channels of this size.
    pub streaming_channel_buffer: Option<usize>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            inter_agent_delay: DEFAULT_INTER_AGENT_DELAY,
            backend_log_level: BackendLogLevel::default(),
            max_consecutive_model_errors: None,
            max_consecutive_empty_steps: None,
            streaming_channel_buffer: None,
        }
    }
}
