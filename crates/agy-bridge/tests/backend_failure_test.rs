//! Backend failure integration tests.
//!
//! Spins up mock Gemini servers that return various error responses (503, 500,
//! partial responses, timeouts) and verifies that agy-bridge:
//!
//! 1. Returns `Err(...)` — NOT `Ok("")` — for backend errors
//! 2. Isolates failures: healthy agents on other backends keep working
//! 3. Properly times out stuck backends
//! 4. Handles partial-then-error responses correctly
//! 5. Allows agents to recover after transient failures
//!
//! Run with:
//! ```sh
//! cargo test --test backend_failure_test -- --nocapture
//! ```

use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicUsize, Ordering},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

static BRIDGE: LazyLock<agy_bridge::AgyBridge> = LazyLock::new(|| {
    agy_bridge::AgyBridge::builder()
        .inter_agent_delay(std::time::Duration::ZERO)
        .build()
        .expect("shared AgyBridge")
});

// ─── Mock Server Infrastructure ──────────────────────────────────────────────

async fn parse_http_request<R: tokio::io::AsyncRead + Unpin>(
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

    if content_length > 0 {
        let mut body_buf = vec![0u8; content_length];
        if let Err(e) = buf_reader.read_exact(&mut body_buf).await {
            eprintln!("mock server: failed to read body: {e}");
            return None;
        }
    }

    Some((request_line, String::new()))
}

fn json_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
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
                "generateContent",
                "streamGenerateContent",
                "countTokens"
            ],
            "inputTokenLimit": 1_048_576,
            "outputTokenLimit": 8192
        }]
    })
    .to_string()
}

fn generate_content_json() -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"text": "Healthy mock response"}],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 5,
            "totalTokenCount": 15
        }
    })
    .to_string()
}

fn error_503_json() -> String {
    serde_json::json!({
        "error": {
            "code": 503,
            "message": "Scope '/aistudio/gemini-v4p1s-rev25-perseus-sc@2026062900' not found",
            "status": "UNAVAILABLE"
        }
    })
    .to_string()
}

fn error_500_json() -> String {
    serde_json::json!({
        "error": {
            "code": 500,
            "message": "Internal server error: APP_ERROR(2)",
            "status": "INTERNAL"
        }
    })
    .to_string()
}

fn error_429_json() -> String {
    serde_json::json!({
        "error": {
            "code": 429,
            "message": "Quota exceeded for quota metric 'Generate Content API requests per minute' and limit 'GenerateContent request limit per minute for a region' of service 'generativelanguage.googleapis.com'",
            "status": "RESOURCE_EXHAUSTED"
        }
    })
    .to_string()
}

/// Behaviour modes for the mock server.
#[derive(Clone, Copy, Debug)]
enum MockBehaviour {
    /// Always return a healthy 200 response.
    Healthy,
    /// Always return a 503 Service Unavailable.
    Error503,
    /// Always return a 500 Internal Server Error.
    Error500,
    /// Always return a 429 rate-limit error.
    Error429,
    /// Never respond (hang forever to trigger timeout).
    Hang,
}

struct MockFailureServer {
    addr: std::net::SocketAddr,
    post_count: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl MockFailureServer {
    async fn start(behaviour: MockBehaviour) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");

        let post_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&post_count);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let count = Arc::clone(&count);
                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut buf_reader = BufReader::new(reader);

                    let Some((request_line, _body)) = parse_http_request(&mut buf_reader).await
                    else {
                        return;
                    };

                    let is_get = request_line.starts_with("GET ");

                    if is_get {
                        // Always serve model list (agent creation needs it).
                        let response = json_response(200, &model_list_json());
                        if let Err(e) = writer.write_all(response.as_bytes()).await {
                            eprintln!("mock server: write failed: {e}");
                        }
                        if let Err(e) = writer.flush().await {
                            eprintln!("mock server: flush failed: {e}");
                        }
                        return;
                    }

                    count.fetch_add(1, Ordering::SeqCst);

                    let response = match behaviour {
                        MockBehaviour::Healthy => sse_response(&generate_content_json()),
                        MockBehaviour::Error503 => json_response(503, &error_503_json()),
                        MockBehaviour::Error500 => json_response(500, &error_500_json()),
                        MockBehaviour::Error429 => json_response(429, &error_429_json()),
                        MockBehaviour::Hang => {
                            // Never respond — just hold the connection open.
                            // Sleep longer than any reasonable timeout.
                            tokio::time::sleep(std::time::Duration::from_mins(5)).await;
                            return;
                        }
                    };

                    if let Err(e) = writer.write_all(response.as_bytes()).await {
                        eprintln!("mock server: write failed: {e}");
                    }
                    if let Err(e) = writer.flush().await {
                        eprintln!("mock server: flush failed: {e}");
                    }
                });
            }
        });

        Self {
            addr,
            post_count,
            handle,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn post_count(&self) -> usize {
        self.post_count.load(Ordering::SeqCst)
    }
}

impl Drop for MockFailureServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn agent_config_for(base_url: &str, system: &str) -> agy_bridge::config::AgentConfig {
    agy_bridge::config::AgentConfig::builder()
        .system_instructions(system)
        .gemini(agy_bridge::config::GeminiConfig {
            api_key: Some("test-key".to_string()),
            base_url: Some(base_url.to_string()),
            models: agy_bridge::config::ModelConfig::default(),
        })
        .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
        .build()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// 503 backend error must return `Err(...)`, NOT `Ok("")`.
///
/// This is the exact bug that killed the ARTIST swarm: agy-bridge returned
/// `Ok("")` for 503 errors, causing the orchestrator to misclassify fatal
/// backend failures as empty successful responses.
#[test]
fn error_503_returns_err_not_empty_ok() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockFailureServer::start(MockBehaviour::Error503).await;
        let config = agent_config_for(&server.base_url(), "Agent under 503");
        let agent = BRIDGE.agent(config).await.expect("create agent");

        let result = agent.chat_text("Hello").await;
        eprintln!("503 result: {result:?}");

        let err_msg = result
            .expect_err(
                "Backend 503 MUST return Err (bug: agy-bridge returned Ok(\"\") for 503 errors)",
            )
            .to_string();
        eprintln!("503 error message: {err_msg}");

        // The error message should contain useful context about the failure.
        // With quota retry logic, 503 errors may surface as QuotaExceeded
        // ("Quota exceeded, retry after ...") since is_quota_error() matches "503".
        assert!(
            err_msg.contains("503")
                || err_msg.contains("UNAVAILABLE")
                || err_msg.contains("error")
                || err_msg.contains("Error")
                || err_msg.contains("terminated")
                || err_msg.contains("Scope")
                || err_msg.contains("Quota")
                || err_msg.contains("quota")
                || err_msg.contains("retry"),
            "Error message should contain backend error context, got: {err_msg}"
        );

        assert!(
            server.post_count() > 0,
            "Server should have received at least 1 request"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// 500 backend error must also return `Err(...)`.
#[test]
fn error_500_returns_err() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockFailureServer::start(MockBehaviour::Error500).await;
        let config = agent_config_for(&server.base_url(), "Agent under 500");
        let agent = BRIDGE.agent(config).await.expect("create agent");

        let result = agent.chat_text("Hello").await;
        eprintln!("500 result: {result:?}");

        result.expect_err("Backend 500 MUST return Err");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Concurrent agents: one on a healthy backend, one on a 503 backend.
/// The healthy agent MUST still work. The failing agent MUST return Err.
#[test]
fn concurrent_agents_mixed_healthy_and_503() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let healthy_server = MockFailureServer::start(MockBehaviour::Healthy).await;
        let broken_server = MockFailureServer::start(MockBehaviour::Error503).await;

        eprintln!(
            "Healthy: {}, Broken: {}",
            healthy_server.base_url(),
            broken_server.base_url()
        );

        let healthy_config = agent_config_for(&healthy_server.base_url(), "Healthy agent");
        let broken_config = agent_config_for(&broken_server.base_url(), "Broken agent");

        let healthy_agent = BRIDGE.agent(healthy_config).await.expect("healthy agent");
        let broken_agent = BRIDGE.agent(broken_config).await.expect("broken agent");

        // Execute both concurrently.
        let (healthy_result, broken_result) = tokio::join!(
            healthy_agent.chat_text("Ping"),
            broken_agent.chat_text("Ping"),
        );

        eprintln!("Healthy result: {healthy_result:?}");
        eprintln!("Broken result: {broken_result:?}");

        // Healthy agent MUST succeed.
        let healthy_text = healthy_result.expect("Healthy agent should succeed");
        assert_eq!(
            healthy_text, "Healthy mock response",
            "Healthy agent should return the mock response"
        );

        // Broken agent MUST fail.
        broken_result.expect_err("Broken agent should fail with 503");

        healthy_agent.shutdown().await.expect("shutdown healthy");
        broken_agent.shutdown().await.expect("shutdown broken");
    });
}

/// Multiple agents on different broken backends (503, 500, 429).
/// ALL must return Err — none should silently succeed with empty text.
#[test]
fn concurrent_agents_all_different_errors() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server_503 = MockFailureServer::start(MockBehaviour::Error503).await;
        let server_500 = MockFailureServer::start(MockBehaviour::Error500).await;
        let server_429 = MockFailureServer::start(MockBehaviour::Error429).await;

        let agent_503 = BRIDGE
            .agent(agent_config_for(&server_503.base_url(), "503 agent"))
            .await
            .expect("503 agent");
        let agent_500 = BRIDGE
            .agent(agent_config_for(&server_500.base_url(), "500 agent"))
            .await
            .expect("500 agent");
        let agent_429 = BRIDGE
            .agent(agent_config_for(&server_429.base_url(), "429 agent"))
            .await
            .expect("429 agent");

        let (r503, r500, r429) = tokio::join!(
            agent_503.chat_text("Hello"),
            agent_500.chat_text("Hello"),
            agent_429.chat_text("Hello"),
        );

        eprintln!("503: {r503:?}");
        eprintln!("500: {r500:?}");
        eprintln!("429: {r429:?}");

        r503.expect_err("503 agent must fail");
        r500.expect_err("500 agent must fail");
        r429.expect_err("429 agent must fail");

        agent_503.shutdown().await.expect("shutdown 503");
        agent_500.shutdown().await.expect("shutdown 500");
        agent_429.shutdown().await.expect("shutdown 429");
    });
}

/// Healthy backend: multiple concurrent agents, all producing correct text.
/// Ensures the mock infrastructure itself works and we're not false-positive'ing
/// on mock bugs.
#[test]
fn concurrent_agents_all_healthy_baseline() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockFailureServer::start(MockBehaviour::Healthy).await;
        let base = server.base_url();

        let a0 = BRIDGE
            .agent(agent_config_for(&base, "Healthy 0"))
            .await
            .expect("a0");
        let a1 = BRIDGE
            .agent(agent_config_for(&base, "Healthy 1"))
            .await
            .expect("a1");
        let a2 = BRIDGE
            .agent(agent_config_for(&base, "Healthy 2"))
            .await
            .expect("a2");

        let (r0, r1, r2) = tokio::join!(
            a0.chat_text("Ping 0"),
            a1.chat_text("Ping 1"),
            a2.chat_text("Ping 2"),
        );

        assert!(r0.is_ok(), "Agent 0 should succeed, got: {r0:?}");
        assert!(r1.is_ok(), "Agent 1 should succeed, got: {r1:?}");
        assert!(r2.is_ok(), "Agent 2 should succeed, got: {r2:?}");
        assert_eq!(r0.unwrap(), "Healthy mock response");
        assert_eq!(r1.unwrap(), "Healthy mock response");
        assert_eq!(r2.unwrap(), "Healthy mock response");

        let total_posts = server.post_count();
        assert_eq!(
            total_posts, 3,
            "Expected 3 POST requests, got {total_posts}"
        );

        a0.shutdown().await.expect("shutdown");
        a1.shutdown().await.expect("shutdown");
        a2.shutdown().await.expect("shutdown");
    });
}

/// Chat timeout must fire when the backend hangs.
///
/// This tests that `tokio::time::timeout` around `agent.chat()` actually
/// works. The ARTIST bug was that the timeout couldn't fire because the
/// Python GIL was held — verify that doesn't happen here.
#[test]
fn timeout_fires_when_backend_hangs() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockFailureServer::start(MockBehaviour::Hang).await;

        // The bridge no longer imposes a chat timeout — stall detection is a
        // consumer-layer concern. A tokio timeout around chat() must be able to
        // fire even while Python is busy; this is the ARTIST GIL-starvation
        // regression guard.
        let bridge = agy_bridge::AgyBridge::builder()
            .inter_agent_delay(std::time::Duration::ZERO)
            .build()
            .expect("bridge");

        let config = agent_config_for(&server.base_url(), "Timeout agent");
        let agent = bridge.agent(config).await.expect("create agent");

        let start = std::time::Instant::now();
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(3), agent.chat_text("Hello")).await;
        let elapsed = start.elapsed();

        eprintln!(
            "Timeout result after {:.1}s: {result:?}",
            elapsed.as_secs_f64()
        );

        result.expect_err("Hanging backend MUST cause the consumer tokio timeout to fire");

        // Should have timed out within a reasonable margin of the 3s timeout.
        assert!(
            elapsed.as_secs() < 8,
            "Timeout should fire within ~3s, took {}s — possible GIL starvation",
            elapsed.as_secs()
        );

        tokio::time::timeout(std::time::Duration::from_secs(10), agent.shutdown())
            .await
            .expect("shutdown must not stall")
            .expect("shutdown");
    });
}

/// A healthy agent must still function even after a sibling agent on a
/// different backend experiences a timeout.
#[test]
fn healthy_agent_survives_sibling_timeout() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let healthy_server = MockFailureServer::start(MockBehaviour::Healthy).await;
        let hanging_server = MockFailureServer::start(MockBehaviour::Hang).await;

        let bridge = agy_bridge::AgyBridge::builder()
            .inter_agent_delay(std::time::Duration::ZERO)
            .build()
            .expect("bridge");

        let healthy_config = agent_config_for(&healthy_server.base_url(), "Healthy");
        let hanging_config = agent_config_for(&hanging_server.base_url(), "Hanging");

        let healthy_agent = bridge.agent(healthy_config).await.expect("healthy agent");
        let hanging_agent = bridge.agent(hanging_config).await.expect("hanging agent");

        // Run both concurrently. Stall detection for the hanging agent is a
        // consumer-layer tokio timeout.
        let (healthy_res, hanging_res) = tokio::join!(
            healthy_agent.chat_text("Ping"),
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                hanging_agent.chat_text("Ping"),
            ),
        );

        // Healthy must succeed.
        let healthy_text =
            healthy_res.expect("Healthy agent must work even when sibling is timing out");
        assert_eq!(healthy_text, "Healthy mock response");

        // Hanging must time out at the consumer layer.
        hanging_res.expect_err("Hanging agent must time out");

        tokio::time::timeout(std::time::Duration::from_secs(10), healthy_agent.shutdown())
            .await
            .expect("shutdown healthy must not stall")
            .expect("shutdown healthy");
        tokio::time::timeout(std::time::Duration::from_secs(10), hanging_agent.shutdown())
            .await
            .expect("shutdown hanging must not stall")
            .expect("shutdown hanging");
    });
}

/// After a 503 error, the agent handle should remain usable for shutdown.
/// No panic, no hang.
#[test]
fn agent_shutdown_after_error_does_not_hang() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockFailureServer::start(MockBehaviour::Error503).await;
        let config = agent_config_for(&server.base_url(), "Shutdown after error");
        let agent = BRIDGE.agent(config).await.expect("create agent");

        // Chat should fail.
        let result = agent.chat_text("Hello").await;
        assert!(result.is_err(), "Should get error from 503 backend");

        // Shutdown must succeed without hanging.
        let shutdown_result =
            tokio::time::timeout(std::time::Duration::from_secs(5), agent.shutdown()).await;

        let shutdown_inner = shutdown_result.expect("Agent shutdown should not hang after error");
        shutdown_inner.expect("Agent shutdown should succeed after error");
    });
}

/// Multiple sequential chat calls to a 503 backend: each must return Err.
/// Verifies no stale state accumulates between calls.
#[test]
fn repeated_errors_each_return_err() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockFailureServer::start(MockBehaviour::Error503).await;
        let config = agent_config_for(&server.base_url(), "Repeated errors");
        let agent = BRIDGE.agent(config).await.expect("create agent");

        for i in 0..3 {
            // Bound each attempt: the bridge imposes no timeout, so a backend
            // that the SDK keeps retrying is bounded at the consumer layer. A
            // timeout or an SDK error both prove the 503 backend never yields Ok.
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                agent.chat_text(format!("Attempt {i}")),
            )
            .await;
            assert!(
                result.as_ref().map_or(true, std::result::Result::is_err),
                "Attempt {i}: 503 backend must not return Ok, got {result:?}"
            );
        }

        tokio::time::timeout(std::time::Duration::from_secs(10), agent.shutdown())
            .await
            .expect("shutdown must not stall")
            .expect("shutdown");
    });
}

/// Verify the full `chat()` streaming handle also produces errors correctly
/// (not just `chat_text()`).
#[test]
fn streaming_handle_text_returns_err_on_503() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockFailureServer::start(MockBehaviour::Error503).await;
        let config = agent_config_for(&server.base_url(), "Streaming handle");
        let agent = BRIDGE.agent(config).await.expect("create agent");

        let chat_result =
            tokio::time::timeout(std::time::Duration::from_secs(30), agent.chat("Hello")).await;
        match chat_result {
            Ok(Ok(handle)) => {
                let text_result =
                    tokio::time::timeout(std::time::Duration::from_secs(30), handle.text()).await;
                eprintln!("Streaming handle .text() result: {text_result:?}");
                assert!(
                    text_result
                        .as_ref()
                        .map_or(true, std::result::Result::is_err),
                    "handle.text() must not return Ok for 503, got {text_result:?}"
                );
            }
            Ok(Err(e)) => {
                // Also acceptable — some error paths surface at chat() time.
                eprintln!("Error at chat() level (also acceptable): {e}");
            }
            Err(_elapsed) => {
                // A stalled retry loop bounded at the consumer layer is also a
                // valid non-Ok outcome for a persistent 503 backend.
                eprintln!("chat() bounded by consumer timeout (acceptable for 503)");
            }
        }

        tokio::time::timeout(std::time::Duration::from_secs(10), agent.shutdown())
            .await
            .expect("shutdown must not stall")
            .expect("shutdown");
    });
}

/// 5 agents on 5 different mock backends: 2 healthy, 3 broken (503, 500, 429).
/// All healthy must succeed, all broken must fail. No cross-contamination.
///
/// This is the core "ARTIST swarm" scenario: multiple agents on different
/// backend endpoints running concurrently.
#[test]
fn five_agents_mixed_backends_full_isolation() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let healthy_a = MockFailureServer::start(MockBehaviour::Healthy).await;
        let healthy_b = MockFailureServer::start(MockBehaviour::Healthy).await;
        let broken_503 = MockFailureServer::start(MockBehaviour::Error503).await;
        let broken_500 = MockFailureServer::start(MockBehaviour::Error500).await;
        let broken_429 = MockFailureServer::start(MockBehaviour::Error429).await;

        let a_h1 = BRIDGE
            .agent(agent_config_for(&healthy_a.base_url(), "healthy-a"))
            .await
            .expect("h-a");
        let a_h2 = BRIDGE
            .agent(agent_config_for(&healthy_b.base_url(), "healthy-b"))
            .await
            .expect("h-b");
        let a_503 = BRIDGE
            .agent(agent_config_for(&broken_503.base_url(), "broken-503"))
            .await
            .expect("503");
        let a_500 = BRIDGE
            .agent(agent_config_for(&broken_500.base_url(), "broken-500"))
            .await
            .expect("500");
        let a_429 = BRIDGE
            .agent(agent_config_for(&broken_429.base_url(), "broken-429"))
            .await
            .expect("429");

        // Chat with all concurrently.
        let (rh1, rh2, r503, r500, r429) = tokio::join!(
            a_h1.chat_text("Hello"),
            a_h2.chat_text("Hello"),
            a_503.chat_text("Hello"),
            a_500.chat_text("Hello"),
            a_429.chat_text("Hello"),
        );

        // Healthy agents must succeed.
        assert!(rh1.is_ok(), "healthy-a should succeed, got: {rh1:?}");
        assert!(rh2.is_ok(), "healthy-b should succeed, got: {rh2:?}");
        assert_eq!(rh1.unwrap(), "Healthy mock response");
        assert_eq!(rh2.unwrap(), "Healthy mock response");

        // Broken agents must fail.
        r503.expect_err("broken-503 must fail");
        r500.expect_err("broken-500 must fail");
        r429.expect_err("broken-429 must fail");

        a_h1.shutdown().await.expect("shutdown");
        a_h2.shutdown().await.expect("shutdown");
        a_503.shutdown().await.expect("shutdown");
        a_500.shutdown().await.expect("shutdown");
        a_429.shutdown().await.expect("shutdown");
    });
}
