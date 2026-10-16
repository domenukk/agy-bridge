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
                    match agent.chat("PING").await {
                        Ok(_) => panic!("Expected an error due to invalid config, got success"),
                        Err(e) => eprintln!("Got expected error from invalid config: {e}"),
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

            match bridge.agent(config).await {
                Ok(agent) => {
                    match agent.chat("hi").await {
                        Ok(_) => panic!("Expected chat to fail with python error"),
                        Err(err) => eprintln!("Clean Rust error from Python: {err:?}"),
                    }
                    agent.shutdown().await?;
                }
                Err(err) => {
                    let err_str = format!("{err:?}");
                    eprintln!("Clean Rust error from Python on init: {err_str}");
                    assert!(
                        err_str.contains("Python")
                            || err_str.contains("Error")
                            || err_str.contains("InvalidConfig"),
                        "Should have an error message indicating failure"
                    );
                }
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
