//! Integration tests verifying that `base_url` routing works correctly.
//!
//! These tests spin up a lightweight TCP mock server that records incoming
//! HTTP requests, then create agy-bridge agents pointing at that mock.
//! They verify:
//!
//! 1. When `base_url` is set in `GeminiConfig`, requests are sent
//!    to that URL instead of the default Gemini endpoint.
//! 2. Multiple agents in the same process can use different `base_url`
//!    values independently (no cross-contamination).
//!
//! No real API key or Gemini backend is needed — the mock server returns
//! a minimal valid Gemini API response.
//!
//! Run with:
//! ```sh
//! cargo test --test base_url_routing_test -- --nocapture
//! ```

use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicUsize, Ordering},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::Mutex,
};

// ─── Shared Bridge ───────────────────────────────────────────────────────────

/// All tests share a single `AgyBridge` instance because the Python runtime
/// can only be meaningfully initialized once per process (due to the GIL and
/// the SDK's global WebSocket connection state). This `LazyLock` ensures the
/// bridge is created once and reused across all tests.
static BRIDGE: LazyLock<agy_bridge::AgyBridge> = LazyLock::new(|| {
    agy_bridge::AgyBridge::builder()
        .build()
        .expect("shared AgyBridge")
});

// ─── Mock Server ─────────────────────────────────────────────────────────────

/// A recorded HTTP request from the mock server.
#[derive(Debug, Clone)]
struct RecordedRequest {
    /// The HTTP request line.
    request_line: String,
    /// The request body.
    body: String,
}

/// Parsed HTTP request from a TCP stream.
struct ParsedHttpRequest {
    request_line: String,
    body: String,
    content_length: usize,
}

/// Parse a complete HTTP request from a buffered reader.
///
/// Returns `None` if the connection was closed before a full request was read.
async fn parse_http_request<R: tokio::io::AsyncRead + Unpin>(
    buf_reader: &mut BufReader<R>,
) -> Option<ParsedHttpRequest> {
    let mut request_line = String::new();
    buf_reader.read_line(&mut request_line).await.ok()?;
    let request_line = request_line.trim_end().to_string();
    if request_line.is_empty() {
        return None;
    }

    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        buf_reader.read_line(&mut line).await.ok()?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_lowercase();
        if let Some(val) = lower.strip_prefix("content-length:") {
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    let body = if content_length > 0 {
        let mut body_buf = vec![0u8; content_length];
        buf_reader.read_exact(&mut body_buf).await.ok()?;
        String::from_utf8_lossy(&body_buf).to_string()
    } else {
        String::new()
    };

    Some(ParsedHttpRequest {
        request_line,
        body,
        content_length,
    })
}

/// Build an HTTP response based on the request line.
///
/// Routes:
/// - `GET */models*` → model listing JSON
/// - `POST *streamGenerateContent*` → SSE response
/// - `POST *generateContent*` → JSON response
/// - Everything else → 404
fn build_response(request_line: &str) -> String {
    let is_get = request_line.starts_with("GET ");
    let is_stream = request_line.contains("streamGenerateContent");
    let is_generate = request_line.contains("generateContent");
    let is_models = request_line.contains("/models");

    if is_get && is_models {
        json_response(200, &model_list_json())
    } else if is_stream {
        sse_response(&generate_content_json())
    } else if is_generate {
        json_response(200, &generate_content_json())
    } else {
        json_response(404, r#"{"error":{"code":404,"message":"Not found"}}"#)
    }
}

/// Format an HTTP response with a JSON body.
fn json_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
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

/// Format a Server-Sent Events HTTP response wrapping a JSON payload.
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

/// Minimal model listing JSON.
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
        }, {
            "name": "models/gemini-3.5-flash",
            "displayName": "Gemini 3.5 Flash",
            "supportedGenerationMethods": [
                "generateContent",
                "streamGenerateContent",
                "countTokens"
            ],
            "inputTokenLimit": 1_048_576,
            "outputTokenLimit": 65_536
        }]
    })
    .to_string()
}

/// Minimal valid `GenerateContentResponse` JSON.
fn generate_content_json() -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"text": "Mock response"}],
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

/// A mock HTTP server that emulates Gemini API endpoints.
///
/// Handles model listing (GET) and generate content (POST).
/// Records all POST requests for test assertions.
struct MockGeminiServer {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    request_count: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl MockGeminiServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");

        let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let request_count = Arc::new(AtomicUsize::new(0));

        let reqs = Arc::clone(&requests);
        let count = Arc::clone(&request_count);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let reqs = Arc::clone(&reqs);
                let count = Arc::clone(&count);
                tokio::spawn(Self::handle_connection(stream, reqs, count));
            }
        });

        Self {
            addr,
            requests,
            request_count,
            handle,
        }
    }

    /// Handle a single TCP connection: parse request, record it, send response.
    async fn handle_connection(
        stream: tokio::net::TcpStream,
        reqs: Arc<Mutex<Vec<RecordedRequest>>>,
        count: Arc<AtomicUsize>,
    ) {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut buf_reader = BufReader::new(reader);

        let Some(parsed) = parse_http_request(&mut buf_reader).await else {
            return;
        };

        let is_get = parsed.request_line.starts_with("GET ");

        if is_get {
            eprintln!("[MOCK] {}", parsed.request_line);
        } else {
            reqs.lock().await.push(RecordedRequest {
                request_line: parsed.request_line.clone(),
                body: parsed.body.clone(),
            });
            count.fetch_add(1, Ordering::SeqCst);
            eprintln!(
                "[MOCK] {} (body: {} bytes)",
                parsed.request_line, parsed.content_length,
            );
        }

        let response = build_response(&parsed.request_line);
        if let Err(e) = writer.write_all(response.as_bytes()).await {
            eprintln!("[MOCK] write error: {e}");
        }
        if let Err(e) = writer.flush().await {
            eprintln!("[MOCK] flush error: {e}");
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn count(&self) -> usize {
        self.request_count.load(Ordering::SeqCst)
    }

    async fn recorded(&self) -> Vec<RecordedRequest> {
        self.requests.lock().await.clone()
    }
}

impl Drop for MockGeminiServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// Verify that setting `base_url` in `GeminiConfig` causes requests to be
/// sent to that URL instead of the default endpoint.
#[test]
fn base_url_routes_requests_to_custom_endpoint() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockGeminiServer::start().await;
        let base_url = server.base_url();
        eprintln!("Mock server listening on {base_url}");

        let gemini = agy_bridge::config::GeminiConfig {
            api_key: Some("test-key-for-mock".to_string()),
            base_url: Some(base_url.clone()),
            models: agy_bridge::config::ModelConfig::default(),
        };

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("Reply with exactly: PONG")
            .gemini(gemini)
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .build();

        let agent = BRIDGE.agent(config).await.expect("create agent");

        let result = agent.chat_text("PING").await;
        eprintln!("Chat result: {result:?}");

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let count = server.count();
        let recorded = server.recorded().await;
        eprintln!("Mock received {count} POST request(s)");

        assert!(
            count > 0,
            "Expected at least 1 POST request to mock at {base_url}, got 0. \
             Requests are not being routed to the custom base_url."
        );

        let has_generate = recorded.iter().any(|r| {
            r.request_line.contains("generateContent")
                || r.request_line.contains("streamGenerateContent")
        });
        assert!(
            has_generate,
            "Expected a generateContent request, got: {:?}",
            recorded.iter().map(|r| &r.request_line).collect::<Vec<_>>()
        );

        let has_ping = recorded.iter().any(|r| r.body.contains("PING"));
        assert!(
            has_ping,
            "Expected 'PING' in request body, got bodies: {:?}",
            recorded.iter().map(|r| &r.body).collect::<Vec<_>>()
        );

        drop(agent);
    });
}

/// Verify that two agents in the same process can use different `base_url`
/// values without cross-contamination.
///
/// Uses the shared `BRIDGE` — just like production code, where a single
/// bridge spawns multiple agents.
#[test]
fn multiple_agents_use_independent_base_urls() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server_a = MockGeminiServer::start().await;
        let server_b = MockGeminiServer::start().await;

        let url_a = server_a.base_url();
        let url_b = server_b.base_url();
        eprintln!("Server A: {url_a}");
        eprintln!("Server B: {url_b}");

        let config_a = agy_bridge::config::AgentConfig::builder()
            .system_instructions("Agent A")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("key-a".to_string()),
                base_url: Some(url_a.clone()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .build();

        let config_b = agy_bridge::config::AgentConfig::builder()
            .system_instructions("Agent B")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("key-b".to_string()),
                base_url: Some(url_b.clone()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .build();

        let agent_a = BRIDGE.agent(config_a).await.expect("create agent A");
        let agent_b = BRIDGE.agent(config_b).await.expect("create agent B");

        if let Err(e) = agent_a.chat_text("Hello from A").await {
            eprintln!("Agent A chat error (expected with mock): {e}");
        }
        if let Err(e) = agent_b.chat_text("Hello from B").await {
            eprintln!("Agent B chat error (expected with mock): {e}");
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let count_a = server_a.count();
        let count_b = server_b.count();
        eprintln!("Server A received {count_a} POST(s), Server B received {count_b} POST(s)");

        let recorded_a = server_a.recorded().await;
        let recorded_b = server_b.recorded().await;

        assert!(
            count_a > 0,
            "Server A should have received Agent A's requests, got 0"
        );
        assert!(
            count_b > 0,
            "Server B should have received Agent B's requests, got 0"
        );

        let a_has_a = recorded_a.iter().any(|r| r.body.contains("Hello from A"));
        let b_has_b = recorded_b.iter().any(|r| r.body.contains("Hello from B"));

        assert!(a_has_a, "Server A should contain 'Hello from A'");
        assert!(b_has_b, "Server B should contain 'Hello from B'");

        drop(agent_a);
        drop(agent_b);
    });
}
