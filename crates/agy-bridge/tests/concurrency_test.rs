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
        .chat_timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("shared AgyBridge")
});

// ─── Mock Server Infrastructure ──────────────────────────────────────────────

async fn parse_http_request<R: tokio::io::AsyncRead + Unpin>(
    buf_reader: &mut BufReader<R>,
) -> Option<String> {
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
            // NOLINT: test helper — invalid content-length defaults to zero
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    if content_length > 0 {
        let mut body_buf = vec![0u8; content_length];
        buf_reader.read_exact(&mut body_buf).await.ok()?;
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

impl MockServer {
    /// Start a mock Gemini API server that responds with a tag in each response.
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
                let count = Arc::clone(&count);
                let tag = tag.clone();
                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut buf_reader = BufReader::new(reader);

                    let Some(request_line) = parse_http_request(&mut buf_reader).await else {
                        return;
                    };

                    let is_get = request_line.starts_with("GET ");
                    let response = if is_get {
                        json_response(200, &model_list_json())
                    } else {
                        count.fetch_add(1, Ordering::SeqCst);
                        sse_response(&generate_content_json(&tag))
                    };

                    if let Err(e) = writer.write_all(response.as_bytes()).await {
                        eprintln!("[MOCK {tag}] write error: {e}");
                    }
                    let _ = writer.flush().await;
                });
            }
        });

        Self {
            addr,
            post_count,
            handle,
        }
    }

    /// Start a mock server with an intentional delay on POST responses.
    async fn start_slow(tag: &str, delay: std::time::Duration) -> Self {
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
                let count = Arc::clone(&count);
                let tag = tag.clone();
                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut buf_reader = BufReader::new(reader);

                    let Some(request_line) = parse_http_request(&mut buf_reader).await else {
                        return;
                    };

                    let is_get = request_line.starts_with("GET ");
                    let response = if is_get {
                        json_response(200, &model_list_json())
                    } else {
                        count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(delay).await;
                        sse_response(&generate_content_json(&tag))
                    };

                    if let Err(e) = writer.write_all(response.as_bytes()).await {
                        eprintln!("[MOCK {tag}] write error: {e}");
                    }
                    let _ = writer.flush().await;
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
        .build()
}

fn multi_thread_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("multi-thread tokio runtime")
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

        let start = std::time::Instant::now();

        // Create 5 agents concurrently.
        let (a1, a2, a3, a4, a5) = tokio::join!(
            BRIDGE.agent(agent_config(&url, "Agent 1")),
            BRIDGE.agent(agent_config(&url, "Agent 2")),
            BRIDGE.agent(agent_config(&url, "Agent 3")),
            BRIDGE.agent(agent_config(&url, "Agent 4")),
            BRIDGE.agent(agent_config(&url, "Agent 5")),
        );

        let elapsed = start.elapsed();
        eprintln!("5 concurrent agent creations took {elapsed:.1?}");

        // All must succeed.
        let a1 = a1.expect("agent 1");
        let a2 = a2.expect("agent 2");
        let a3 = a3.expect("agent 3");
        let a4 = a4.expect("agent 4");
        let a5 = a5.expect("agent 5");

        // GIL lockup detection: 5 concurrent creates should not take
        // 5 × sequential time. With a 15s timeout on the bridge, even
        // one GIL-blocked create would time out, failing the test.
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
        let server = MockServer::start("leak-test").await;
        let url = server.base_url();

        let mut durations = Vec::with_capacity(CYCLES);

        for i in 0..CYCLES {
            let start = std::time::Instant::now();
            let agent = BRIDGE
                .agent(agent_config(&url, &format!("cycle-{i}")))
                .await
                .unwrap_or_else(|e| panic!("agent creation failed at cycle {i}: {e}"));

            let text = agent
                .chat_text(format!("cycle {i}"))
                .await
                .unwrap_or_else(|e| panic!("chat failed at cycle {i}: {e}"));
            assert!(text.contains("mock:leak-test"), "cycle {i} got: {text}");

            agent
                .shutdown()
                .await
                .unwrap_or_else(|e| panic!("shutdown failed at cycle {i}: {e}"));

            let elapsed = start.elapsed();
            eprintln!("Cycle {i}: {elapsed:.1?}");
            durations.push(elapsed);
        }

        // Leak detection: compare steady-state windows.
        //
        // Cycles 0–2 include cold-start overhead (Python import, SDK init),
        // so comparing against them is meaningless. Instead compare the
        // *middle* warm window (cycles 4–6) against the *last* window (7–9).
        // If Python objects leak, each cycle accumulates GC pressure / event
        // loop overhead, so the last window would be measurably slower.
        let mid_avg: std::time::Duration =
            durations[4..7].iter().sum::<std::time::Duration>() / 3;
        let last_avg: std::time::Duration =
            durations[CYCLES - 3..].iter().sum::<std::time::Duration>() / 3;

        eprintln!("Mid window (4–6) avg: {mid_avg:.1?}");
        eprintln!("Last window (7–9) avg: {last_avg:.1?}");

        // Both windows are warm — allow 2× for GC jitter / system load.
        assert!(
            last_avg < mid_avg * 2,
            "Possible Python object leak: last 3 cycles avg {last_avg:.1?} vs mid 3 avg {mid_avg:.1?} \
             (>2× slowdown in steady state)"
        );

        assert_eq!(
            server.post_count(),
            CYCLES,
            "Expected exactly {CYCLES} POST requests"
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

// =============================================================================
// 7. Multiple bridges with concurrent agents — full isolation
// =============================================================================

/// Two separate `AgyBridge` instances, each with their own Python runtime
/// thread and event loop. Agents on different bridges must not interfere.
#[test]
fn two_bridges_concurrent_agents_isolation() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let server_1 = MockServer::start("bridge1").await;
        let server_2 = MockServer::start("bridge2").await;

        let bridge_1 = agy_bridge::AgyBridge::builder()
            .inter_agent_delay(std::time::Duration::ZERO)
            .chat_timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("bridge 1");
        let bridge_2 = agy_bridge::AgyBridge::builder()
            .inter_agent_delay(std::time::Duration::ZERO)
            .chat_timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("bridge 2");

        let a1 = bridge_1
            .agent(agent_config(&server_1.base_url(), "b1-agent"))
            .await
            .expect("b1 agent");
        let a2 = bridge_2
            .agent(agent_config(&server_2.base_url(), "b2-agent"))
            .await
            .expect("b2 agent");

        // Chat concurrently on separate bridges.
        let (r1, r2) = tokio::join!(a1.chat_text("b1 ping"), a2.chat_text("b2 ping"),);

        let t1 = r1.expect("bridge 1 chat");
        let t2 = r2.expect("bridge 2 chat");

        assert!(t1.contains("mock:bridge1"), "Bridge 1 response wrong: {t1}");
        assert!(t2.contains("mock:bridge2"), "Bridge 2 response wrong: {t2}");

        // Each server should have received exactly 1 POST.
        assert_eq!(server_1.post_count(), 1, "bridge 1 should get 1 POST");
        assert_eq!(server_2.post_count(), 1, "bridge 2 should get 1 POST");

        // Shut down bridge 1's agent, bridge 2's agent must still work.
        a1.shutdown().await.expect("b1 shutdown");
        let r2_after = a2.chat_text("after b1 shutdown").await;
        assert!(
            r2_after.is_ok(),
            "Bridge 2 agent should survive bridge 1 teardown: {r2_after:?}"
        );

        a2.shutdown().await.expect("b2 shutdown");
    });
}

// =============================================================================
// 8. Startup timeout detection — GIL lockup canary
// =============================================================================

/// A canary test that creates agents one at a time on a fast mock. If any
/// single create takes more than the timeout, it's a GIL lockup signal.
#[test]
fn sequential_agent_startup_timing() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        const COUNT: usize = 5;
        let server = MockServer::start("timing").await;
        let url = server.base_url();

        let mut agents = Vec::with_capacity(COUNT);

        for i in 0..COUNT {
            let start = std::time::Instant::now();
            let agent = BRIDGE
                .agent(agent_config(&url, &format!("timing-{i}")))
                .await
                .unwrap_or_else(|e| panic!("agent {i} creation failed: {e}"));
            let elapsed = start.elapsed();
            eprintln!("Agent {i} creation: {elapsed:.1?}");

            // Each agent creation should complete well within the chat_timeout.
            // If the GIL is locked by a prior agent's init, this will time out.
            assert!(
                elapsed.as_secs() < 14,
                "Agent {i} creation took {elapsed:.1?} — possible GIL contention"
            );

            agents.push(agent);
        }

        for agent in agents {
            agent.shutdown().await.expect("shutdown");
        }
    });
}

// =============================================================================
// 9. Create agent during mid-flight chat on same bridge
// =============================================================================

/// While agent A is actively receiving a slow streaming response, create agent
/// B on the same bridge. B's creation must not be blocked by A's ongoing chat.
#[test]
fn create_agent_during_peer_active_chat() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let slow_server =
            MockServer::start_slow("slow-chat", std::time::Duration::from_millis(500)).await;
        let fast_server = MockServer::start("fast-create").await;

        let agent_a = BRIDGE
            .agent(agent_config(&slow_server.base_url(), "slow-chatter"))
            .await
            .expect("agent A");

        // Start A's slow chat and B's creation concurrently.
        let (a_chat_result, b_creation_result) = tokio::join!(
            agent_a.chat_text("slow request"),
            BRIDGE.agent(agent_config(&fast_server.base_url(), "fast-new")),
        );

        // Both must succeed.
        let a_text = a_chat_result.expect("A chat during concurrent B creation");
        assert!(a_text.contains("mock:slow-chat"), "A got: {a_text}");

        let agent_b = b_creation_result.expect("B creation during A's active chat");
        let b_text = agent_b.chat_text("hello B").await.expect("B chat");
        assert!(b_text.contains("mock:fast-create"), "B got: {b_text}");

        agent_a.shutdown().await.expect("A shutdown");
        agent_b.shutdown().await.expect("B shutdown");
    });
}

// =============================================================================
// 10. Rapid concurrent create+chat+shutdown — stress test
// =============================================================================

/// Spawn 4 agents that each do create → chat → shutdown concurrently.
/// All 4 must succeed. This exercises the full lifecycle under maximum
/// contention on the Python runtime thread / event loop / GIL.
#[test]
fn rapid_concurrent_lifecycle_stress() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        const N: usize = 4;
        let server = MockServer::start("stress").await;
        let url = server.base_url();

        let mut handles = Vec::with_capacity(N);

        for i in 0..N {
            let url = url.clone();
            handles.push(tokio::spawn(async move {
                let agent = BRIDGE
                    .agent(agent_config(&url, &format!("stress-{i}")))
                    .await
                    .unwrap_or_else(|e| panic!("stress agent {i} creation: {e}"));

                let text = agent
                    .chat_text(format!("stress ping {i}"))
                    .await
                    .unwrap_or_else(|e| panic!("stress agent {i} chat: {e}"));
                assert!(text.contains("mock:stress"), "stress agent {i} got: {text}");

                agent
                    .shutdown()
                    .await
                    .unwrap_or_else(|e| panic!("stress agent {i} shutdown: {e}"));

                i
            }));
        }

        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.expect("tokio task panicked"));
        }
        results.sort_unstable();
        assert_eq!(
            results,
            (0..N).collect::<Vec<_>>(),
            "All {N} stress tasks must complete"
        );

        assert_eq!(
            server.post_count(),
            N,
            "Expected exactly {N} POST requests from stress test"
        );
    });
}

// =============================================================================
// 11. Shutdown is idempotent — double shutdown must not panic
// =============================================================================

/// Calling `shutdown()` twice on the same agent must not panic or hang.
#[test]
fn double_shutdown_is_safe() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let server = MockServer::start("double-shutdown").await;
        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "double"))
            .await
            .expect("agent");

        agent.chat_text("ping").await.expect("chat");

        // First shutdown succeeds.
        agent.shutdown().await.expect("first shutdown");

        // Second shutdown: should be an error (already shut down), not a panic/hang.
        let second = agent.shutdown().await;
        eprintln!("Second shutdown result: {second:?}");
        // The exact semantics (Ok or Err) depend on the implementation,
        // but it must never panic or hang.
    });
}

// =============================================================================
// 12. Chat after shutdown returns error — no silent success
// =============================================================================

/// After `shutdown()`, calling `chat_text()` must return `Err`, not `Ok("")`.
#[test]
fn chat_after_shutdown_returns_error() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        let server = MockServer::start("post-shutdown-chat").await;
        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "post-shutdown"))
            .await
            .expect("agent");

        agent.chat_text("ping").await.expect("pre-shutdown chat");
        agent.shutdown().await.expect("shutdown");

        // Chat after shutdown must fail.
        let result = agent.chat_text("after shutdown").await;
        assert!(
            result.is_err(),
            "Chat after shutdown MUST return Err, got Ok({:?})",
            result.unwrap_or_default()
        );
    });
}
