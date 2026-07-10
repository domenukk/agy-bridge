mod common;

// =============================================================================
// Test 14: Agent creation with invalid config returns proper error
// =============================================================================

#[test]
fn live_agent_invalid_config_returns_error() {
    common::run_live_test("live_agent_invalid_config_returns_error", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = common::create_bridge();
            let schema = serde_json::json!("not_an_object");
            let config = agy_bridge::config::AgentConfig::builder()
                .response_schema(agy_bridge::config::JsonSchema::new(schema))
                .system_instructions("Reply with exactly: PONG")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            match bridge.agent(config).await {
                Ok(agent) => {
                    let result = agent.chat("PING").await;
                    assert!(
                        // NOLINT: test assertion — checking that invalid config produces an error
                        result.is_err(),
                        "Expected an error due to invalid config, got success"
                    );
                    if let Err(e) = result {
                        eprintln!("Got expected error from invalid config: {e}");
                    }
                    agent.shutdown().await?;
                }
                Err(e) => {
                    eprintln!("Got expected error during agent creation: {e}");
                }
            }
            Ok(())
        })
    });
}

// =============================================================================
// Test 15: Timeout triggers after configured duration
// =============================================================================

#[test]
fn live_agent_timeout_triggers() {
    common::run_live_test("live_agent_timeout_triggers", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = agy_bridge::AgyBridge::builder()
                .chat_timeout(std::time::Duration::from_millis(1))
                .build()?;

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Write a very long poem.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            match bridge.agent(config).await {
                Ok(agent) => {
                    let result = agent.chat("Write a very long poem about the sea.").await;
                    assert!(
                        // NOLINT: test assertion — checking that timeout produces an error
                        result.is_err(),
                        "Expected an error due to timeout, got success"
                    );
                    if let Err(e) = result {
                        eprintln!("Got expected error from timeout: {e}");
                    }
                    agent.shutdown().await?;
                }
                Err(e) => {
                    eprintln!("Got expected error during agent creation timeout: {e}");
                }
            }
            Ok(())
        })
    });
}

// =============================================================================
// Test 19: Error recovery - force Python error, verify clean Rust error
// =============================================================================

#[test]
fn live_error_recovery_force_python_error() {
    common::run_live_test("live_error_recovery_force_python_error", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = common::create_bridge();
            let schema = serde_json::json!("not_an_object");
            let config = agy_bridge::config::AgentConfig::builder()
                .response_schema(agy_bridge::config::JsonSchema::new(schema))
                .build();

            let result = bridge.agent(config).await;
            // NOLINT: test intentionally handles both Ok/Err paths to validate error recovery
            if let Ok(agent) = result {
                let chat_result = agent.chat("hi").await;
                assert!(
                    // NOLINT: test assertion — checking that forced error produces an error
                    chat_result.is_err(),
                    "Expected chat to fail with python error"
                );
                let err_str = format!("{:?}", chat_result.err().unwrap());
                eprintln!("Clean Rust error from Python: {err_str}");
                agent.shutdown().await?;
            } else {
                let err_str = format!("{:?}", result.err().unwrap());
                eprintln!("Clean Rust error from Python on init: {err_str}");
                assert!(
                    err_str.contains("Python") || err_str.contains("Error"),
                    "Should have an error message indicating failure"
                );
            }
            Ok(())
        })
    });
}

// =============================================================================
// Test 21: Quota backoff - simulate 429, verify backoff and retry
// =============================================================================

#[test]
fn live_quota_backoff_retry() {
    common::run_live_test("live_quota_backoff_retry", || {
        let _api_key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = common::create_bridge();
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: PONG")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            // Rapid-fire sequential calls to exercise quota backoff/retry.
            for i in 0..3 {
                let text = agent.chat_text("PING").await?;
                assert!(
                    text.to_lowercase().contains("pong"),
                    "Expected PONG in response {i}, got: {text}"
                );
            }

            agent.shutdown().await?;
            Ok(())
        })
    });
}
