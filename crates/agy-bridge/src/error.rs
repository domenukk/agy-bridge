//! Bridge error types and helpers for mapping Python exceptions to Rust errors.

use std::time::Duration;

use pyo3::prelude::*;

use crate::streaming::StreamError;

/// HTTP status code for `Too Many Requests` (429).
pub const HTTP_TOO_MANY_REQUESTS: u16 = 429;

/// Start of the HTTP server error 5xx status code range (500).
pub const HTTP_SERVER_ERROR_MIN: u16 = 500;

/// HTTP status code for `Service Unavailable` (503).
pub const HTTP_SERVICE_UNAVAILABLE: u16 = 503;

/// End of the HTTP server error 5xx status code range (599).
pub const HTTP_SERVER_ERROR_MAX: u16 = 599;

/// Unset / unknown HTTP status code (`0`).
pub const HTTP_CODE_UNKNOWN: u16 = 0;

/// Antigravity SDK connection error exception class name.
const PY_CLASS_ANTIGRAVITY_CONNECTION_ERROR: &str = "AntigravityConnectionError";

/// Antigravity SDK validation error exception class name.
const PY_CLASS_ANTIGRAVITY_VALIDATION_ERROR: &str = "AntigravityValidationError";

/// Pydantic validation error exception class name.
const PY_CLASS_PYDANTIC_VALIDATION_ERROR: &str = "ValidationError";

/// Python traceback module name.
const PY_MODULE_TRACEBACK: &str = "traceback";

/// Python traceback `format_exception` function name.
const PY_FN_FORMAT_EXCEPTION: &str = "format_exception";

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
    /// - Stream errors carrying a retryable HTTP status (429 or any 5xx)
    /// - Backend errors reporting `RESOURCE_EXHAUSTED` or HTTP 503
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ConnectionError { .. } | Self::QuotaExceeded { .. } => true,
            Self::Stream(se) if se.http_code != HTTP_CODE_UNKNOWN => {
                http_code_is_retryable(se.http_code)
            }
            Self::BackendError { message } | Self::Stream(StreamError { message, .. }) => {
                message.contains("RESOURCE_EXHAUSTED")
                    || message.contains("429")
                    || message.contains("503")
            }
            _ => false,
        }
    }

    /// Returns `true` if this error indicates a quota / rate-limit condition.
    ///
    /// Matches the structured [`Error::QuotaExceeded`] variant, stream errors
    /// carrying a quota HTTP status (429 or 503), and backend/stream messages
    /// reporting `RESOURCE_EXHAUSTED`, HTTP 429, or HTTP 503.
    #[must_use]
    pub fn is_quota_error(&self) -> bool {
        match self {
            Self::QuotaExceeded { .. } => true,
            Self::Stream(se) if se.http_code != HTTP_CODE_UNKNOWN => {
                http_code_is_quota(se.http_code)
            }
            Self::BackendError { message } | Self::Stream(StreamError { message, .. }) => {
                message.contains("RESOURCE_EXHAUSTED")
                    || message.contains("429")
                    || message.contains("503")
            }
            _ => false,
        }
    }
}

/// Whether an HTTP status code denotes a quota / rate-limit condition.
///
/// `429 Too Many Requests` (`RESOURCE_EXHAUSTED`) and `503 Service Unavailable`
/// (model overload / "high demand") both warrant quota-style backoff, matching
/// the harness's own retry guidance.
#[must_use]
pub const fn http_code_is_quota(code: u16) -> bool {
    matches!(code, HTTP_TOO_MANY_REQUESTS | HTTP_SERVICE_UNAVAILABLE)
}

/// Whether an HTTP status code denotes a transiently retryable failure.
///
/// Rate limits (`429`) and any server-side `5xx` are transient: the harness
/// logs a warning and continues iterating, so a higher-level retry may succeed.
#[must_use]
pub const fn http_code_is_retryable(code: u16) -> bool {
    code == HTTP_TOO_MANY_REQUESTS || matches!(code, HTTP_SERVER_ERROR_MIN..=HTTP_SERVER_ERROR_MAX)
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
    match err.get_type(py).name() {
        Ok(name) => {
            if name == PY_CLASS_ANTIGRAVITY_CONNECTION_ERROR {
                return Some(Error::ConnectionError {
                    message: err.to_string(),
                });
            }
            if name == PY_CLASS_ANTIGRAVITY_VALIDATION_ERROR {
                return Some(Error::BackendError {
                    message: err.to_string(),
                });
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, "Failed to get exception type name for antigravity check");
        }
    }
    None
}

fn check_pydantic_error(py: Python<'_>, err: &PyErr) -> Option<Error> {
    match err.get_type(py).name() {
        Ok(name) if name == PY_CLASS_PYDANTIC_VALIDATION_ERROR => Some(Error::BackendError {
            message: err.to_string(),
        }),
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(error = %e, "Failed to get exception type name for pydantic check");
            None
        }
    }
}

fn check_builtin_error(py: Python<'_>, err: &PyErr) -> Option<Error> {
    if err.is_instance_of::<pyo3::exceptions::PyImportError>(py) {
        return Some(Error::BackendError {
            message: err.to_string(),
        });
    }
    None
}

/// Format a backend exception into a human-readable string including traceback.
fn format_backend_error(py: Python<'_>, err: &PyErr) -> String {
    // Try to get the full traceback via traceback.format_exception(exc).
    let formatted = py
        .import(PY_MODULE_TRACEBACK)
        .and_then(|tb_mod| tb_mod.call_method1(PY_FN_FORMAT_EXCEPTION, (err.value(py),)))
        .and_then(|lines| lines.extract::<Vec<String>>());

    match formatted {
        Ok(lines) => lines.join(""),
        Err(fmt_err) => {
            tracing::warn!(error = %fmt_err, "Failed to format backend traceback, using fallback");
            // Fall back to the inline traceback format.
            let traceback = err.traceback(py);
            traceback.as_ref().map_or_else(
                || err.to_string(),
                |tb| {
                    tb.format().map_or_else(
                        |tb_fmt_err| {
                            tracing::warn!(error = %tb_fmt_err, "Failed to format Python traceback");
                            err.to_string()
                        },
                        |tb_str| format!("{err}\nTraceback:\n{tb_str}"),
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
        let safety_err = StreamError::new("Step error (status=ERROR): Candidate blocked by safety");
        let mapped_safety = Error::from(safety_err);
        assert!(
            matches!(mapped_safety, Error::Stream(_)),
            "StreamError with 'safety' should pass through as Error::Stream"
        );

        let max_tokens_err = StreamError::new("Step error (status=ERROR): Max tokens reached");
        let mapped_max_tokens = Error::from(max_tokens_err);
        assert!(
            matches!(mapped_max_tokens, Error::Stream(_)),
            "StreamError with 'max tokens' should pass through as Error::Stream"
        );

        let other_err = StreamError::new("Some other connection issue");
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
        let stream_err = StreamError::new("connection reset");
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
        let stream_err = StreamError::new("quota exceeded");
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
        let err = Error::Stream(StreamError::new("stream failed"));
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

    #[test]
    fn test_stream_http_code_429_is_quota_and_retryable() {
        // A structured 429 classifies as quota + retryable regardless of the
        // (deliberately unhelpful) message text.
        let err = Error::Stream(StreamError::with_http_code("rate limited", 429));
        assert!(err.is_quota_error());
        assert!(err.is_retryable());
    }

    #[test]
    fn test_stream_http_code_503_is_quota_and_retryable() {
        let err = Error::Stream(StreamError::with_http_code("service unavailable", 503));
        assert!(err.is_quota_error());
        assert!(err.is_retryable());
    }

    #[test]
    fn test_stream_http_code_500_is_retryable_not_quota() {
        // Generic 5xx is transiently retryable but is not a quota condition.
        let err = Error::Stream(StreamError::with_http_code("internal error", 500));
        assert!(err.is_retryable());
        assert!(!err.is_quota_error());
    }

    #[test]
    fn test_stream_http_code_400_is_neither() {
        // Client errors (e.g. bad request) are terminal: not retryable, not quota.
        let err = Error::Stream(StreamError::with_http_code("bad request", 400));
        assert!(!err.is_retryable());
        assert!(!err.is_quota_error());
    }

    #[test]
    fn test_stream_http_code_is_authoritative_over_message() {
        // The structured code wins even when the message would substring-match
        // a quota indicator: a real 400 carrying the text "429" is still a
        // terminal client error, not a rate limit.
        let err = Error::Stream(StreamError::with_http_code(
            "error 429 mentioned in prose",
            400,
        ));
        assert!(!err.is_quota_error());
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_stream_unknown_http_code_falls_back_to_message() {
        // http_code == 0 (unknown, e.g. a Python-level exception) falls back to
        // substring classification so no signal is lost.
        let quota = Error::Stream(StreamError::new("HTTP 429 Too Many Requests"));
        assert!(quota.is_quota_error());
        assert!(quota.is_retryable());

        let plain = Error::Stream(StreamError::new("some unrelated failure"));
        assert!(!plain.is_quota_error());
        assert!(!plain.is_retryable());
    }
}
