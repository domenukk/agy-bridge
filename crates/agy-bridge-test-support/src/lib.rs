//! Shared mock-server infrastructure for the `agy-bridge` integration tests.
//!
//! This is a dev-only library crate (a `[dev-dependencies]` of `agy-bridge`).
//! Hosting the shared helpers in a *library* — rather than a `tests/` submodule
//! included by several test binaries — means every `pub` item is treated as
//! public API, so helpers used by only some test binaries are never flagged as
//! dead code. The `agy-bridge` integration tests bring it in with
//! `use agy_bridge_test_support::*;`.
//!
//! **No API key required** — everything runs against local TCP mock servers.

use std::{
    fmt::Write as _,
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};

use agy_bridge::{
    policies::PolicyRule,
    tools::{JsonSchema, RustTool, ToolError, ToolOutput},
};
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

/// Environment variable overriding the per-test wall-clock timeout applied by
/// [`run_test`], in whole seconds.
pub const ENV_TEST_TIMEOUT_SECS: &str = "AGY_BRIDGE_TEST_TIMEOUT_SECS";

/// Default per-test wall-clock timeout, in seconds.
///
/// Generous enough for a cold `localharness` spawn or a cold Python SDK import
/// on a loaded CI runner, tight enough that a deadlock fails the job instead of
/// burning the runner's own timeout.
const DEFAULT_TEST_TIMEOUT_SECS: u64 = 120;

/// The bridge shared by every test in a process, handed out as `Arc` clones.
///
/// # Why not a `LazyLock` static
///
/// Rust never drops statics. The bridge owns the runtime, and the runtime's
/// `Drop` is the only thing that kills the `localharness` children the native
/// backend spawns — so a `static` fixture guarantees those children outlive the
/// test binary, leaving the harness's voluntary exit-on-stdin-EOF as the sole
/// safety net.
///
/// Caching a [`Weak`] instead keeps the deduplication property that matters for
/// memory (all tests overlapping in time share exactly one bridge, one runtime,
/// one harness) while restoring deterministic teardown: when the last
/// [`AgyBridge`](agy_bridge::AgyBridge) clone *and* the last
/// [`AgentHandle`](agy_bridge::Agent) built from it go away, the runtime drops
/// and reaps its children. A later caller simply builds a fresh one.
///
/// Callers should bind the result for the duration of the test
/// (`let bridge = shared_bridge();`) rather than calling this per statement.
///
/// # Panics
///
/// Panics if the bridge cannot be constructed.
#[must_use]
pub fn shared_bridge() -> agy_bridge::AgyBridge {
    static CACHE: Mutex<Weak<agy_bridge::DefaultRuntime>> = Mutex::new(Weak::new());

    // A poisoned fixture lock means some other test panicked while holding it;
    // the cached `Weak` is still perfectly valid, so recover rather than
    // cascading the panic into every remaining test.
    let mut cached = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(runtime) = cached.upgrade() {
        return agy_bridge::AgyBridge::new(runtime);
    }

    let bridge = agy_bridge::AgyBridge::builder()
        .inter_agent_delay(std::time::Duration::ZERO)
        .build()
        .expect("shared AgyBridge");
    *cached = Arc::downgrade(bridge.runtime());
    bridge
}

/// The per-test wall-clock timeout, honouring [`ENV_TEST_TIMEOUT_SECS`].
fn test_timeout() -> std::time::Duration {
    let secs = match std::env::var(ENV_TEST_TIMEOUT_SECS) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(0) => {
                eprintln!(
                    "[test-support] {ENV_TEST_TIMEOUT_SECS}=0 is not a usable timeout; \
                     falling back to {DEFAULT_TEST_TIMEOUT_SECS}s"
                );
                DEFAULT_TEST_TIMEOUT_SECS
            }
            Ok(n) => n,
            Err(parse_err) => {
                eprintln!(
                    "[test-support] {ENV_TEST_TIMEOUT_SECS} is not a number ({parse_err}); \
                     falling back to {DEFAULT_TEST_TIMEOUT_SECS}s"
                );
                DEFAULT_TEST_TIMEOUT_SECS
            }
        },
        // Unset is the normal case, not an error.
        Err(_unset) => DEFAULT_TEST_TIMEOUT_SECS,
    };
    std::time::Duration::from_secs(secs)
}

/// Drive a mock-server test to completion on a fresh runtime, under a
/// wall-clock timeout.
///
/// Use this instead of `multi_thread_rt().block_on(...)`: a hung await on a
/// mock server (or a deadlocked bridge) then fails the test with a clear
/// message instead of hanging until the CI runner is killed. Override the
/// budget with [`ENV_TEST_TIMEOUT_SECS`].
///
/// # Panics
///
/// Panics if `future` has not completed within the timeout.
pub fn run_test<F: std::future::Future<Output = ()>>(future: F) {
    let timeout = test_timeout();
    let runtime = multi_thread_rt();
    if let Err(elapsed) = runtime.block_on(tokio::time::timeout(timeout, future)) {
        panic!(
            "test exceeded its {timeout:?} budget ({elapsed}) — likely a hang; \
             override with {ENV_TEST_TIMEOUT_SECS}"
        );
    }
}

// ─── Tool Definitions ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddParams {
    /// First number.
    x: f64,
    /// Second number.
    y: f64,
}

pub struct AddTool;

impl RustTool for AddTool {
    type Params = AddParams;
    const NAME: &'static str = "add_numbers";
    const DESCRIPTION: &'static str = "Adds two numbers.";

    fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolOutput, ToolError>> + Send {
        let result = params.x + params.y;
        std::future::ready(Ok(format!("{result}").into()))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookupParams {
    /// Key to look up.
    key: String,
}

pub struct LookupTool;

impl RustTool for LookupTool {
    type Params = LookupParams;
    const NAME: &'static str = "lookup";
    const DESCRIPTION: &'static str = "Looks up a value by key.";

    fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolOutput, ToolError>> + Send {
        let val = match params.key.as_str() {
            "secret" => "GAMMA-42",
            "status" => "operational",
            _ => "not_found",
        };
        std::future::ready(Ok(val.into()))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FailParams {
    /// Reason for failure.
    reason: String,
}

pub struct AlwaysFailTool;

impl RustTool for AlwaysFailTool {
    type Params = FailParams;
    const NAME: &'static str = "always_fail";
    const DESCRIPTION: &'static str = "Always fails with the given reason.";

    fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolOutput, ToolError>> + Send {
        std::future::ready(Err(ToolError::new(params.reason)))
    }
}

/// A tool whose invocation count we can inspect from tests.
pub struct CountingTool {
    count: Arc<AtomicUsize>,
}

impl CountingTool {
    /// Create a counting tool alongside a shared handle to its invocation count.
    #[must_use]
    pub fn new() -> (Self, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        (
            Self {
                count: Arc::clone(&count),
            },
            count,
        )
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CountParams {}

impl RustTool for CountingTool {
    type Params = CountParams;
    const NAME: &'static str = "counting_tool";
    const DESCRIPTION: &'static str = "Increments a counter and returns the count.";

    fn call(
        &self,
        _params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolOutput, ToolError>> + Send {
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        std::future::ready(Ok(format!("invocation_{n}").into()))
    }
}

// ─── Mock Server Infrastructure ──────────────────────────────────────────────

/// Recorded POST body for inspection.
#[derive(Debug, Clone)]
pub struct RecordedPost {
    pub body: String,
}

/// A mock Gemini API server with configurable tool-call sequences.
pub struct MockGeminiServer {
    addr: std::net::SocketAddr,
    post_count: Arc<AtomicUsize>,
    posts: Arc<tokio::sync::Mutex<Vec<RecordedPost>>>,
    handle: tokio::task::JoinHandle<()>,
}

/// A response the mock server should return for each POST in sequence.
#[derive(Clone)]
pub enum MockResponse {
    /// Return a `functionCall` to invoke a tool.
    FunctionCall {
        name: String,
        args: serde_json::Value,
    },
    /// Return a final text response.
    Text(String),
    /// Return a candidate with empty content — no text, no tool calls.
    ///
    /// The SDK backend validates that model output contains either text or tool
    /// calls. When neither is present, it emits a step with
    /// `status=ERROR, error="model output must contain either output text or
    /// tool calls, these cannot both be empty, please try again"`.
    ///
    /// This variant exercises the SDK's internal retry + agy-bridge's
    /// recoverable-error classification path.
    EmptyCandidate,
    /// Return an HTTP error response (e.g. 503, 500, 429).
    ///
    /// The server responds with the given HTTP status code and a JSON error
    /// body. The SDK treats these as backend failures and may retry internally
    /// before surfacing the error.
    HttpError { status: u16, message: String },
    /// Return a sequence of SSE frames for a single request.
    ///
    /// Used to simulate multi-chunk streaming where the first chunk is an
    /// empty candidate (triggering the backend error) and the second is a
    /// normal text response (the retry result).
    Sequence(Vec<MockResponse>),
}

impl MockGeminiServer {
    /// Start a mock server that returns the given responses in sequence for POSTs.
    /// If there are more POSTs than responses, the last response is repeated.
    ///
    /// # Panics
    ///
    /// Panics if a local TCP listener cannot be bound — which only happens if
    /// the test host has exhausted ephemeral ports or loopback is unavailable.
    pub async fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let addr = listener.local_addr().expect("local addr");

        let post_count = Arc::new(AtomicUsize::new(0));
        let posts: Arc<tokio::sync::Mutex<Vec<RecordedPost>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let count = Arc::clone(&post_count);
        let recs = Arc::clone(&posts);

        let responses = Arc::new(responses);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let count = Arc::clone(&count);
                let recs = Arc::clone(&recs);
                let responses = Arc::clone(&responses);
                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut buf_reader = BufReader::new(reader);

                    while let Some((request_line, body)) =
                        parse_http_request_with_body(&mut buf_reader).await
                    {
                        if request_line.starts_with("GET ") {
                            let resp = json_response(200, &model_list_json());
                            if let Err(e) = writer.write_all(resp.as_bytes()).await {
                                eprintln!("mock server: write failed: {e}");
                                break;
                            }
                            if let Err(e) = writer.flush().await {
                                eprintln!("mock server: flush failed: {e}");
                                break;
                            }
                            continue;
                        }

                        let n = count.fetch_add(1, Ordering::SeqCst);
                        recs.lock().await.push(RecordedPost { body });

                        let idx = n.min(responses.len().saturating_sub(1));
                        let response = match &responses[idx] {
                            MockResponse::FunctionCall { name, args } => {
                                sse_response(&function_call_json(name, args))
                            }
                            MockResponse::Text(text) => sse_response(&text_response_json(text)),
                            MockResponse::EmptyCandidate => sse_response(&empty_candidate_json()),
                            MockResponse::HttpError { status, message } => {
                                http_error_response(*status, message)
                            }
                            MockResponse::Sequence(items) => {
                                let mut frames = String::new();
                                for item in items {
                                    let json = match item {
                                        MockResponse::Text(t) => text_response_json(t),
                                        MockResponse::FunctionCall { name, args } => {
                                            function_call_json(name, args)
                                        }
                                        MockResponse::EmptyCandidate => empty_candidate_json(),
                                        MockResponse::HttpError { .. }
                                        | MockResponse::Sequence(_) => {
                                            continue;
                                        }
                                    };
                                    write!(frames, "data: {json}\n\n")
                                        .expect("writing to a String is infallible");
                                }
                                format!(
                                    "HTTP/1.1 200 OK\r\n\
                                     Content-Type: text/event-stream\r\n\
                                     Content-Length: {}\r\n\
                                     \r\n\
                                     {}",
                                    frames.len(),
                                    frames,
                                )
                            }
                        };

                        if let Err(e) = writer.write_all(response.as_bytes()).await {
                            eprintln!("mock server: write failed: {e}");
                            break;
                        }
                        if let Err(e) = writer.flush().await {
                            eprintln!("mock server: flush failed: {e}");
                            break;
                        }
                    }
                });
            }
        });

        Self {
            addr,
            post_count,
            posts,
            handle,
        }
    }

    /// The `http://host:port` base URL this mock server is listening on.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The number of POST requests this server has handled so far.
    #[must_use]
    pub fn post_count(&self) -> usize {
        self.post_count.load(Ordering::SeqCst)
    }

    /// A snapshot of the POST bodies recorded so far.
    pub async fn recorded_posts(&self) -> Vec<RecordedPost> {
        self.posts.lock().await.clone()
    }
}

impl Drop for MockGeminiServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// ─── HTTP/JSON Helpers ───────────────────────────────────────────────────────

async fn parse_http_request_with_body<R: tokio::io::AsyncRead + Unpin>(
    buf_reader: &mut BufReader<R>,
) -> Option<(String, String)> {
    let mut request_line = String::new();
    if let Err(e) = buf_reader.read_line(&mut request_line).await {
        eprintln!("mock server: failed to read request line: {e}");
        return None;
    }
    let request_line = request_line.trim_end().to_string();
    if request_line.is_empty() {
        return None;
    }

    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        if let Err(e) = buf_reader.read_line(&mut line).await {
            eprintln!("mock server: failed to read header line: {e}");
            return None;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_lowercase();
        if let Some(val) = lower.strip_prefix("content-length:") {
            content_length = match val.trim().parse() {
                Ok(len) => len,
                Err(e) => {
                    eprintln!("mock server: invalid Content-Length header: {e}");
                    return None;
                }
            };
        }
    }

    let body = if content_length > 0 {
        let mut buf = vec![0u8; content_length];
        if let Err(e) = buf_reader.read_exact(&mut buf).await {
            eprintln!("mock server: failed to read body: {e}");
            return None;
        }
        String::from_utf8_lossy(&buf).to_string()
    } else {
        String::new()
    };

    Some((request_line, body))
}

fn json_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        _ => "Error",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        body.len(),
        body
    )
}

fn sse_response(json_body: &str) -> String {
    let sse_data = format!("data: {json_body}\n\n");
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        sse_data.len(),
        sse_data
    )
}

fn model_list_json() -> String {
    serde_json::json!({
        "models": [{
            "name": "models/gemini-2.0-flash",
            "displayName": "Gemini 2.0 Flash",
            "supportedGenerationMethods": [
                "generateContent", "streamGenerateContent", "countTokens"
            ],
            "inputTokenLimit": 1_048_576,
            "outputTokenLimit": 8192
        }]
    })
    .to_string()
}

fn function_call_json(name: &str, args: &serde_json::Value) -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"functionCall": {"name": name, "args": args}}],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 10, "candidatesTokenCount": 5, "totalTokenCount": 15
        }
    })
    .to_string()
}

fn text_response_json(text: &str) -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"text": text}],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 15, "candidatesTokenCount": 10, "totalTokenCount": 25
        }
    })
    .to_string()
}

/// A candidate with empty content — no text parts, no function calls.
///
/// The SDK backend rejects this because both text and tool calls are missing.
/// The SDK emits it as an error step with `status=ERROR` but keeps iterating
/// (it's recoverable, not fatal).
fn empty_candidate_json() -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 15, "candidatesTokenCount": 0, "totalTokenCount": 15
        }
    })
    .to_string()
}

/// An HTTP error response (503, 500, 429, etc.) with a JSON body.
fn http_error_response(status: u16, message: &str) -> String {
    let (reason, grpc_status) = match status {
        429 => ("Too Many Requests", "RESOURCE_EXHAUSTED"),
        500 => ("Internal Server Error", "INTERNAL"),
        503 => ("Service Unavailable", "UNAVAILABLE"),
        _ => ("Error", "UNKNOWN"),
    };
    let body = serde_json::json!({
        "error": {
            "code": status,
            "message": message,
            "status": grpc_status,
        }
    })
    .to_string();
    format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        body.len(),
        body
    )
}

/// Build an [`agy_bridge::config::AgentConfig`] pointed at a mock `base_url`
/// with a test API key, custom-tools-only capabilities, and an `AllowAll` policy.
#[must_use]
pub fn agent_config(base_url: &str, system: &str) -> agy_bridge::config::AgentConfig {
    agy_bridge::config::AgentConfig::builder()
        .system_instructions(system)
        .gemini(agy_bridge::config::GeminiConfig {
            api_key: Some("test-key".to_string()),
            base_url: Some(base_url.to_string()),
            models: agy_bridge::config::ModelConfig::default(),
        })
        .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
        .policies([PolicyRule::AllowAll])
        .retry_config(agy_bridge::config::RetryConfig {
            api_retry: Some(agy_bridge::config::ModelAPIRetryConfig {
                max_retries: Some(2),
                initial_sleep_duration_ms: Some(50),
                exponential_multiplier: Some(1.0),
                jitter_range: Some(0.0),
            }),
            model_output_retry: None,
        })
        .build()
}

/// A fresh multi-threaded Tokio runtime for driving a single test to completion.
///
/// # Panics
///
/// Panics if the runtime cannot be constructed (e.g. the OS refuses to spawn
/// the worker threads).
#[must_use]
pub fn multi_thread_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}
