use super::*;

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
            .build()
            .expect("bridge 1");
        let bridge_2 = agy_bridge::AgyBridge::builder()
            .inter_agent_delay(std::time::Duration::ZERO)
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
        r2_after.expect("Bridge 2 agent should survive bridge 1 teardown");

        a2.shutdown().await.expect("b2 shutdown");
    });
}

/// Concurrent agent creation across two *separate* bridges must not serialize,
/// block, or interfere. Even though agent IDs and the per-agent state registry
/// are process-global, each bridge's live-agent count reflects only its own
/// agents, and fully tearing down one bridge leaves the other untouched.
///
/// This is the deterministic regression guard for the lock-free, per-agent
/// keyed creation path (no process-global create mutex).
#[test]
fn concurrent_creation_across_bridges_no_interference() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        const PER_BRIDGE: usize = 4;

        let server = MockServer::start("multi-bridge").await;
        let url = server.base_url();

        let bridge_1 = agy_bridge::AgyBridge::builder()
            .inter_agent_delay(std::time::Duration::ZERO)
            .build()
            .expect("bridge 1");
        let bridge_2 = agy_bridge::AgyBridge::builder()
            .inter_agent_delay(std::time::Duration::ZERO)
            .build()
            .expect("bridge 2");

        // Create all agents on BOTH bridges fully concurrently. A process-wide
        // create lock (the bug this guards against) would serialize these.
        let (b1, b2) = (&bridge_1, &bridge_2);
        let (created_1, created_2) = tokio::join!(
            futures::future::join_all((0..PER_BRIDGE).map(|i| {
                let url = url.clone();
                async move { b1.agent(agent_config(&url, &format!("b1-{i}"))).await }
            })),
            futures::future::join_all((0..PER_BRIDGE).map(|i| {
                let url = url.clone();
                async move { b2.agent(agent_config(&url, &format!("b2-{i}"))).await }
            })),
        );

        let agents_1: Vec<_> = created_1
            .into_iter()
            .enumerate()
            .map(|(i, r)| r.unwrap_or_else(|e| panic!("bridge 1 agent {i} creation: {e}")))
            .collect();
        let agents_2: Vec<_> = created_2
            .into_iter()
            .enumerate()
            .map(|(i, r)| r.unwrap_or_else(|e| panic!("bridge 2 agent {i} creation: {e}")))
            .collect();

        // Deterministic isolation: each bridge counts only its own agents.
        assert_eq!(
            bridge_1.active_agent_count().await.expect("b1 count"),
            PER_BRIDGE,
            "bridge 1 must see exactly its own agents"
        );
        assert_eq!(
            bridge_2.active_agent_count().await.expect("b2 count"),
            PER_BRIDGE,
            "bridge 2 must see exactly its own agents"
        );

        // Agent IDs are process-globally unique — no collisions across bridges.
        let ids: Vec<u64> = agents_1
            .iter()
            .chain(agents_2.iter())
            .map(agy_bridge::agent::AgentHandle::id)
            .collect();
        let unique: std::collections::HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(
            ids.len(),
            unique.len(),
            "agent IDs must be globally unique across bridges: {ids:?}"
        );

        // Tear down ALL of bridge 1's agents — bridge 2 must be untouched.
        futures::future::join_all(
            agents_1
                .into_iter()
                .map(|a| async move { a.shutdown().await }),
        )
        .await
        .into_iter()
        .enumerate()
        .for_each(|(i, r)| r.unwrap_or_else(|e| panic!("bridge 1 shutdown {i}: {e}")));

        assert_eq!(
            bridge_1.active_agent_count().await.expect("b1 count after"),
            0,
            "bridge 1 must be drained after shutting down all its agents"
        );
        assert_eq!(
            bridge_2.active_agent_count().await.expect("b2 count after"),
            PER_BRIDGE,
            "bridge 2 agents must survive bridge 1's full teardown"
        );

        // Bridge 2 agents remain fully functional after bridge 1 is drained.
        for (i, a) in agents_2.iter().enumerate() {
            let text = a
                .chat_text(format!("b2 still alive {i}"))
                .await
                .unwrap_or_else(|e| panic!("bridge 2 chat {i} after bridge 1 teardown: {e}"));
            assert!(
                text.contains("mock:multi-bridge"),
                "bridge 2 agent {i}: {text}"
            );
        }

        futures::future::join_all(
            agents_2
                .into_iter()
                .map(|a| async move { a.shutdown().await }),
        )
        .await
        .into_iter()
        .enumerate()
        .for_each(|(i, r)| r.unwrap_or_else(|e| panic!("bridge 2 shutdown {i}: {e}")));

        assert!(
            wait_for_zero_agents(&bridge_2).await,
            "bridge 2 must drain to zero agents after teardown"
        );
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

        // Warm up the one-time cold SDK import so the per-agent timings below
        // exclude the first import (see `warm_up_python_sdk`).
        warm_up_python_sdk(&url).await;

        for i in 0..COUNT {
            let start = std::time::Instant::now();
            let agent = BRIDGE
                .agent(agent_config(&url, &format!("timing-{i}")))
                .await
                .unwrap_or_else(|e| panic!("agent {i} creation failed: {e}"));
            let elapsed = start.elapsed();
            eprintln!("Agent {i} creation: {elapsed:.1?}");

            // Each agent creation should complete quickly. If the GIL is locked
            // by a prior agent's init, this will stall.
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
        result.expect_err("Chat after shutdown MUST return Err");
    });
}

// =============================================================================
// 13. Concurrent create + simultaneous shutdown storm — no stall
// =============================================================================

/// Create many agents concurrently, chat with all of them concurrently, then
/// shut them **all** down at the same time. The entire lifecycle is guarded by
/// a consumer-level timeout: if the GIL or the runtime thread stalls at any
/// point, the timeout fires and the test fails instead of hanging forever.
#[test]
fn concurrent_create_and_shutdown_storm_no_stall() {
    let rt = multi_thread_rt();

    rt.block_on(async {
        const N: usize = 8;
        let server = MockServer::start("storm").await;
        let url = server.base_url();

        let lifecycle = async {
            // Create all agents concurrently.
            let agents = futures::future::join_all((0..N).map(|i| {
                let url = url.clone();
                let name = format!("storm-{i}");
                async move { BRIDGE.agent(agent_config(&url, &name)).await }
            }))
            .await
            .into_iter()
            .enumerate()
            .map(|(i, res)| res.unwrap_or_else(|e| panic!("storm agent {i} creation: {e}")))
            .collect::<Vec<_>>();

            // Chat with all of them concurrently.
            let chats =
                futures::future::join_all(agents.iter().map(|a| a.chat_text("storm ping"))).await;
            for (i, res) in chats.into_iter().enumerate() {
                let text = res.unwrap_or_else(|e| panic!("storm agent {i} chat: {e}"));
                assert!(text.contains("mock:storm"), "storm agent {i} got: {text}");
            }

            // Shut every agent down simultaneously.
            let shutdowns = futures::future::join_all(
                agents
                    .into_iter()
                    .map(|a| async move { a.shutdown().await }),
            )
            .await;
            for (i, res) in shutdowns.into_iter().enumerate() {
                res.unwrap_or_else(|e| panic!("storm agent {i} shutdown: {e}"));
            }
        };

        tokio::time::timeout(std::time::Duration::from_mins(1), lifecycle)
            .await
            .expect("concurrent create/chat/shutdown storm stalled — possible GIL deadlock");

        assert_eq!(
            server.post_count(),
            N,
            "Expected exactly {N} POST requests from storm test"
        );
    });
}
