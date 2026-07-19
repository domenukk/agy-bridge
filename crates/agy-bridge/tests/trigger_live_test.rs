/// Tests for the trigger data model and SDK integration surface.
///
/// Full end-to-end trigger tests (every, `on_file_change`) require a persistent
/// Python asyncio event loop for the SDK's `TriggerRunner` to run background
/// tasks. Our architecture bridges Python coroutines transiently via
/// `pyo3_async_runtimes`, so `asyncio.create_task()` in the `TriggerRunner` has
/// no event loop to tick on between bridged calls.
///
/// These tests validate:
/// 1. `TriggerEntry`/`TriggerConfig` serialization roundtrips correctly
/// 2. The triggers field is accepted by the SDK's `LocalAgentConfig`
/// 3. Agent creation with triggers succeeds (SDK parses them)
use agy_bridge::prelude::*;

mod common;

#[test]
fn trigger_entry_serialization_roundtrip() {
    let entry = agy_bridge::triggers::TriggerEntry::new(
        "test_every",
        agy_bridge::triggers::TriggerConfig::try_every(std::time::Duration::from_secs(5)).unwrap(),
        "ping from every",
    );
    let json = serde_json::to_string(&entry).expect("serialize");
    let parsed: agy_bridge::triggers::TriggerEntry =
        serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.name, "test_every");
    assert_eq!(parsed.message_template, "ping from every");
}

#[test]
fn trigger_config_every_validates() {
    let config =
        agy_bridge::triggers::TriggerConfig::try_every(std::time::Duration::from_secs(10)).unwrap();
    let json = serde_json::to_value(&config).expect("to_value");
    let every = json.get("Every").expect("should have Every variant");
    // interval is f64 in the struct, so serde produces 10.0
    let interval = every["interval"]
        .as_f64()
        .expect("interval should be numeric");
    assert!(
        (interval - 10.0).abs() < f64::EPSILON,
        "Expected interval 10, got {interval}"
    );
}

#[test]
fn trigger_config_on_file_change_validates() {
    let config = agy_bridge::triggers::TriggerConfig::try_on_file_change("/tmp/watch").unwrap();
    let json = serde_json::to_value(&config).expect("to_value");
    let fc = json
        .get("OnFileChange")
        .expect("should have OnFileChange variant");
    assert_eq!(fc["path"], "/tmp/watch");
}

/// Verify that an agent can be created with triggers configured, even though
/// the SDK's `TriggerRunner` won't produce events in our bridged architecture.
#[test]
fn test_trigger_agent_creation() {
    common::run_live_test("test_trigger_agent_creation", || {
        let _key = common::api_key();
        let rt = common::test_runtime();

        rt.block_on(async {
            let bridge = common::create_bridge();
            let triggers = vec![agy_bridge::triggers::TriggerEntry::new(
                "test_every",
                agy_bridge::triggers::TriggerConfig::try_every(std::time::Duration::from_secs(10))
                    .unwrap(),
                "ping from every",
            )];

            let config = AgentConfig::builder()
                .system_instructions("You are a test agent. Reply with OK.")
                .triggers(triggers)
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .policies(vec![agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            // Agent creation with triggers should succeed.
            let agent = bridge.agent(config).await?;

            // Verify the agent still works for normal prompts.
            let result = agent.chat_text("Say hello").await;
            match result {
                Ok(text) => {
                    assert!(!text.is_empty(), "Expected non-empty response");
                }
                Err(e) => {
                    // Transient backend errors (workspace discovery, etc.) are
                    // acceptable — but only specific error types.
                    let err_str = e.to_string();
                    assert!(
                        err_str.contains("Backend")
                            || err_str.contains("timeout")
                            || err_str.contains("Timeout")
                            || err_str.contains("429")
                            || err_str.contains("workspace"),
                        "Unexpected error type from trigger agent test: {e}"
                    );
                    eprintln!("Trigger agent prompt returned expected error: {e}");
                }
            }

            agent.shutdown().await?;
            Ok(())
        })
    });
}
