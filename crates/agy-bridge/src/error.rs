//! Bridge error types and helpers for mapping Python exceptions to Rust errors.

use std::time::Duration;

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
    /// operation may succeed if retried at a higher level.
    ///
    /// Currently retryable:
    /// - [`Error::ConnectionError`] — network-level failures
    /// - [`Error::QuotaExceeded`] — rate-limited, retry after backoff
    /// - Backend errors containing HTTP 503 — server overload
    ///
    /// Classification only: the bridge itself is single-shot, so retrying is
    /// the caller's responsibility (e.g. via `agent_resilience::backoff`).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ConnectionError { .. } | Self::QuotaExceeded { .. } => true,
            Self::BackendError { message } => message.contains("503"),
            Self::Stream(se) => se.message.contains("503") || se.message.contains("429"),
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
            Self::Stream(se) => {
                se.message.contains("429")
                    || se.message.contains("503")
                    || se.message.contains("quota")
                    || se.message.contains("RESOURCE_EXHAUSTED")
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
        Self::Stream(err)
    }
}

#[doc(hidden)]
impl From<PyErr> for Error {
    fn from(err: PyErr) -> Self {
        Python::attach(|py| classify_py_error(py, &err))
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
    match crate::runtime::py_scripts::import_serialized(py, "google.antigravity.types") {
        Ok(types_mod) => {
            // NOLINT: intentional fallthrough — if getattr fails, the type isn't available and we skip this check
            if let Ok(conn_err_cls) = types_mod.getattr("AntigravityConnectionError")
                && err.is_instance(py, &conn_err_cls)
            {
                return Some(Error::ConnectionError {
                    message: err.to_string(),
                });
            }
            // NOLINT: intentional fallthrough — if getattr fails, the type isn't available and we skip this check
            if let Ok(val_err_cls) = types_mod.getattr("AntigravityValidationError")
                && err.is_instance(py, &val_err_cls)
            {
                return Some(Error::BackendError {
                    message: err.to_string(),
                });
            }
        }
        Err(import_err) => {
            tracing::debug!(
                error = %import_err,
                "antigravity.types not available, skipping AntigravityError classification"
            );
        }
    }
    None
}

fn check_pydantic_error(py: Python<'_>, err: &PyErr) -> Option<Error> {
    match crate::runtime::py_scripts::import_serialized(py, "pydantic") {
        Ok(pydantic) => {
            // NOLINT: intentional fallthrough — if getattr fails, the type isn't available and we skip this check
            if let Ok(validation_err_cls) = pydantic.getattr("ValidationError")
                && err.is_instance(py, &validation_err_cls)
            {
                return Some(Error::BackendError {
                    message: err.to_string(),
                });
            }
        }
        Err(import_err) => {
            tracing::debug!(
                error = %import_err,
                "pydantic not available, skipping ValidationError classification"
            );
        }
    }
    None
}

fn check_builtin_error(py: Python<'_>, err: &PyErr) -> Option<Error> {
    // NOLINT: intentional fallthrough — if import fails, we skip the builtins check (logged in else)
    if let Ok(builtins) = py.import("builtins") {
        // NOLINT: intentional fallthrough — if getattr fails, the type isn't available and we skip this check
        if let Ok(import_err_cls) = builtins.getattr("ImportError")
            && err.is_instance(py, &import_err_cls)
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
        .import("traceback")
        .and_then(|tb_mod| {
            tb_mod.call_method1(
                "format_exception",
                (err.get_type(py), err.value(py), err.traceback(py)),
            )
        })
        .and_then(|lines| lines.extract::<Vec<String>>());

    match formatted {
        Ok(lines) => lines.join(""),
        Err(fmt_err) => {
            tracing::warn!(error = %fmt_err, "Failed to format backend traceback, using fallback");
            // Fall back to the inline traceback format that map_py_error used.
            let traceback = err.traceback(py);
            traceback.as_ref().map_or_else(
                || err.to_string(),
                |tb| {
                    tb.format().map_or_else(
                        |tb_fmt_err| {
                            tracing::warn!(error = %tb_fmt_err, "Failed to format Python traceback");
                            err.to_string()
                        },
                        |tb_str| format!("{}\nTraceback:\n{}", err.value(py), tb_str),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_error_conversion() {
        // All StreamErrors should pass through as Error::Stream — the bridge
        // does not interpret or reclassify stream error messages.
        let safety_err = StreamError {
            message: "Step error (status=ERROR): Candidate blocked by safety".to_string(),
        };
        let mapped_safety = Error::from(safety_err);
        assert!(
            matches!(mapped_safety, Error::Stream(_)),
            "StreamError with 'safety' should pass through as Error::Stream"
        );

        let max_tokens_err = StreamError {
            message: "Step error (status=ERROR): Max tokens reached".to_string(),
        };
        let mapped_max_tokens = Error::from(max_tokens_err);
        assert!(
            matches!(mapped_max_tokens, Error::Stream(_)),
            "StreamError with 'max tokens' should pass through as Error::Stream"
        );

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
        Python::initialize();
        let err = Python::attach(|py| {
            let result: PyResult<()> = py.run(c"raise ValueError('test error 42')", None, None);
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
