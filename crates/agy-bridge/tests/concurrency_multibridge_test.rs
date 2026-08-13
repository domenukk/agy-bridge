//! OS-thread parallelism and multi-bridge concurrency tests.
//!
//! Uses mock TCP servers — no API key required.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

async fn parse_http_request<R: tokio::io::AsyncRead + Unpin>(
    buf_reader: &mut BufReader<R>,
) -> Option<String> {
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

    Some(request_line)
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

fn generate_content_json(tag: &str) -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"text": format!("mock:{tag}")}],
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

struct MockServer {
    addr: std::net::SocketAddr,
    post_count: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
    tag: String,
    count: Arc<AtomicUsize>,
    delay: Option<std::time::Duration>,
) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf_reader = BufReader::new(reader);

    loop {
        let Some(request_line) = parse_http_request(&mut buf_reader).await else {
            break;
        };

        let response = if request_line.starts_with("GET ") {
            json_response(200, &model_list_json())
        } else {
            count.fetch_add(1, Ordering::SeqCst);
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            sse_response(&generate_content_json(&tag))
        };

        if let Err(e) = writer.write_all(response.as_bytes()).await {
            eprintln!("[MOCK {tag}] write error: {e}");
            break;
        }
        if let Err(e) = writer.flush().await {
            eprintln!("[MOCK {tag}] flush error: {e}");
            break;
        }
    }
}

impl MockServer {
    async fn start(tag: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");

        let post_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&post_count);
        let tag = tag.to_string();

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(serve_connection(
                    stream,
                    tag.clone(),
                    Arc::clone(&count),
                    None,
                ));
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

impl Drop for MockServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn agent_config(base_url: &str, system: &str) -> agy_bridge::config::AgentConfig {
    agy_bridge::config::AgentConfig::builder()
        .system_instructions(system)
        .gemini(agy_bridge::config::GeminiConfig {
            api_key: Some("test-key".to_string()),
            base_url: Some(base_url.to_string()),
            models: agy_bridge::config::ModelConfig::default(),
        })
        .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
        .retry_config(agy_bridge::config::RetryConfig::no_retries())
        .build()
}

fn multi_thread_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("multi-thread tokio runtime")
}

/// The strongest end-to-end guard for concurrent, multi-config use: spawn
/// several **OS threads** (not just tokio tasks), each of which builds its own
/// [`AgyBridge`](agy_bridge::AgyBridge) with a *distinct* configuration and its own multi-threaded
/// tokio runtime, then drives multiple agents concurrently against its own mock
/// backend.
#[test]
fn os_threads_multiple_bridges_multiple_configs_concurrent() {
    const THREADS: usize = 4;
    const AGENTS_PER_BRIDGE: usize = 3;

    let handles: Vec<std::thread::JoinHandle<()>> = (0..THREADS)
        .map(|t| {
            std::thread::spawn(move || {
                let rt = multi_thread_rt();
                rt.block_on(async move {
                    let tag = format!("thread-{t}");
                    let server = MockServer::start(&tag).await;
                    let url = server.base_url();

                    let bridge = agy_bridge::AgyBridge::builder()
                        .inter_agent_delay(std::time::Duration::from_millis((t as u64) * 5))
                        .build()
                        .unwrap_or_else(|e| panic!("thread {t}: bridge build: {e}"));

                    let agents = futures::future::join_all((0..AGENTS_PER_BRIDGE).map(|a| {
                        let url = url.clone();
                        let bridge = &bridge;
                        async move {
                            bridge
                                .agent(agent_config(&url, &format!("t{t}-agent{a}")))
                                .await
                        }
                    }))
                    .await
                    .into_iter()
                    .enumerate()
                    .map(|(a, r)| r.unwrap_or_else(|e| panic!("thread {t} agent {a} create: {e}")))
                    .collect::<Vec<_>>();

                    assert_eq!(
                        bridge
                            .active_agent_count()
                            .await
                            .unwrap_or_else(|e| panic!("thread {t} count: {e}")),
                        AGENTS_PER_BRIDGE,
                        "thread {t}: bridge must see exactly its own agents"
                    );

                    let chats = futures::future::join_all(
                        agents.iter().map(|agent| agent.chat_text("ping")),
                    )
                    .await;
                    for (a, res) in chats.into_iter().enumerate() {
                        let text = res.unwrap_or_else(|e| panic!("thread {t} agent {a} chat: {e}"));
                        assert!(
                            text.contains(&format!("mock:{tag}")),
                            "thread {t} agent {a} got wrong tag: {text}"
                        );
                    }

                    futures::future::join_all(
                        agents
                            .into_iter()
                            .map(|a| async move { a.shutdown().await }),
                    )
                    .await
                    .into_iter()
                    .enumerate()
                    .for_each(|(a, r)| {
                        r.unwrap_or_else(|e| panic!("thread {t} agent {a} shutdown: {e}"));
                    });

                    assert_eq!(
                        server.post_count(),
                        AGENTS_PER_BRIDGE,
                        "thread {t}: expected {AGENTS_PER_BRIDGE} POSTs to its own backend"
                    );
                });
            })
        })
        .collect();

    for (t, h) in handles.into_iter().enumerate() {
        h.join().unwrap_or_else(|e| {
            panic!("thread {t} panicked: {e:?} — concurrent multi-bridge failure")
        });
    }
}
