//! Concurrency, teardown, leak detection, and GIL-safety tests.
//!
//! These tests use mock TCP servers — no API key required. They verify:
//!
//! 1. Agents can be created concurrently without GIL deadlock.
//! 2. Ongoing conversations keep working during/after a peer's teardown.
//! 3. New agents can be created *during* another agent's `shutdown()`.
//! 4. Python objects don't leak — the bridge state registry is clean after shutdown.
//! 5. Rapid create → chat → shutdown cycles don't corrupt state.
//! 6. Multiple bridges with concurrent agents work in isolation.
//!
//! Run with:
//! ```sh
//! cargo test --test concurrency_test -- --nocapture
//! ```

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
        .inter_agent_delay(std::time::Duration::ZERO)
        .build()
        .expect("shared AgyBridge")
});

// ─── Mock Server Infrastructure ──────────────────────────────────────────────

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

/// Serve HTTP/1.1 requests on a single connection until the client closes it
/// (keep-alive). `GET` returns the model list; `POST` returns an SSE completion
/// tagged with `tag` (after an optional `delay`) and increments `count`.
///
/// Serving multiple requests per connection avoids the connection churn and
/// ephemeral-port pressure that otherwise makes highly concurrent test runs
/// flaky.
async fn serve_connection(
    stream: tokio::net::TcpStream,
    tag: String,
    count: Arc<AtomicUsize>,
    delay: Option<std::time::Duration>,
) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf_reader = BufReader::new(reader);

    loop {
        // `None` means the client closed the connection (or sent a malformed
        // request) — nothing more to serve.
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
    /// Start a mock Gemini API server that tags each response with `tag`.
    async fn start(tag: &str) -> Self {
        Self::start_with_delay(tag, None).await
    }

    /// Start a mock server that delays each POST response by `delay`.
    async fn start_slow(tag: &str, delay: std::time::Duration) -> Self {
        Self::start_with_delay(tag, Some(delay)).await
    }

    async fn start_with_delay(tag: &str, delay: Option<std::time::Duration>) -> Self {
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
                    delay,
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

/// Poll `bridge.active_agent_count()` until it reaches zero or the timeout
/// elapses. Returns `true` if the registry drained to zero.
///
/// Used to deterministically verify that dropping a handle (which fires a
/// best-effort *asynchronous* shutdown) cleans up — without resorting to a
/// fixed-duration sleep.
async fn wait_for_zero_agents(bridge: &agy_bridge::AgyBridge) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match bridge.active_agent_count().await {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) => panic!("active_agent_count query failed: {e}"),
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Force the one-time, process-wide Python SDK import by creating and discarding
/// a throwaway agent.
///
/// Cold-importing `google.antigravity.*` and the compiled `pydantic` core takes
/// several seconds. The GIL-lockup *canary* tests below measure per-agent
/// startup latency to detect serialization/deadlock; without warming up first
/// they would also capture this one-time import, which is load-sensitive and
/// would make the canaries flaky on a busy machine. Warming up isolates the
/// signal they care about (per-create GIL contention), while a true deadlock is
/// still caught by the surrounding 30s timeout.
async fn warm_up_python_sdk(base_url: &str) {
    let warmup = BRIDGE
        .agent(agent_config(base_url, "warmup"))
        .await
        .expect("warm-up agent creation");
    warmup.shutdown().await.expect("warm-up agent shutdown");
}

// ─── Tests ───────────────────────────────────────────────────────────────────

// =============================================================================
// 1. Concurrent agent startup — no GIL lockup
// =============================================================================

/// Create 5 agents concurrently via `tokio::join!`. All must succeed without
/// GIL deadlock or "event loop is closed" errors. A 15-second overall timeout
/// ensures we detect GIL starvation.
#[test]
fn concurrent_agent_startup_no_gil_lockup() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let server = MockServer::start("startup").await;
        let url = server.base_url();

        // Warm up the one-time cold SDK import so we measure concurrent startup
        // latency, not the first import (see `warm_up_python_sdk`).
        warm_up_python_sdk(&url).await;

        let start = std::time::Instant::now();

        // Create 5 agents concurrently, guarded by a consumer-level timeout so a
        // true GIL deadlock fails the test instead of hanging forever.
        let (a1, a2, a3, a4, a5) =
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::join!(
                    BRIDGE.agent(agent_config(&url, "Agent 1")),
                    BRIDGE.agent(agent_config(&url, "Agent 2")),
                    BRIDGE.agent(agent_config(&url, "Agent 3")),
                    BRIDGE.agent(agent_config(&url, "Agent 4")),
                    BRIDGE.agent(agent_config(&url, "Agent 5")),
                )
            })
            .await
            .expect("concurrent agent creation stalled — possible GIL deadlock");

        let elapsed = start.elapsed();
        eprintln!("5 concurrent agent creations took {elapsed:.1?}");

        // All must succeed.
        let a1 = a1.expect("agent 1");
        let a2 = a2.expect("agent 2");
        let a3 = a3.expect("agent 3");
        let a4 = a4.expect("agent 4");
        let a5 = a5.expect("agent 5");

        // GIL lockup detection: 5 concurrent creates should not take
        // 5 × sequential time. A GIL-blocked create would serialize startup.
        assert!(
            elapsed.as_secs() < 14,
            "5 concurrent creates took {elapsed:.1?} — possible GIL deadlock"
        );

        // Chat with all to verify they're functional.
        let (r1, r2, r3, r4, r5) = tokio::join!(
            a1.chat_text("ping"),
            a2.chat_text("ping"),
            a3.chat_text("ping"),
            a4.chat_text("ping"),
            a5.chat_text("ping"),
        );

        assert!(r1.is_ok(), "Agent 1 chat failed: {r1:?}");
        assert!(r2.is_ok(), "Agent 2 chat failed: {r2:?}");
        assert!(r3.is_ok(), "Agent 3 chat failed: {r3:?}");
        assert!(r4.is_ok(), "Agent 4 chat failed: {r4:?}");
        assert!(r5.is_ok(), "Agent 5 chat failed: {r5:?}");

        // Clean shutdown.
        a1.shutdown().await.expect("shutdown a1");
        a2.shutdown().await.expect("shutdown a2");
        a3.shutdown().await.expect("shutdown a3");
        a4.shutdown().await.expect("shutdown a4");
        a5.shutdown().await.expect("shutdown a5");
    });
}

// =============================================================================
// 2. Ongoing conversation survives peer teardown
// =============================================================================

/// Agent B is mid-chat (slow response) while Agent A shuts down.
/// Agent B's response must complete successfully.
#[test]
fn ongoing_chat_survives_peer_shutdown() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let fast_server = MockServer::start("fast").await;
        let slow_server =
            MockServer::start_slow("slow", std::time::Duration::from_millis(500)).await;

        let agent_a = BRIDGE
            .agent(agent_config(&fast_server.base_url(), "fast-agent"))
            .await
            .expect("agent A");
        let agent_b = BRIDGE
            .agent(agent_config(&slow_server.base_url(), "slow-agent"))
            .await
            .expect("agent B");

        // Warm up agent A.
        let warmup = agent_a.chat_text("hello").await;
        assert!(warmup.is_ok(), "warmup failed: {warmup:?}");

        // Start B's slow chat and A's shutdown concurrently.
        let (b_result, a_shutdown) =
            tokio::join!(agent_b.chat_text("slow request"), agent_a.shutdown(),);

        // A must shut down cleanly.
        a_shutdown.expect("agent A shutdown");

        // B must complete its chat despite A shutting down mid-flight.
        let b_text = b_result.expect("agent B chat should succeed during A's shutdown");
        assert!(
            b_text.contains("mock:slow"),
            "Expected slow mock response, got: {b_text}"
        );

        agent_b.shutdown().await.expect("agent B shutdown");
    });
}

// =============================================================================
// 3. New agent creation during peer teardown
// =============================================================================

/// While agent A is shutting down, create a brand new agent C. C must work.
#[test]
fn new_agent_during_peer_shutdown() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let server = MockServer::start("create-during-shutdown").await;
        let url = server.base_url();

        let agent_a = BRIDGE
            .agent(agent_config(&url, "A"))
            .await
            .expect("agent A");

        // Warm up A.
        agent_a.chat_text("warmup").await.expect("A warmup");

        // Start A's shutdown and C's creation concurrently.
        let (a_shutdown, c_creation) = tokio::join!(
            agent_a.shutdown(),
            BRIDGE.agent(agent_config(&url, "C-new")),
        );

        a_shutdown.expect("A shutdown");
        let agent_c = c_creation.expect("agent C creation during A shutdown");

        // C must be fully functional.
        let c_text = agent_c.chat_text("hello from C").await.expect("C chat");
        assert!(
            c_text.contains("mock:create-during-shutdown"),
            "Expected mock response from C, got: {c_text}"
        );

        agent_c.shutdown().await.expect("C shutdown");
    });
}

// =============================================================================
// 4. Post-teardown agent creation — no stale state
// =============================================================================

/// After fully shutting down all agents and dropping them, create new agents.
/// The new agents must work with zero interference from previous state.
#[test]
fn agents_work_after_full_teardown() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let server = MockServer::start("post-teardown").await;
        let url = server.base_url();

        // Phase 1: create, chat, shutdown.
        {
            let agent = BRIDGE
                .agent(agent_config(&url, "phase1"))
                .await
                .expect("phase1 agent");
            let text = agent.chat_text("phase1").await.expect("phase1 chat");
            assert!(text.contains("mock:post-teardown"), "phase1 got: {text}");
            agent.shutdown().await.expect("phase1 shutdown");
        }

        // Phase 2: create new agents — must work fine.
        let agent_new = BRIDGE
            .agent(agent_config(&url, "phase2"))
            .await
            .expect("phase2 agent");
        let text = agent_new.chat_text("phase2").await.expect("phase2 chat");
        assert!(text.contains("mock:post-teardown"), "phase2 got: {text}");
        agent_new.shutdown().await.expect("phase2 shutdown");

        assert!(
            server.post_count() >= 2,
            "Expected at least 2 POST requests (one per phase), got {}",
            server.post_count()
        );
    });
}

// =============================================================================
// 5. No Python object leaks — rapid create/shutdown cycles
// =============================================================================

/// Create and shut down 10 agents in rapid succession. If Python objects leak,
/// the bridge state registry grows unboundedly and eventually each new agent
/// becomes slower (or the process runs out of memory/file-descriptors).
///
/// We measure that the Nth cycle is not dramatically slower than the 1st —
/// which would indicate accumulated leaked state.
#[test]
fn no_python_object_leaks_rapid_cycles() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        const CYCLES: usize = 10;

        // A dedicated bridge so the live-agent count is isolated from any
        // other test that shares `BRIDGE` and may run in parallel.
        let bridge = agy_bridge::AgyBridge::builder()
            .inter_agent_delay(std::time::Duration::ZERO)
            .build()
            .expect("dedicated leak-test bridge");

        let server = MockServer::start("leak-test").await;
        let url = server.base_url();

        assert_eq!(
            bridge
                .active_agent_count()
                .await
                .expect("count on fresh bridge"),
            0,
            "a fresh bridge must have no live agents"
        );

        for i in 0..CYCLES {
            let agent = bridge
                .agent(agent_config(&url, &format!("cycle-{i}")))
                .await
                .unwrap_or_else(|e| panic!("agent creation failed at cycle {i}: {e}"));

            assert_eq!(
                bridge
                    .active_agent_count()
                    .await
                    .expect("count with live agent"),
                1,
                "exactly one agent should be live during cycle {i}"
            );

            let text = agent
                .chat_text(format!("cycle {i}"))
                .await
                .unwrap_or_else(|e| panic!("chat failed at cycle {i}: {e}"));
            assert!(text.contains("mock:leak-test"), "cycle {i} got: {text}");

            agent
                .shutdown()
                .await
                .unwrap_or_else(|e| panic!("shutdown failed at cycle {i}: {e}"));

            // Deterministic leak check: `shutdown()` removes the agent from the
            // registry *before* it returns, so the count must be exactly zero.
            // A leak would leave the entry behind and the count would grow.
            let live = bridge
                .active_agent_count()
                .await
                .expect("count after shutdown");
            assert_eq!(
                live, 0,
                "agent leak after cycle {i}: {live} agent(s) still registered"
            );
        }

        // Dropping a handle *without* an explicit shutdown must also clean up
        // (best-effort async shutdown fired from `Drop`). The registry must
        // drain back to zero.
        {
            let agent = bridge
                .agent(agent_config(&url, "drop-cycle"))
                .await
                .expect("drop-cycle agent");
            assert_eq!(
                bridge
                    .active_agent_count()
                    .await
                    .expect("count before drop"),
                1,
                "one live agent before drop"
            );
            drop(agent);
        }
        assert!(
            wait_for_zero_agents(&bridge).await,
            "agent dropped without shutdown() was not cleaned up — leak"
        );

        assert_eq!(
            server.post_count(),
            CYCLES,
            "expected exactly {CYCLES} POST requests"
        );
    });
}

// =============================================================================
// 6. Concurrent chat during sequential teardown
// =============================================================================

/// 3 agents all chatting. Agent A finishes and shuts down. Then B. C must
/// still work throughout and after both teardowns.
#[test]
fn sequential_teardown_while_others_chat() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let server = MockServer::start("seq-teardown").await;
        let url = server.base_url();

        let a = BRIDGE.agent(agent_config(&url, "A")).await.expect("A");
        let b = BRIDGE.agent(agent_config(&url, "B")).await.expect("B");
        let c = BRIDGE.agent(agent_config(&url, "C")).await.expect("C");

        // All chat concurrently.
        let (ra, rb, rc) = tokio::join!(
            a.chat_text("hello A"),
            b.chat_text("hello B"),
            c.chat_text("hello C"),
        );
        assert!(ra.is_ok(), "A chat failed: {ra:?}");
        assert!(rb.is_ok(), "B chat failed: {rb:?}");
        assert!(rc.is_ok(), "C chat failed: {rc:?}");

        // Shut down A, then verify B and C still work.
        a.shutdown().await.expect("A shutdown");
        let rb2 = b.chat_text("after A shutdown").await;
        let rc2 = c.chat_text("after A shutdown").await;
        assert!(rb2.is_ok(), "B should work after A shutdown: {rb2:?}");
        assert!(rc2.is_ok(), "C should work after A shutdown: {rc2:?}");

        // Shut down B, verify C still works.
        b.shutdown().await.expect("B shutdown");
        let rc3 = c.chat_text("after B shutdown").await;
        assert!(rc3.is_ok(), "C should work after B shutdown: {rc3:?}");

        c.shutdown().await.expect("C shutdown");
    });
}

mod multi_bridge_and_stress;
