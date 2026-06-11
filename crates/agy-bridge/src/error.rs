//! Bridge error types and helpers for mapping Python exceptions to Rust errors.

use std::time::Duration;

use fast_rands::Rand;
use pyo3::prelude::*;

use crate::streaming::StreamError;

/// All errors that can occur in the bridge layer.
#[non_exhaustive]
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    /// The agent was not started or has been shut down before an operation was requested.
    #[error("Agent is not started or has been shut down")]
    AgentNotStarted,
    /// An exception was raised in the backend.
    #[error("Backend error: {message}")]
    BackendError {
        /// Formatted traceback or error message from backend.
        message: String,
    },

    /// A connection-level error from the Antigravity SDK.
    #[error("Connection error: {message}")]
    ConnectionError {
        /// Human-readable description of the connection failure.
        message: String,
    },

    /// Quota / rate-limit error (HTTP 429 or equivalent).
    #[error("Quota exceeded, retry after {retry_after:?}")]
    QuotaExceeded {
        /// Suggested wait duration before retrying.
        retry_after: Duration,
    },

    /// The internal command channel was closed unexpectedly.
    #[error("Channel closed: {message}")]
    ChannelClosed {
        /// Context about which channel closed.
        message: String,
    },

    /// The agent request was blocked due to safety violations.
    #[error("Blocked by safety filter")]
    Safety,

    /// The agent request reached the max tokens limit.
    #[error("Max tokens reached")]
    MaxTokens,

    /// Connection was permanently closed.
    #[error("Connection permanently closed: {message}")]
    ConnectionClosed {
        /// Human-readable descriptor.
        message: String,
    },

    /// An operation exceeded its configured timeout.
    #[error("Timeout after {duration:?}: {operation}")]
    Timeout {
        /// How long we waited before giving up.
        duration: Duration,
        /// Which operation timed out.
        operation: String,
    },

    /// An error originating from the streaming response layer.
    #[error(transparent)]
    Stream(StreamError),

    /// The provided configuration is invalid or self-contradictory.
    #[error("Invalid configuration: {message}")]
    InvalidConfig {
        /// Human-readable description of the configuration issue.
        message: String,
    },

    /// An I/O error occurred during a file or socket operation.
    #[error("I/O error: {message}")]
    Io {
        /// The original I/O error message.
        message: String,
        /// The category of I/O error.
        kind: std::io::ErrorKind,
    },
}

impl Error {
    /// Returns `true` if this error is potentially transient and the
    /// operation may succeed on retry with [`with_retry`].
    ///
    /// Currently retryable:
    /// - [`Error::ConnectionError`] — network-level failures
    /// - [`Error::QuotaExceeded`] — rate-limited, retry after backoff
    /// - Backend errors containing HTTP 503 — server overload
    ///
    /// Note: The agent's internal retry loop handles quota errors
    /// automatically via [`crate::quota::QuotaState`]. This method is
    /// primarily for consumers who want to retry at a higher level.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ConnectionError { .. } | Self::QuotaExceeded { .. } => true,
            Self::BackendError { message } => message.contains("503"),
            _ => false,
        }
    }

    /// Returns `true` if this error indicates a quota / rate-limit condition.
    ///
    /// Matches the structured [`Error::QuotaExceeded`] variant as well as
    /// backend errors whose message contains HTTP 429, 503, or
    /// `RESOURCE_EXHAUSTED` status indicators.
    #[must_use]
    pub fn is_quota_error(&self) -> bool {
        match self {
            Self::QuotaExceeded { .. } => true,
            Self::BackendError { message } => {
                message.contains("429")
                    || message.contains("503")
                    || message.contains("RESOURCE_EXHAUSTED")
            }
            _ => false,
        }
    }
}

/// Converts a Python exception into the most specific [`Error`] variant.
///
/// Checks for Antigravity SDK errors (connection, validation), Pydantic
/// validation errors, and Python `ImportError` before falling back to
/// [`Error::BackendError`] with a formatted traceback.
///
/// This impl is always compiled because `pyo3` is a mandatory dependency of
/// the bridge crate — the entire runtime requires it. If you depend on
/// `agy-bridge` as a library, `pyo3` will be linked transitively.
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::Io {
            message: err.to_string(),
            kind: err.kind(),
        }
    }
}

impl From<StreamError> for Error {
    fn from(err: StreamError) -> Self {
        let msg = err.message.to_lowercase();
        if msg.contains("safety") {
            Self::Safety
        } else if msg.contains("max tokens") || msg.contains("token limit") {
            Self::MaxTokens
        } else {
            Self::Stream(err)
        }
    }
}

#[doc(hidden)]
impl From<PyErr> for Error {
    fn from(err: PyErr) -> Self {
        Python::with_gil(|py| classify_py_error(py, &err))
    }
}

#[doc(hidden)]
impl From<Error> for PyErr {
    fn from(err: Error) -> Self {
        pyo3::exceptions::PyRuntimeError::new_err(err.to_string())
    }
}

/// Classify a Python exception into the most specific [`Error`] variant.
///
/// This is the single source of truth for mapping `PyErr` → [`Error`].
/// Both the [`From<PyErr>`] impl and any call sites that hold a `&PyErr`
/// (with the GIL already acquired) should use this function.
pub(crate) fn classify_py_error(py: Python<'_>, err: &PyErr) -> Error {
    if let Some(classified) = check_antigravity_error(py, err) {
        return classified;
    }
    if let Some(classified) = check_pydantic_error(py, err) {
        return classified;
    }
    if let Some(classified) = check_builtin_error(py, err) {
        return classified;
    }

    let message = format_backend_error(py, err);
    Error::BackendError { message }
}

fn check_antigravity_error(py: Python<'_>, err: &PyErr) -> Option<Error> {
    if let Ok(types_mod) = py.import_bound("google.antigravity.types") {
        if let Ok(conn_err_cls) = types_mod.getattr("AntigravityConnectionError")
            && err.is_instance_bound(py, &conn_err_cls)
        {
            return Some(Error::ConnectionError {
                message: err.to_string(),
            });
        }
        if let Ok(val_err_cls) = types_mod.getattr("AntigravityValidationError")
            && err.is_instance_bound(py, &val_err_cls)
        {
            return Some(Error::BackendError {
                message: err.to_string(),
            });
        }
    }
    None
}

fn check_pydantic_error(py: Python<'_>, err: &PyErr) -> Option<Error> {
    if let Ok(pydantic) = py.import_bound("pydantic")
        && let Ok(validation_err_cls) = pydantic.getattr("ValidationError")
        && err.is_instance_bound(py, &validation_err_cls)
    {
        return Some(Error::BackendError {
            message: err.to_string(),
        });
    }
    None
}

fn check_builtin_error(py: Python<'_>, err: &PyErr) -> Option<Error> {
    if let Ok(builtins) = py.import_bound("builtins") {
        if let Ok(import_err_cls) = builtins.getattr("ImportError")
            && err.is_instance_bound(py, &import_err_cls)
        {
            return Some(Error::BackendError {
                message: err.to_string(),
            });
        }
    } else {
        tracing::warn!("Failed to import Python builtins module, skipping ImportError check");
    }
    None
}

/// Format a backend exception into a human-readable string including traceback.
fn format_backend_error(py: Python<'_>, err: &PyErr) -> String {
    // Try to get the full traceback via traceback.format_exception.
    let formatted = py
        .import_bound("traceback")
        .and_then(|tb_mod| {
            tb_mod.call_method1(
                "format_exception",
                (
                    err.get_type_bound(py),
                    err.value_bound(py),
                    err.traceback_bound(py),
                ),
            )
        })
        .and_then(|lines| lines.extract::<Vec<String>>());

    match formatted {
        Ok(lines) => lines.join(""),
        Err(fmt_err) => {
            tracing::warn!(error = %fmt_err, "Failed to format backend traceback, using fallback");
            // Fall back to the inline traceback format that map_py_error used.
            let traceback = err.traceback_bound(py);
            traceback.as_ref().map_or_else(
                || err.to_string(),
                |tb| {
                    tb.format().map_or_else(
                        |tb_fmt_err| {
                            tracing::warn!(error = %tb_fmt_err, "Failed to format Python traceback");
                            err.to_string()
                        },
                        |tb_str| format!("{}\nTraceback:\n{}", err.value_bound(py), tb_str),
                    )
                },
            )
        }
    }
}

/// Run `f` with a timeout. Returns `Error::Timeout` if the future
/// does not complete within `timeout`.
///
/// # Errors
///
/// Returns `Error::Timeout` if the future exceeds the deadline,
/// or propagates whatever error `f` itself returns.
pub async fn with_timeout<F, T>(timeout: Duration, operation: &str, f: F) -> Result<T, Error>
where
    F: std::future::Future<Output = Result<T, Error>>,
{
    match tokio::time::timeout(timeout, f).await {
        Ok(result) => result,
        Err(_elapsed) => Err(Error::Timeout {
            duration: timeout,
            operation: operation.to_string(),
        }),
    }
}

/// Retry `f` with exponential backoff on connection errors.
///
/// Retries up to `max_retries` times with delays of 2s, 4s, 8s, … capped at 120s.
/// Only [`Error::ConnectionError`] triggers a retry; all other errors —
/// including [`Error::QuotaExceeded`] — propagate immediately.
///
/// This distinction is intentional: quota / rate-limit errors are handled
/// separately by [`crate::quota::QuotaState`], which manages per-model
/// backoff and concurrency. Use [`Error::is_retryable()`] to check whether
/// an error *could* be retried at a higher level; this function implements
/// the narrower retry policy for transient network failures only.
///
/// # Errors
///
/// Returns the last `Error::ConnectionError` if all retries are exhausted,
/// or any non-retryable error from `f`.
pub async fn with_retry<F, Fut, T>(max_retries: u32, operation: &str, mut f: F) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Error>>,
{
    let mut attempt = 0u32;
    loop {
        match f().await {
            Ok(val) => return Ok(val),
            Err(Error::ConnectionError { ref message }) => {
                attempt += 1;
                if attempt > max_retries {
                    tracing::error!(
                        attempts = attempt,
                        operation,
                        "All retries exhausted for connection error: {message}"
                    );
                    return Err(Error::ConnectionError {
                        message: message.clone(),
                    });
                }
                let backoff = backoff_duration(attempt);
                tracing::warn!(
                    attempt,
                    max_retries,
                    backoff_ms = u64::try_from(backoff.as_millis()).unwrap_or_else(|e| {
                        tracing::warn!("Int conversion failed: {}", e);
                        u64::MAX
                    }),
                    operation,
                    "Connection error, retrying: {message}"
                );
                tokio::time::sleep(backoff).await;
            }
            Err(other) => return Err(other),
        }
    }
}

pub(crate) const MAX_BACKOFF_SECS: u64 = 120;

/// Base for the exponential backoff calculation (e.g. 2^n).
const BACKOFF_EXPONENT_BASE: u64 = 2;
/// Conversion factor from seconds to milliseconds.
const MILLISECONDS_PER_SECOND: u64 = 1000;
/// Divisor to compute total jitter spread (e.g., base / 2 = 50% spread).
const JITTER_TOTAL_SPREAD_DIVISOR: u64 = 2;
/// Divisor to compute minimum jitter boundary (e.g., base / 4 = 25% lower bound).
const JITTER_MIN_SUBTRACT_DIVISOR: u64 = 4;

/// Compute exponential backoff duration with jitter: 2^attempt seconds,
/// capped at [`MAX_BACKOFF_SECS`], then jittered by ±25%.
///
/// `attempt` is 1-indexed (first retry = 1). Passing 0 is treated the same as
/// 1 because the value is clamped via [`u32::saturating_sub`].
///
/// Jitter is applied to avoid the thundering-herd problem when many callers
/// retry simultaneously.
fn backoff_duration(attempt: u32) -> Duration {
    let attempt = attempt.max(1);
    let base_secs = BACKOFF_EXPONENT_BASE
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(MAX_BACKOFF_SECS)
        .min(MAX_BACKOFF_SECS);
    let base_ms = base_secs.saturating_mul(MILLISECONDS_PER_SECOND);
    // Apply ±25% jitter: range is [75%, 125%] of base_ms.
    let jitter_range = base_ms / JITTER_TOTAL_SPREAD_DIVISOR; // 50% total spread
    let jitter_min = base_ms.saturating_sub(base_ms / JITTER_MIN_SUBTRACT_DIVISOR);
    let jittered_ms = if jitter_range == 0 {
        base_ms
    } else {
        let limit = u32::try_from(jitter_range).unwrap_or_else(|e| {
            tracing::warn!("Int conversion failed: {}", e);
            u32::MAX
        });
        jitter_min
            + (fast_rands::StdRand::new().between(0, limit.saturating_sub(1) as usize) as u64)
    };
    Duration::from_millis(jittered_ms)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[test]
    fn test_stream_error_conversion() {
        let safety_err = StreamError {
            message: "Step error (status=ERROR): Candidate blocked by safety".to_string(),
        };
        let mapped_safety = Error::from(safety_err);
        assert!(matches!(mapped_safety, Error::Safety));

        let max_tokens_err = StreamError {
            message: "Step error (status=ERROR): Max tokens reached".to_string(),
        };
        let mapped_max_tokens = Error::from(max_tokens_err);
        assert!(matches!(mapped_max_tokens, Error::MaxTokens));

        let other_err = StreamError {
            message: "Some other connection issue".to_string(),
        };
        let mapped_other = Error::from(other_err);
        match mapped_other {
            Error::Stream(e) => {
                assert_eq!(e.message, "Some other connection issue");
            }
            other => panic!("Expected Error::Stream, got: {other:?}"),
        }
    }

    #[test]
    fn test_backend_error_from_pyerr() {
        pyo3::prepare_freethreaded_python();
        let err = Python::with_gil(|py| {
            let result: PyResult<()> =
                py.run_bound("raise ValueError('test error 42')", None, None);
            result.unwrap_err()
        });

        let bridge_err: Error = err.into();
        match &bridge_err {
            Error::BackendError { message } => {
                assert!(
                    message.contains("ValueError"),
                    "Expected 'ValueError' in message, got: {message}"
                );
                assert!(
                    message.contains("test error 42"),
                    "Expected 'test error 42' in message, got: {message}"
                );
            }
            other => panic!("Expected BackendError, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_timeout_triggers() {
        let short_timeout = Duration::from_millis(50);
        let result: Result<(), Error> = with_timeout(short_timeout, "test_op", async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(())
        })
        .await;

        match result {
            Err(Error::Timeout {
                duration,
                operation,
            }) => {
                assert_eq!(duration, short_timeout);
                assert_eq!(operation, "test_op");
            }
            other => panic!("Expected Timeout, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_timeout_succeeds_when_fast() {
        let result = with_timeout(Duration::from_secs(5), "fast_op", async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let counter = AtomicU32::new(0);
        let result = with_retry(3, "test_retry", || {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt < 2 {
                    Err(Error::ConnectionError {
                        message: "transient".to_string(),
                    })
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let counter = AtomicU32::new(0);
        let result: Result<i32, Error> = with_retry(2, "doomed", || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(Error::ConnectionError {
                    message: "always fails".to_string(),
                })
            }
        })
        .await;

        assert!(matches!(result, Err(Error::ConnectionError { .. })));
        // 1 initial + 2 retries = 3 total attempts
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_does_not_retry_non_connection_errors() {
        let counter = AtomicU32::new(0);
        let result: Result<i32, Error> = with_retry(5, "python_err", || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(Error::BackendError {
                    message: "kaboom".to_string(),
                })
            }
        })
        .await;

        assert!(matches!(result, Err(Error::BackendError { .. })));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_backoff_duration_progression() {
        // With ±25% jitter, each base duration should fall in [75%, 125%] of base.
        let bases_ms: [(u32, u64); 6] = [
            (1, 2_000),
            (2, 4_000),
            (3, 8_000),
            (4, 16_000),
            (7, 120_000),   // capped
            (100, 120_000), // overflow → capped
        ];
        for (attempt, base_ms) in bases_ms {
            let d = backoff_duration(attempt);
            let lo = base_ms * 3 / 4;
            let hi = base_ms * 5 / 4;
            assert!(
                d.as_millis() >= u128::from(lo) && d.as_millis() <= u128::from(hi),
                "backoff_duration({attempt}) = {d:?} outside [{lo}ms, {hi}ms]"
            );
        }
    }

    #[test]
    fn test_error_display_messages() {
        let err = Error::BackendError {
            message: "test".to_string(),
        };
        assert_eq!(format!("{err}"), "Backend error: test");

        let err = Error::ConnectionError {
            message: "lost".to_string(),
        };
        assert_eq!(format!("{err}"), "Connection error: lost");

        let err = Error::QuotaExceeded {
            retry_after: Duration::from_secs(5),
        };
        assert!(format!("{err}").contains("5s"));

        let err = Error::ChannelClosed {
            message: "cmd".to_string(),
        };
        assert_eq!(format!("{err}"), "Channel closed: cmd");

        let err = Error::Timeout {
            duration: Duration::from_secs(30),
            operation: "chat".to_string(),
        };
        assert!(format!("{err}").contains("chat"));
    }

    #[test]
    fn test_backoff_duration_zero_attempt() {
        // Attempt 0 should be treated as attempt 1 → base 2s, jittered [1.5s, 2.5s].
        let d = backoff_duration(0);
        assert!(
            d.as_millis() >= 1500 && d.as_millis() <= 2500,
            "backoff_duration(0) = {d:?} outside [1500ms, 2500ms]"
        );
    }

    #[test]
    fn test_backoff_duration_large_attempt_capped() {
        // Very large attempt numbers should be capped at base=120s, jittered [90s, 150s].
        let d = backoff_duration(u32::MAX);
        assert!(
            d.as_millis() >= 90_000 && d.as_millis() <= 150_000,
            "backoff_duration(u32::MAX) = {d:?} outside [90s, 150s]"
        );
    }

    #[tokio::test]
    async fn test_timeout_propagates_inner_error() {
        let result: Result<(), Error> = with_timeout(Duration::from_secs(10), "inner_err", async {
            Err(Error::BackendError {
                message: "inner failure".to_string(),
            })
        })
        .await;

        match result {
            Err(Error::BackendError { message }) => {
                assert_eq!(message, "inner failure");
            }
            other => panic!("Expected BackendError, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_retry_zero_max_retries_still_runs_once() {
        let counter = AtomicU32::new(0);
        let result: Result<i32, Error> = with_retry(0, "no_retries", || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(Error::ConnectionError {
                    message: "fail".to_string(),
                })
            }
        })
        .await;

        assert!(matches!(result, Err(Error::ConnectionError { .. })));
        // 1 initial attempt, 0 retries = 1 total
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_succeeds_on_first_attempt() {
        let counter = AtomicU32::new(0);
        let result = with_retry(5, "instant_success", || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok(99) }
        })
        .await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_quota_exceeded_not_retried() {
        let counter = AtomicU32::new(0);
        let result: Result<i32, Error> = with_retry(5, "quota", || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(Error::QuotaExceeded {
                    retry_after: Duration::from_secs(1),
                })
            }
        })
        .await;

        assert!(matches!(result, Err(Error::QuotaExceeded { .. })));
        // QuotaExceeded is not ConnectionError, so no retry
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_timeout_not_retried() {
        let counter = AtomicU32::new(0);
        let result: Result<i32, Error> = with_retry(5, "timeout", || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(Error::Timeout {
                    duration: Duration::from_secs(10),
                    operation: "test".to_string(),
                })
            }
        })
        .await;

        assert!(matches!(result, Err(Error::Timeout { .. })));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_channel_closed_not_retried() {
        let counter = AtomicU32::new(0);
        let result: Result<i32, Error> = with_retry(5, "channel", || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err(Error::ChannelClosed {
                    message: "gone".to_string(),
                })
            }
        })
        .await;

        assert!(matches!(result, Err(Error::ChannelClosed { .. })));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_error_debug_format() {
        let err = Error::BackendError {
            message: "debug test".to_string(),
        };
        let debug = format!("{err:?}");
        assert!(debug.contains("BackendError"));
        assert!(debug.contains("debug test"));
    }

    #[test]
    fn test_backoff_duration_full_progression() {
        // Verify the complete exponential progression with ±25% jitter.
        let base_secs: [u64; 8] = [2, 4, 8, 16, 32, 64, 120, 120];
        for (i, base) in base_secs.iter().enumerate() {
            let attempt = u32::try_from(i + 1).unwrap();
            let d = backoff_duration(attempt);
            let base_ms = base * 1000;
            let lo = base_ms * 3 / 4;
            let hi = base_ms * 5 / 4;
            assert!(
                d.as_millis() >= u128::from(lo) && d.as_millis() <= u128::from(hi),
                "backoff_duration({attempt}) = {d:?} outside [{lo}ms, {hi}ms]"
            );
        }
    }

    #[test]
    fn test_stream_error_from_conversion() {
        let stream_err = StreamError {
            message: "connection reset".to_string(),
        };
        let bridge_err = Error::from(stream_err);
        match &bridge_err {
            Error::Stream(inner) => {
                assert_eq!(inner.message, "connection reset");
            }
            other => panic!("Expected Stream variant, got: {other:?}"),
        }
    }

    #[test]
    fn test_stream_error_display_through_bridge() {
        let stream_err = StreamError {
            message: "quota exceeded".to_string(),
        };
        let bridge_err = Error::from(stream_err);
        let display = format!("{bridge_err}");
        assert!(
            display.contains("quota exceeded"),
            "Expected 'quota exceeded' in display, got: {display}"
        );
    }

    #[test]
    fn test_is_retryable_connection_error() {
        let err = Error::ConnectionError {
            message: "timeout".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_quota_exceeded_is_retryable() {
        let err = Error::QuotaExceeded {
            retry_after: Duration::from_secs(5),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_is_not_retryable_backend_error() {
        let err = Error::BackendError {
            message: "kaboom".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_is_not_retryable_channel_closed() {
        let err = Error::ChannelClosed {
            message: "gone".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_is_not_retryable_timeout() {
        let err = Error::Timeout {
            duration: Duration::from_secs(30),
            operation: "chat".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_is_not_retryable_stream() {
        let err = Error::Stream(StreamError {
            message: "stream failed".to_string(),
        });
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_is_retryable_503_backend_error() {
        let err = Error::BackendError {
            message: "request failed (code 503): high demand".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_is_quota_error_quota_exceeded() {
        let err = Error::QuotaExceeded {
            retry_after: Duration::from_secs(5),
        };
        assert!(err.is_quota_error());
    }

    #[test]
    fn test_is_quota_error_backend_429() {
        let err = Error::BackendError {
            message: "HTTP 429 Too Many Requests".to_string(),
        };
        assert!(err.is_quota_error());
    }

    #[test]
    fn test_is_quota_error_resource_exhausted() {
        let err = Error::BackendError {
            message: "RESOURCE_EXHAUSTED: quota exceeded".to_string(),
        };
        assert!(err.is_quota_error());
    }

    #[test]
    fn test_is_not_quota_error_connection() {
        let err = Error::ConnectionError {
            message: "timeout".to_string(),
        };
        assert!(!err.is_quota_error());
    }

    #[test]
    fn test_is_not_quota_error_normal_backend() {
        let err = Error::BackendError {
            message: "something else".to_string(),
        };
        assert!(!err.is_quota_error());
    }

    #[test]
    fn test_is_quota_error_503_high_demand() {
        let err = Error::BackendError {
            message: "request failed (code 503): This model is currently experiencing high demand"
                .to_string(),
        };
        assert!(err.is_quota_error());
    }
}
