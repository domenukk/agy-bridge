//! Integration tests verifying that quota backoff and retries (HTTP 429) work correctly.
//!
//! These tests spin up a lightweight TCP mock server that returns HTTP 429 on
//! the first request, and HTTP 200 SSE stream on the second request.

use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicUsize, Ordering},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

// ─── Shared Bridge ───────────────────────────────────────────────────────────

static BRIDGE: LazyLock<agy_bridge::AgyBridge> = LazyLock::new(|| {
    agy_bridge::AgyBridge::builder()
        .build()
        .expect("shared AgyBridge")
});

// ─── Mock Server ─────────────────────────────────────────────────────────────

struct ParsedHttpRequest {
    request_line: String,
}

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

    if content_length > 0 {
        let mut body_buf = vec![0u8; content_length];
        buf_reader.read_exact(&mut body_buf).await.ok()?;
    }

    Some(ParsedHttpRequest { request_line })
}

fn json_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        429 => "Too Many Requests",
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

struct MockQuotaServer {
    addr: std::net::SocketAddr,
    request_count: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl MockQuotaServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");

        let request_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&request_count);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let count = Arc::clone(&count);
                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut buf_reader = BufReader::new(reader);

                    let Some(parsed) = parse_http_request(&mut buf_reader).await else {
                        return;
                    };

                    let is_get = parsed.request_line.starts_with("GET ");
                    let response = if is_get {
                        json_response(200, &model_list_json())
                    } else {
                        let current_attempt = count.fetch_add(1, Ordering::SeqCst);
                        if current_attempt == 0 {
                            eprintln!(
                                "[MOCK SERVER] Returning 429 for request {}",
                                parsed.request_line
                            );
                            json_response(
                                429,
                                r#"{"error":{"code":429,"message":"Quota exceeded"}}"#,
                            )
                        } else {
                            eprintln!(
                                "[MOCK SERVER] Returning 200 for request {}",
                                parsed.request_line
                            );
                            sse_response(&generate_content_json())
                        }
                    };

                    if let Err(e) = writer.write_all(response.as_bytes()).await {
                        eprintln!("[MOCK] write error: {e}");
                    }
                    if let Err(e) = writer.flush().await {
                        eprintln!("[MOCK] flush error: {e}");
                    }
                });
            }
        });

        Self {
            addr,
            request_count,
            handle,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn count(&self) -> usize {
        self.request_count.load(Ordering::SeqCst)
    }
}

impl Drop for MockQuotaServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn agent_retries_on_quota_errors() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockQuotaServer::start().await;
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
            .max_quota_retries(3u32) // Ensure retries are configured
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .build();

        let agent = BRIDGE.agent(config).await.expect("create agent");

        let result = agent.chat_text("PING").await;
        eprintln!("Chat result: {result:?}");

        assert!(
            result.is_ok(),
            "Expected chat to succeed after retry, got: {result:?}"
        );
        assert_eq!(result.unwrap(), "Mock response");

        let count = server.count();
        assert_eq!(
            count, 2,
            "Expected exactly 2 generateContent requests to mock server, got {count}"
        );
    });
}
