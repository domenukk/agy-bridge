//! Concurrent multi-agent deadlock and safety test.
//!
//! Spins up a mock Gemini server and spawns multiple agents in concurrent tasks
//! that execute chat turns concurrently to verify there are no deadlocks.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

// ─── Mock Server ─────────────────────────────────────────────────────────────

struct ParsedHttpRequest {
    request_line: String,
}

async fn parse_http_request<R: tokio::io::AsyncRead + Unpin>(
    buf_reader: &mut BufReader<R>,
) -> Option<ParsedHttpRequest> {
    let mut request_line = String::new();
    // NOLINT: test mock server helper — malformed request means caller error, return None
    buf_reader.read_line(&mut request_line).await.ok()?;
    let request_line = request_line.trim_end().to_string();
    if request_line.is_empty() {
        return None;
    }

    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        // NOLINT: test mock server helper — malformed request means caller error, return None
        buf_reader.read_line(&mut line).await.ok()?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_lowercase();
        if let Some(val) = lower.strip_prefix("content-length:") {
            // NOLINT: test helper — invalid content-length defaults to zero (no body)
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    if content_length > 0 {
        let mut body_buf = vec![0u8; content_length];
        // NOLINT: test mock server helper — incomplete body means malformed request, return None
        buf_reader.read_exact(&mut body_buf).await.ok()?;
    }

    Some(ParsedHttpRequest { request_line })
}

fn json_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Error",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Connection: close\r\n\
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
         Connection: close\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        sse_data.len(),
        sse_data
    )
}

fn model_list_json() -> String {
    serde_json::json!({
        "models": [
            {
                "name": "models/gemini-3.6-flash",
                "displayName": "Gemini 3.6 Flash",
                "supportedGenerationMethods": [
                    "generateContent",
                    "streamGenerateContent",
                    "countTokens"
                ],
                "inputTokenLimit": 1_048_576,
                "outputTokenLimit": 8192
            },
            {
                "name": "models/gemini-3.5-flash",
                "displayName": "Gemini 3.5 Flash",
                "supportedGenerationMethods": [
                    "generateContent",
                    "streamGenerateContent",
                    "countTokens"
                ],
                "inputTokenLimit": 1_048_576,
                "outputTokenLimit": 8192
            },
            {
                "name": "models/gemini-2.0-flash",
                "displayName": "Gemini 2.0 Flash",
                "supportedGenerationMethods": [
                    "generateContent",
                    "streamGenerateContent",
                    "countTokens"
                ],
                "inputTokenLimit": 1_048_576,
                "outputTokenLimit": 8192
            }
        ]
    })
    .to_string()
}

fn generate_content_json() -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"text": "Mock concurrent response"}],
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

struct MockConcurrentServer {
    addr: std::net::SocketAddr,
    request_count: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl MockConcurrentServer {
    async fn start() -> Self {
        Self::start_internal(None).await
    }

    async fn start_with_barrier(barrier: Arc<tokio::sync::Barrier>) -> Self {
        Self::start_internal(Some(barrier)).await
    }

    async fn start_internal(barrier: Option<Arc<tokio::sync::Barrier>>) -> Self {
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
                let barrier = barrier.clone();
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
                        count.fetch_add(1, Ordering::SeqCst);
                        if let Some(ref b) = barrier {
                            b.wait().await;
                        }
                        sse_response(&generate_content_json())
                    };

                    if let Err(e) = writer.write_all(response.as_bytes()).await {
                        eprintln!("Mock server: failed to write response: {e}");
                    }
                    if let Err(e) = writer.flush().await {
                        eprintln!("Mock server: failed to flush response: {e}");
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

impl Drop for MockConcurrentServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn multiple_concurrent_agents_no_deadlock() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let server = MockConcurrentServer::start().await;
        let base_url = server.base_url();
        eprintln!("Mock concurrent server listening on {base_url}");

        let bridge = Arc::new(
            agy_bridge::AgyBridge::builder()
                .inter_agent_delay(std::time::Duration::ZERO)
                .build()
                .expect("AgyBridge"),
        );

        let num_agents = 5usize;
        let num_turns = 3usize;
        let mut tasks = Vec::new();

        for i in 0..num_agents {
            let base_url = base_url.clone();
            let bridge = Arc::clone(&bridge);
            tasks.push(tokio::spawn(async move {
                let gemini = agy_bridge::config::GeminiConfig {
                    api_key: Some(format!("key-{i}")),
                    base_url: Some(base_url),
                    models: agy_bridge::config::ModelConfig::default(),
                };

                let config = agy_bridge::config::AgentConfig::builder()
                    .system_instructions(format!("Agent {i}"))
                    .gemini(gemini)
                    .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                    .retry_config(agy_bridge::config::RetryConfig::no_retries())
                    .build();

                let agent = bridge.agent(config).await.expect("create agent");

                for turn in 0..num_turns {
                    // Small delay to stagger execution and mix threads
                    tokio::time::sleep(std::time::Duration::from_millis(10 * (i as u64))).await;
                    let result = agent.chat_text(format!("Ping {turn}")).await;
                    assert!(result.is_ok(), "Agent {i} turn {turn} failed: {result:?}");
                    assert_eq!(result.unwrap(), "Mock concurrent response");
                }

                agent.shutdown().await.expect("shutdown agent");
            }));
        }

        // Enforce a strict 10 second timeout for all concurrent execution to ensure NO deadlock
        let join_all = futures::future::join_all(tasks);
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_secs(10), join_all).await;

        assert!(
            // NOLINT: test assertion \u2014 checking that no deadlock timeout occurred
            timeout_result.is_ok(),
            "Test timed out! Possible deadlock in runtime."
        );

        let total_requests = server.count();
        assert_eq!(
            total_requests,
            num_agents * num_turns,
            "Expected exactly {} API requests, got {total_requests}",
            num_agents * num_turns
        );
    });
}

#[test]
fn test_simultaneous_turns_across_agents_barrier() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        // Barrier requires exactly 2 concurrent in-flight requests to trip
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let server = MockConcurrentServer::start_with_barrier(barrier).await;
        let base_url = server.base_url();

        let bridge = Arc::new(
            agy_bridge::AgyBridge::builder()
                .inter_agent_delay(std::time::Duration::ZERO)
                .build()
                .expect("AgyBridge"),
        );

        let make_agent = |i: usize| {
            let base_url = base_url.clone();
            let bridge = Arc::clone(&bridge);
            async move {
                let gemini = agy_bridge::config::GeminiConfig {
                    api_key: Some(format!("key-{i}")),
                    base_url: Some(base_url),
                    models: agy_bridge::config::ModelConfig::default(),
                };
                let config = agy_bridge::config::AgentConfig::builder()
                    .system_instructions(format!("Agent {i}"))
                    .gemini(gemini)
                    .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                    .retry_config(agy_bridge::config::RetryConfig::no_retries())
                    .build();
                bridge.agent(config).await.expect("create agent")
            }
        };

        let agent1 = make_agent(1).await;
        let agent2 = make_agent(2).await;

        // Execute turns simultaneously across both agents.
        // If the harness serialized requests, agent 1 would wait for server response,
        // but server is waiting for agent 2 to arrive at the barrier, causing a deadlock!
        let turn1 = agent1.chat_text("Simultaneous turn from Agent 1");
        let turn2 = agent2.chat_text("Simultaneous turn from Agent 2");

        let (res1, res2) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            futures::future::join(turn1, turn2),
        )
        .await
        .expect(
            "Deadlock or serialization detected: both turns were not in flight simultaneously!",
        );

        assert_eq!(res1.unwrap(), "Mock concurrent response");
        assert_eq!(res2.unwrap(), "Mock concurrent response");

        agent1.shutdown().await.expect("shutdown agent1");
        agent2.shutdown().await.expect("shutdown agent2");
    });
}
