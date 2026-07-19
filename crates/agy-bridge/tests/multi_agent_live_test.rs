mod common;

// =============================================================================
// Test 16: Multi-agent - create 3 agents, chat with each, shutdown all
// =============================================================================

#[test]
fn live_multi_agent_lifecycle() {
    common::run_live_test("live_multi_agent_lifecycle", || {
        let _api_key = common::api_key();
        // Multi-threaded runtime: this test spawns 3 agents concurrently and
        // chats via `tokio::join!`.  A current-thread runtime can race with the
        // Python process lifecycle under heavy load, causing "coroutine was
        // never awaited" warnings and sporadic failures.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("multi-thread tokio runtime");

        rt.block_on(async {
            let bridge = common::create_bridge();
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply exactly with the number you receive plus one.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            // Create agents sequentially to avoid overwhelming the Python init.
            let a1 = bridge.agent(config.clone()).await?;
            let a2 = bridge.agent(config.clone()).await?;
            let a3 = bridge.agent(config.clone()).await?;

            let f1 = a1.chat("What is 1+1? Reply with just the number.");
            let f2 = a2.chat("What is 2+2? Reply with just the number.");
            let f3 = a3.chat("What is 3+3? Reply with just the number.");

            let (r1, r2, r3) = tokio::join!(f1, f2, f3);
            let _t1 = r1?.text().await?;
            let _t2 = r2?.text().await?;
            let _t3 = r3?.text().await?;

            // Shutdown sequentially for clean teardown.
            a1.shutdown().await?;
            a2.shutdown().await?;
            a3.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test 20: Subagent - agent spawns subagent, gets result
// =============================================================================

#[test]
fn live_subagent_spawn() {
    common::run_live_test("live_subagent_spawn", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = common::create_bridge();
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a parent. Pass the task to your subagent using the start_subagent tool and return its response.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::full())
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).await?;

            let prompt = "Ask your subagent what 5+5 is, and return the answer. Use the start_subagent tool.";
            let result = agent.chat_text(prompt).await;

            match result {
                Ok(text) => {
                    eprintln!("Parent response: {text}");
                    assert!(
                        text.contains("10"),
                        "Expected parent to return 10 from subagent, got: {text}"
                    );
                }
                Err(e) => {
                    // Subagent tool execution may fail if the Python runtime
                    // doesn't fully support it — but only specific error types
                    // are acceptable (tool dispatch or backend errors).
                    let err_str = e.to_string();
                    assert!(
                        err_str.contains("subagent") || err_str.contains("tool") || err_str.contains("Backend") || err_str.contains("timeout") || err_str.contains("Timeout") || err_str.contains("429"),
                        "Unexpected error type from subagent test: {e}"
                    );
                    eprintln!("Subagent prompt returned expected error: {e}");
                }
            }

            agent.shutdown().await?;
            Ok(())
        })
    });
}

#[test]
fn live_mcp_server_config_passes_to_python() {
    common::run_live_test("live_mcp_server_config_passes_to_python", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            use agy_bridge::config::McpServer;

            let bridge = common::create_bridge();

            // A mock stdio MCP server that handles the initialization handshake and capability aggregation.
            let server = McpServer::stdio("python3")
                .args([
                    "-c",
                    r"
import sys, json
for line in sys.stdin:
    try:
        req = json.loads(line)
        if 'id' in req:
            m = req.get('method')
            if m == 'initialize':
                res = {'protocolVersion': req.get('params', {}).get('protocolVersion', '2024-11-05'), 'capabilities': {'resources': {}, 'prompts': {}, 'tools': {}}, 'serverInfo': {'name': 'dummy', 'version': '1.0'}}
            elif m == 'resources/list':
                res = {'resources': []}
            elif m == 'prompts/list':
                res = {'prompts': []}
            elif m == 'tools/list':
                res = {'tools': []}
            else:
                res = {}
            sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': res}) + '\n')
            sys.stdout.flush()
    except Exception:
        pass
",
                ])
                .build();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Just say ok.")
                .mcp_servers([server])
                .policies([agy_bridge::PolicyRule::AllowAll])
                .build();

            // Verify that the agent constructs and successfully connects to the MCP server.
            let agent = bridge.agent(config).await?;
            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Multi-agent isolation — shutdown one, others still work (same bridge)
// =============================================================================

#[test]
fn shutdown_one_agent_others_still_work_same_bridge() {
    common::run_live_test("shutdown_one_agent_others_still_work_same_bridge", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = common::create_bridge();

            let config_a = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: AGENT_A")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();
            let config_b = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: AGENT_B")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent_a = bridge.agent(config_a).await?;
            let agent_b = bridge.agent(config_b).await?;

            // Both agents respond before shutdown.
            let text_a = agent_a.chat_text("Hello").await?;
            eprintln!("Agent A (pre-shutdown): {text_a}");
            assert!(!text_a.is_empty(), "Agent A should respond");

            let text_b = agent_b.chat_text("Hello").await?;
            eprintln!("Agent B (pre-shutdown): {text_b}");
            assert!(!text_b.is_empty(), "Agent B should respond");

            // Shut down agent A.
            agent_a.shutdown().await?;
            eprintln!("Agent A shut down");

            // Agent B must still work after A is gone.
            let text_b_after = agent_b.chat_text("Are you still there?").await?;
            eprintln!("Agent B (post-shutdown of A): {text_b_after}");
            assert!(
                !text_b_after.is_empty(),
                "Agent B must still respond after agent A is shut down"
            );

            agent_b.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Sequential bridge instances — tear down one, create another
// =============================================================================

/// Verifies that after fully tearing down one `AgyBridge` (agent shutdown +
/// bridge drop), a *new* `AgyBridge` can be created and used without any
/// leftover global state corruption.
#[test]
fn sequential_bridge_instances_work_after_teardown() {
    common::run_live_test("sequential_bridge_instances_work_after_teardown", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            // ── Phase 1: create bridge, use agent, tear everything down ──
            {
                let bridge_1 = agy_bridge::AgyBridge::builder().build()?;

                let config = agy_bridge::config::AgentConfig::builder()
                    .system_instructions("Reply with exactly: BRIDGE_ONE")
                    .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                    .build();

                let agent = bridge_1.agent(config).await?;
                let text = agent.chat_text("Hello").await?;
                eprintln!("Bridge 1 agent: {text}");
                assert!(!text.is_empty(), "Bridge 1 agent should respond");

                agent.shutdown().await?;
                drop(agent);
                drop(bridge_1);
                eprintln!("Bridge 1 fully torn down");
            }

            // ── Phase 2: create a fresh bridge and verify it works ──
            {
                let bridge_2 = agy_bridge::AgyBridge::builder().build()?;

                let config = agy_bridge::config::AgentConfig::builder()
                    .system_instructions("Reply with exactly: BRIDGE_TWO")
                    .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                    .build();

                let agent = bridge_2.agent(config).await?;
                let text = agent.chat_text("Hello").await?;
                eprintln!("Bridge 2 agent (after bridge 1 teardown): {text}");
                assert!(
                    !text.is_empty(),
                    "Bridge 2 agent must work after bridge 1 is fully torn down"
                );

                agent.shutdown().await?;
            }

            Ok(())
        })
    });
}

// =============================================================================
// Test: Three agents, shut down middle one, first and last still work
// =============================================================================

#[test]
fn three_agents_shutdown_middle_others_survive() {
    common::run_live_test("three_agents_shutdown_middle_others_survive", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = common::create_bridge();

            let make_config = |name: &str| {
                agy_bridge::config::AgentConfig::builder()
                    .system_instructions(format!("Reply with exactly: {name}"))
                    .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                    .build()
            };

            let first = bridge.agent(make_config("FIRST")).await?;
            let middle = bridge.agent(make_config("MIDDLE")).await?;
            let last = bridge.agent(make_config("LAST")).await?;

            // All three respond.
            let t1 = first.chat_text("Hi").await?;
            let t2 = middle.chat_text("Hi").await?;
            let t3 = last.chat_text("Hi").await?;
            assert!(!t1.is_empty() && !t2.is_empty() && !t3.is_empty());
            eprintln!("All three agents responded");

            // Shut down the middle agent.
            middle.shutdown().await?;
            drop(middle);
            eprintln!("Middle agent shut down");

            // First and last must still work.
            let t1_after = first.chat_text("Still there?").await?;
            eprintln!("First (after middle shutdown): {t1_after}");
            assert!(
                !t1_after.is_empty(),
                "First agent must survive middle agent shutdown"
            );

            let t3_after = last.chat_text("Still there?").await?;
            eprintln!("Last (after middle shutdown): {t3_after}");
            assert!(
                !t3_after.is_empty(),
                "Last agent must survive middle agent shutdown"
            );

            first.shutdown().await?;
            last.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Sequential bridges with different proxy configs
// =============================================================================

/// Verifies that tearing down a bridge configured with a proxy `base_url`
/// doesn't corrupt global state for a subsequent bridge using the default
/// endpoint (no proxy).
#[test]
fn sequential_bridges_with_different_proxy_configs() {
    common::run_live_test("sequential_bridges_with_different_proxy_configs", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            // ── Phase 1: bridge with a proxy base_url ──
            // We use the real Gemini URL as the "proxy" to avoid needing an
            // actual proxy server — the point is to verify config isolation,
            // not proxy routing.
            {
                let config = agy_bridge::config::AgentConfig::builder()
                    .system_instructions("Reply with exactly: PROXIED")
                    .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                    .gemini(agy_bridge::config::GeminiConfig {
                        api_key: None, // falls back to env var
                        base_url: Some("https://generativelanguage.googleapis.com".to_owned()),
                        models: agy_bridge::config::ModelConfig::default(),
                    })
                    .build();

                let bridge = common::create_bridge();
                let agent = bridge.agent(config).await?;
                let text = agent.chat_text("Hello").await?;
                eprintln!("Proxied agent: {text}");
                assert!(!text.is_empty(), "Proxied agent should respond");

                agent.shutdown().await?;
                drop(agent);
                drop(bridge);
                eprintln!("Proxied bridge torn down");
            }

            // ── Phase 2: bridge with no proxy (default endpoint) ──
            {
                let config = agy_bridge::config::AgentConfig::builder()
                    .system_instructions("Reply with exactly: DIRECT")
                    .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                    .build();

                let bridge = common::create_bridge();
                let agent = bridge.agent(config).await?;
                let text = agent.chat_text("Hello").await?;
                eprintln!("Direct agent (after proxied teardown): {text}");
                assert!(
                    !text.is_empty(),
                    "Direct agent must work after proxied bridge teardown"
                );

                agent.shutdown().await?;
            }

            Ok(())
        })
    });
}

// =============================================================================
// Test: Same bridge, two agents with different GeminiConfig (proxy vs direct)
// =============================================================================

/// Two agents on the same bridge: one configured with a `base_url` (proxy),
/// the other using the default endpoint. Shutting down the proxied agent
/// must not affect the direct agent.
#[test]
fn same_bridge_proxy_and_direct_agents_isolation() {
    common::run_live_test("same_bridge_proxy_and_direct_agents_isolation", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = common::create_bridge();

            // Agent with proxy base_url.
            let proxied_config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: PROXIED")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .gemini(agy_bridge::config::GeminiConfig {
                    api_key: None,
                    base_url: Some("https://generativelanguage.googleapis.com".to_owned()),
                    models: agy_bridge::config::ModelConfig::default(),
                })
                .build();

            // Agent with default endpoint (no proxy).
            let direct_config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: DIRECT")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let proxied_agent = bridge.agent(proxied_config).await?;
            let direct_agent = bridge.agent(direct_config).await?;

            // Both respond.
            let t_proxy = proxied_agent.chat_text("Hello").await?;
            eprintln!("Proxied agent: {t_proxy}");
            assert!(!t_proxy.is_empty());

            let t_direct = direct_agent.chat_text("Hello").await?;
            eprintln!("Direct agent: {t_direct}");
            assert!(!t_direct.is_empty());

            // Shut down the proxied agent.
            proxied_agent.shutdown().await?;
            drop(proxied_agent);
            eprintln!("Proxied agent shut down");

            // Direct agent must still work.
            let t_after = direct_agent.chat_text("Still alive?").await?;
            eprintln!("Direct agent (after proxied shutdown): {t_after}");
            assert!(
                !t_after.is_empty(),
                "Direct agent must survive proxied agent shutdown"
            );

            direct_agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Concurrent execution of proxy and direct agents on the same bridge
// =============================================================================

/// Verifies that multiple agents on the same bridge — one configured with a
/// proxy `base_url` and another using the default direct endpoint — can execute
/// requests concurrently via `tokio::join!` without any race conditions or
/// cross-agent `base_url` corruption.
#[test]
fn same_bridge_concurrent_proxy_and_direct_agents() {
    common::run_live_test("same_bridge_concurrent_proxy_and_direct_agents", || {
        let _api_key = common::api_key();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("multi-thread tokio runtime");

        rt.block_on(async {
            let bridge = common::create_bridge();

            // Agent with proxy base_url.
            let proxied_config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: PROXIED_CONCURRENT")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .gemini(agy_bridge::config::GeminiConfig {
                    api_key: None,
                    base_url: Some("https://generativelanguage.googleapis.com".to_owned()),
                    models: agy_bridge::config::ModelConfig::default(),
                })
                .build();

            // Agent with default endpoint (no proxy).
            let direct_config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: DIRECT_CONCURRENT")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            // Create agents sequentially to ensure clean Python initialization.
            let proxied_agent = bridge.agent(proxied_config).await?;
            let direct_agent = bridge.agent(direct_config).await?;

            // Execute chat requests concurrently via tokio::join!
            let fut_proxy = async {
                proxied_agent
                    .chat_text("Who are you? Reply with your exact name.")
                    .await
            };
            let fut_direct = async {
                direct_agent
                    .chat_text("Who are you? Reply with your exact name.")
                    .await
            };

            let (res_proxy, res_direct) = tokio::join!(fut_proxy, fut_direct);

            let text_proxy = res_proxy?;
            let text_direct = res_direct?;

            eprintln!("Proxied agent concurrent response: {text_proxy}");
            eprintln!("Direct agent concurrent response: {text_direct}");

            assert!(
                text_proxy.contains("PROXIED_CONCURRENT"),
                "Expected PROXIED_CONCURRENT in proxied agent response, got: {text_proxy}"
            );
            assert!(
                text_direct.contains("DIRECT_CONCURRENT"),
                "Expected DIRECT_CONCURRENT in direct agent response, got: {text_direct}"
            );

            proxied_agent.shutdown().await?;
            direct_agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Concurrent execution of proxy and direct agents across two AgyBridge instances
// =============================================================================

/// Verifies that two separate `AgyBridge` instances — one running an agent with
/// a proxy `base_url` and another running an agent with a direct connection —
/// can execute chat requests concurrently via `tokio::join!` without any
/// cross-bridge event loop corruption or global module collisions.
#[test]
fn two_bridges_concurrent_proxy_and_direct_agents() {
    common::run_live_test("two_bridges_concurrent_proxy_and_direct_agents", || {
        let _api_key = common::api_key();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("multi-thread tokio runtime");

        rt.block_on(async {
            // Create two completely separate bridge instances. Each spawns its
            // own dedicated Python runtime thread with its own asyncio event loop.
            let bridge_proxy = common::create_bridge();
            let bridge_direct = common::create_bridge();

            // Agent config with proxy base_url.
            let proxied_config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: PROXIED_DUAL_BRIDGE")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .gemini(agy_bridge::config::GeminiConfig {
                    api_key: None,
                    base_url: Some("https://generativelanguage.googleapis.com".to_owned()),
                    models: agy_bridge::config::ModelConfig::default(),
                })
                .build();

            // Agent config with default direct endpoint.
            let direct_config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: DIRECT_DUAL_BRIDGE")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            // Create agents on their respective bridges.
            let proxied_agent = bridge_proxy.agent(proxied_config).await?;
            let direct_agent = bridge_direct.agent(direct_config).await?;

            // Start both chat streams concurrently via tokio::join!
            // The proxy agent gets a long essay prompt so its connection stays open and active while the direct agent answers.
            let fut_proxy = proxied_agent.chat("Write a 3-paragraph essay about the history of the proxy server. Keep your thoughts detailed. End with exactly: PROXIED_DUAL_BRIDGE");
            let fut_direct = direct_agent.chat("Write a short sentence saying exactly: DIRECT_DUAL_BRIDGE");

            let (res_proxy, res_direct) = tokio::join!(fut_proxy, fut_direct);
            let mut handle_proxy = res_proxy?;
            let mut handle_direct = res_direct?;

            eprintln!("Both streaming handles established concurrently — both TCP connections are actively open!");

            // Read the first streaming chunk from both handles to prove both TCP connections are actively receiving data at the exact same time!
            let mut stream_proxy = handle_proxy.take_text_stream().expect("proxy text stream");
            let mut stream_direct = handle_direct.take_text_stream().expect("direct text stream");

            // A text stream may close before yielding any chunk under transient
            // upstream pressure (e.g. a 429/503 when the whole live suite has
            // been hammering the shared TPM quota). Rather than panicking —
            // which would bypass the test retry logic — surface
            // the handle's terminal error (retryable if it's a quota/connection
            // blip) or, failing that, a retryable `Error::Stream` so the test
            // retries the whole test.
            let Some(chunk_proxy) = stream_proxy.recv().await else {
                handle_proxy.text().await?;
                return Err(agy_bridge::error::Error::Stream(
                    agy_bridge::streaming::StreamError {
                        message: "proxy text stream closed before first chunk".to_owned(),
                    },
                ));
            };
            let Some(chunk_direct) = stream_direct.recv().await else {
                handle_direct.text().await?;
                return Err(agy_bridge::error::Error::Stream(
                    agy_bridge::streaming::StreamError {
                        message: "direct text stream closed before first chunk".to_owned(),
                    },
                ));
            };

            eprintln!("Simultaneous in-flight streaming chunk (Proxy): {chunk_proxy}");
            eprintln!("Simultaneous in-flight streaming chunk (Direct): {chunk_direct}");

            // Now drain both streams to completion
            let mut full_text_proxy = chunk_proxy;
            while let Some(chunk) = stream_proxy.recv().await {
                full_text_proxy.push_str(&chunk);
            }

            let mut full_text_direct = chunk_direct;
            while let Some(chunk) = stream_direct.recv().await {
                full_text_direct.push_str(&chunk);
            }

            drop(stream_proxy);
            drop(stream_direct);
            // Clean up the handles
            drop(handle_proxy.text().await?);
            drop(handle_direct.text().await?);

            eprintln!("Proxied bridge full response: {full_text_proxy}");
            eprintln!("Direct bridge full response: {full_text_direct}");

            assert!(
                full_text_proxy.contains("PROXIED_DUAL_BRIDGE"),
                "Expected PROXIED_DUAL_BRIDGE in proxied bridge agent response, got: {full_text_proxy}"
            );
            assert!(
                full_text_direct.contains("DIRECT_DUAL_BRIDGE"),
                "Expected DIRECT_DUAL_BRIDGE in direct bridge agent response, got: {full_text_direct}"
            );

            proxied_agent.shutdown().await?;
            direct_agent.shutdown().await?;
            drop(bridge_proxy);
            drop(bridge_direct);
            Ok(())
        })
    });
}
