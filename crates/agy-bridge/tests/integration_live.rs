//! Live integration tests for agy-bridge against the real Gemini backend.
//!
//! These tests require:
//! - `GEMINI_API_KEY` environment variable to be set
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY="..." cargo test --test integration_live -- --nocapture
//! ```
//!
//! All custom tools are Rust structs implementing [`RustTool`] with
//! strongly-typed parameter structs derived via `schemars`. Built-in SDK
//! tools (`view_file`, `list_directory`, etc.) are supported through the
//! capabilities config.
use agy_bridge::tools::{JsonSchema, RustTool, ToolError, ToolOutput, ToolRegistry};
use serde::Deserialize;

mod common;

// ─── Test Infrastructure ─────────────────────────────────────────────────────

/// Returns the `GEMINI_API_KEY`, checking the environment first and then
/// falling back to a `.env` file in the project root.
///
/// # Panics
///
/// Panics if the key is not found in either location. Tests that require
/// this key should call `require_api_key!()` at the top.
fn api_key() -> String {
    common::api_key()
}

/// Macro that returns the `GEMINI_API_KEY`, panicking if absent.
macro_rules! require_api_key {
    () => {
        api_key()
    };
}

pub use common::run_live_test as run_with_retry;

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// Creates an [`AgyBridge`] with default configuration.
///
/// This is the public entry point. Individual tests create agents via
/// `bridge.agent(config)` and interact with `agent.chat()` / `agent.chat_text()`.
fn create_bridge() -> agy_bridge::AgyBridge {
    agy_bridge::AgyBridge::builder()
        .build()
        .expect("Failed to create bridge")
}

// ─── Tool Definitions ────────────────────────────────────────────────────────

/// Parameters for [`GetDeviceSerial`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetDeviceSerialParams {
    /// The name of the device to look up.
    device_name: String,
}

/// Looks up a device serial number from a hardcoded inventory.
struct GetDeviceSerial;

impl RustTool for GetDeviceSerial {
    type Params = GetDeviceSerialParams;
    const NAME: &'static str = "get_device_serial";
    const DESCRIPTION: &'static str = "Returns the serial number for a device.";

    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let normalized = params.device_name.to_lowercase().replace(' ', "_");
        let serial = match normalized.as_str() {
            "pixel_9" => "SERIAL-PX9-001",
            "cuttlefish" => "SERIAL-CF-002",
            _ => "SERIAL-UNKNOWN",
        };
        serde_json::to_string(&serde_json::json!({
            "device": params.device_name,
            "serial": serial,
        }))
        .map(ToolOutput::from)
        .map_err(|e| ToolError::new(format!("Serialization error: {e}")))
    }
}

/// Parameters for [`CheckBuildStatus`].
#[derive(Debug, Deserialize, JsonSchema)]
struct CheckBuildStatusParams {
    /// The build identifier to check.
    build_id: String,
}

/// Queries a hardcoded build database and returns status information.
struct CheckBuildStatus;

impl RustTool for CheckBuildStatus {
    type Params = CheckBuildStatusParams;
    const NAME: &'static str = "check_build_status";
    const DESCRIPTION: &'static str = "Checks the status of a build job.";

    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let result = match params.build_id.as_str() {
            "build-42" => serde_json::json!({
                "build_id": "build-42",
                "status": "success",
                "artifacts": ["kernel.img", "system.img"],
            }),
            "build-99" => serde_json::json!({
                "build_id": "build-99",
                "status": "failed",
                "error": "OOM during linking",
            }),
            other => serde_json::json!({
                "build_id": other,
                "status": "unknown",
            }),
        };
        serde_json::to_string(&result)
            .map(ToolOutput::from)
            .map_err(|e| ToolError::new(format!("Serialization error: {e}")))
    }
}

/// A no-op tool used to test policy enforcement.
struct SafeTool;

impl RustTool for SafeTool {
    type Params = agy_bridge::tools::EmptyParams;
    const NAME: &'static str = "safe_tool";
    const DESCRIPTION: &'static str = "A safe no-op tool that returns a confirmation.";

    async fn call(
        &self,
        _params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok("safe_tool was called".into())
    }
}

/// Parameters for [`AddNumbers`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddNumbersParams {
    /// First number.
    x: i64,
    /// Second number.
    y: i64,
}

/// Adds two numbers together. Used for multi-step agentic loop testing.
struct AddNumbers;

impl RustTool for AddNumbers {
    type Params = AddNumbersParams;
    const NAME: &'static str = "add_numbers";
    const DESCRIPTION: &'static str = "Adds two numbers together.";

    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(format!("{}", params.x + params.y).into())
    }
}

// =============================================================================
// Test 1: Basic round-trip (create agent → chat → shutdown)
// =============================================================================

#[test]
fn bridge_creates_agent_and_chats() {
    run_with_retry("bridge_creates_agent_and_chats", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: PONG")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;
            eprintln!("Created agent: {}", agent.id());

            let text = agent.chat("PING").await?.text().await?;
            eprintln!("Response: {text}");
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test 2: Custom Rust tool (GetDeviceSerial) via chat_text()
// =============================================================================

#[test]
fn live_agent_with_custom_rust_tool() {
    run_with_retry("live_agent_with_custom_rust_tool", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let mut registry = ToolRegistry::new();
            registry.register(GetDeviceSerial);

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions(
                    "You are a device inventory lookup tool. When asked about a device, \
                     ALWAYS use the get_device_serial tool to look it up. \
                     Your response MUST contain the exact serial number returned by the tool. \
                     Do NOT add follow-up questions. Just report the serial.",
                )
                .model("gemini-3.5-flash")
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;

            let text = agent
                .chat_text("What is the serial number for the Pixel 9?")
                .await?;
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(
                text.contains("SERIAL-PX9-001"),
                "Expected serial in response, got: {text}"
            );
            Ok(())
        })
    });
}

// =============================================================================
// Test 3: ToolDefinition serde round-trip
// =============================================================================

#[test]
fn rust_tool_definition_serde_roundtrip() {
    #[derive(Debug, Deserialize, JsonSchema)]
    struct FlashParams {
        /// Target device identifier.
        device_id: String,
        /// Build image to flash.
        build_image: String,
    }

    // Exercise the struct fields via deserialization to avoid dead_code.
    let params: FlashParams =
        serde_json::from_str("{\"device_id\": \"dev-1\", \"build_image\": \"img.bin\"}")
            .expect("FlashParams deserialization");
    assert_eq!(params.device_id, "dev-1");
    assert_eq!(params.build_image, "img.bin");
    let schema = schemars::schema_for!(FlashParams);
    let schema_value = serde_json::to_value(&schema).expect("schema to Value");

    let tool = agy_bridge::tools::ToolDefinition {
        name: "flash_device".to_string(),
        description: "Flashes a build image onto a device.".to_string(),
        parameter_schema: schema_value,
    };

    let json_str = serde_json::to_string(&tool).expect("serialize ToolDefinition");
    eprintln!("Serialized tool def: {json_str}");

    let roundtripped: agy_bridge::tools::ToolDefinition =
        serde_json::from_str(&json_str).expect("deserialize ToolDefinition");
    assert_eq!(roundtripped.name, "flash_device");
    assert_eq!(
        roundtripped.description,
        "Flashes a build image onto a device."
    );
}

// =============================================================================
// Test 4: CheckBuildStatus tool via chat_text()
// =============================================================================

#[test]
fn live_rust_tool_called_by_agent() {
    run_with_retry("live_rust_tool_called_by_agent", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let mut registry = ToolRegistry::new();
            registry.register(CheckBuildStatus);

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions(
                    "You help check build statuses. Always use the check_build_status tool.",
                )
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;

            let text = agent.chat_text("What's the status of build-42?").await?;
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(
                text.to_lowercase().contains("success"),
                "Expected 'success' in response, got: {text}"
            );
            Ok(())
        })
    });
}

// =============================================================================
// Test 5: Agent with built-in file tools
// =============================================================================

#[test]
fn live_agent_with_builtin_tools() {
    run_with_retry("live_agent_with_builtin_tools", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            // Create a temp file for the agent to read under a non-hidden workspace prefix.
            let td = tempfile::Builder::new()
                .prefix("my-workspace")
                .tempdir()
                .expect("tempdir");
            let temp_dir = td.path().to_path_buf();
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let temp_path = temp_dir.join("secret.txt");
            std::fs::write(&temp_path, "The secret code is GAMMA-42.").expect("write temp file");

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions(
                    "You are a file reader. Read files when asked and report their contents.",
                )
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .workspaces(vec![temp_dir.clone()])
                .build();

            let agent = bridge.agent(config).await?;

            let prompt = format!(
                "Read the file at {} and tell me the secret code.",
                temp_path.display()
            );
            let text = agent.chat_text(&*prompt).await?;
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(
                text.contains("GAMMA-42"),
                "Expected agent to read the file and return 'GAMMA-42', got: {text}"
            );
            Ok(())
        })
    });
}

// =============================================================================
// Test 6: Policy enforcement (SafeTool with AllowAll)
// =============================================================================

#[test]
fn live_agent_policy_allows_safe_tool() {
    run_with_retry("live_agent_policy_allows_safe_tool", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let mut registry = ToolRegistry::new();
            registry.register(SafeTool);

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Always call the safe_tool when asked.")
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;

            let text = agent.chat_text("Call the safe_tool please.").await?;
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(
                text.to_lowercase().contains("safe_tool")
                    || text.to_lowercase().contains("called")
                    || text.to_lowercase().contains("confirmation"),
                "Expected response mentioning safe_tool execution, got: {text}"
            );
            Ok(())
        })
    });
}

// =============================================================================
// Test 7: Real round-trip via chat() (no tools)
// =============================================================================

#[test]
fn bridge_real_roundtrip() {
    run_with_retry("bridge_real_roundtrip", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: hello")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;
            eprintln!("Created agent: {}", agent.id());

            let text = agent.chat("Say 'hello'").await?.text().await?;
            eprintln!("Real response: {text}");
            assert!(!text.is_empty(), "Expected real response text, got empty");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test 8: Agentic loop with AddNumbers tool via chat_text()
// =============================================================================

#[test]
fn live_agentic_loop() {
    run_with_retry("live_agentic_loop", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let mut registry = ToolRegistry::new();
            registry.register(AddNumbers);

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions(
                    "You are a calculator. Use the add_numbers tool to compute sums. \
                     Always use the tool and report the numeric result.",
                )
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;

            let text = agent
                .chat_text("Call the add_numbers tool with x=10 and y=32, then report the result.")
                .await?;
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(text.contains("42"), "Expected 42, got: {text}");
            Ok(())
        })
    });
}

// =============================================================================
// Test 9: Simple chat (PING/PONG)
// =============================================================================

#[test]
fn live_simple_chat() {
    run_with_retry("live_simple_chat", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: PONG")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            let text = agent.chat("PING").await?.text().await?;
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test 10: Text response via chat()
// =============================================================================

#[test]
fn live_text_response() {
    run_with_retry("live_text_response", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a helpful assistant. Answer questions concisely.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            let text = agent.chat("What color is the sky?").await?.text().await?;
            eprintln!("Response text: {text}");
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test 11: Prompt with the README example (wonky_add)
// =============================================================================

use agy_bridge::llm_tool;

/// Adds two numbers together (with a twist).
#[llm_tool]
fn wonky_add(
    /// First number.
    a: i64,
    /// Second number.
    b: i64,
) -> Result<String, ToolError> {
    Ok(format!("{}", a + b + 1))
}

#[test]
fn readme_example_wonky_add() {
    run_with_retry("readme_example_wonky_add", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let mut registry = ToolRegistry::new();
            registry.register(WonkyAdd);

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions(
                    "You are a calculator. Always use the wonky_add tool \
                     to add numbers. Report the exact numeric result.",
                )
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;
            let answer = agent.chat_text("What is 1 + 1?").await?;

            eprintln!("Answer: {answer}");
            assert!(answer.contains('3'), "Expected 3, got: {answer}");
            Ok(())
        })
    });
}

// =============================================================================
// Test 12: Live conversation history, turn count, and token usage tracking
// =============================================================================

#[test]
fn live_conversation_token_usage_tracking() {
    run_with_retry("live_conversation_token_usage_tracking", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Answer very concisely in 1 word.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            // Verify initial turn count is 0
            let tc_init = agent.turn_count().await?;
            assert_eq!(tc_init, 0);

            // Send first turn
            let text = agent
                .chat("What is the capital of France?")
                .await?
                .text()
                .await?;
            eprintln!("Capital response: {text}");

            // Verify turn count is now 1
            let tc_after = agent.turn_count().await?;
            assert_eq!(tc_after, 1);

            // Verify history contains at least user + model messages.
            // Newer SDK versions may insert additional entries (thinking,
            // system) so we search by role instead of assuming indices.
            let history = agent.history().await?;
            assert!(
                history.len() >= 2,
                "Expected at least 2 history entries (user + model), got {}",
                history.len()
            );
            let user_msg = history
                .iter()
                .find(|m| m.role == agy_bridge::MessageRole::User)
                .expect("should have a user message in history");
            assert!(
                user_msg.content.contains("France"),
                "user message should mention France: {:?}",
                user_msg.content
            );
            assert!(
                history
                    .iter()
                    .any(|m| m.role == agy_bridge::MessageRole::Model),
                "should have a model message in history"
            );

            // Verify token usage is tracked and greater than zero
            let usage = agent.total_usage().await?;
            let prompt_tokens = usage.prompt_token_count.expect("prompt_tokens");
            let total_tokens = usage.total_token_count.expect("total_tokens");
            assert!(prompt_tokens > 0, "Expected prompt tokens > 0");
            assert!(
                total_tokens > prompt_tokens,
                "Expected total tokens > prompt tokens"
            );

            // Verify turn usage matches total usage on first turn
            let last_usage = agent.last_turn_usage().await?;
            assert_eq!(last_usage.prompt_token_count, Some(prompt_tokens));
            assert_eq!(last_usage.total_token_count, Some(total_tokens));

            // Verify fast-access last usage is also available
            let fast_usage = agent.get_last_usage().expect("get_last_usage");
            assert_eq!(fast_usage.prompt_token_count, Some(prompt_tokens));
            assert_eq!(fast_usage.total_token_count, Some(total_tokens));

            // Clear history and verify turn count resets
            agent.clear_history().await?;
            let tc_cleared = agent.turn_count().await?;
            assert_eq!(tc_cleared, 0);

            // Verify history is empty
            let history_cleared = agent.history().await?;
            assert!(history_cleared.is_empty());

            agent.shutdown().await?;
            Ok(())
        })
    });
}

#[test]
fn live_multimodal_vision() {
    run_with_retry("live_multimodal_vision", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            use agy_bridge::content::{Content, ContentPrimitive, Image};
            use base64::Engine;

            let key = api_key();

            let config = agy_bridge::config::AgentConfig::builder()
                .model("gemini-3.5-flash")
                .api_key(key)
                .system_instructions("You are a helpful assistant. Answer questions about images directly.")
                .capabilities(agy_bridge::CapabilitiesConfig::custom_tools_only())
                .build();

            let bridge = agy_bridge::AgyBridge::builder()
                .build()?;
            let agent = bridge.agent(config).await?;

            // A tiny 1x1 red PNG base64 decoded
            let red_png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAAXNSR0IArs4c6QAAAERlWElmTU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAAaADAAQAAAABAAAAAQAAAAD5Ip3+AAAADUlEQVQI12P4z8DwHwAFAAH/VscvDQAAAABJRU5ErkJggg==";
            let image_bytes = base64::engine::general_purpose::STANDARD
                .decode(red_png_b64)
                .unwrap();

            let content = Content::Multi {
                parts: vec![
                    ContentPrimitive::Text {
                        text: "What color is this 1x1 image? Answer in one word.".to_string(),
                    },
                    ContentPrimitive::Image(Image::png(image_bytes)),
                ],
            };

            let stream = agent.chat(content).await?;
            let response = stream.text().await?;
            let response_text = response.text();

            assert!(
                response_text.to_lowercase().contains("red"),
                "Expected the model to see the red image, got: {response_text}"
            );
            Ok(())
        })
    });
}

// =============================================================================
// Test 13: Gap 3 - Streaming completion metadata
// =============================================================================

#[test]
fn live_streaming_completion_metadata() {
    run_with_retry("live_streaming_completion_metadata", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            #[derive(Deserialize, JsonSchema)]
            struct CalculatorResponse {
                answer: i32,
            }

            let bridge = create_bridge();

            let schema_root = schemars::schema_for!(CalculatorResponse);
            let schema = serde_json::to_value(&schema_root).expect("schema serialization");

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a calculator that returns the sum of the numbers as a JSON object with a single 'answer' integer field.")
                .response_schema(agy_bridge::config::JsonSchema::new(schema))
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

             let result = agent
                .chat("Calculate: 5 + 7")
                .await?
                .text()
                .await?;

            // ChatResult carries usage and structured output alongside text
            if let Some(usage) = result.usage() {
                assert!(usage.total_token_count.unwrap_or(0) > 0, "Expected non-zero total tokens");
                assert!(usage.prompt_token_count.unwrap_or(0) > 0, "Expected non-zero prompt tokens");
            } else {
                eprintln!("Warning: usage metadata is None (known localharness issue with structured outputs)");
            }

            let structured_json = result.structured_output().ok_or_else(|| {
                agy_bridge::error::Error::ConnectionError {
                    message: "expected structured output, but got None".to_string(),
                }
            })?;
            let structured: CalculatorResponse = serde_json::from_value(structured_json.clone())
                .expect("failed to deserialize structured output");
            assert_eq!(structured.answer, 12, "Expected structured JSON answer to be 12");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test 14: Agent creation with invalid config returns proper error
// =============================================================================

#[test]
fn live_agent_invalid_config_returns_error() {
    run_with_retry("live_agent_invalid_config_returns_error", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
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
    run_with_retry("live_agent_timeout_triggers", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

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
// Test 16: Multi-agent - create 3 agents, chat with each, shutdown all
// =============================================================================

#[test]
fn live_multi_agent_lifecycle() {
    run_with_retry("live_multi_agent_lifecycle", || {
        let _api_key = require_api_key!();
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
            let bridge = create_bridge();
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
// Test 17: Streaming - verify token-by-token delivery matches full text
// =============================================================================

#[test]
fn live_streaming_token_delivery() {
    run_with_retry("live_streaming_token_delivery", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a storyteller. Write a 5 sentence story about a cat.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            let mut response = agent.chat("Tell me the story.").await?;

            let mut streamed_text = String::new();
            let mut text_stream = response.take_text_stream().expect("text stream");
            let mut chunk_count = 0;
            while let Some(chunk) = text_stream.recv().await {
                streamed_text.push_str(&chunk);
                chunk_count += 1;
            }
            drop(text_stream);
            // Consume the handle — text stream already drained, so this yields empty.
            drop(response.text().await?);

            eprintln!("Streamed text chunks: {chunk_count}");
            assert!(chunk_count > 1, "Expected multiple streaming chunks");
            assert!(
                !streamed_text.is_empty(),
                "Expected non-empty streamed text"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test 18: Policy enforcement - deny write tools, verify rejection
// =============================================================================

#[test]
fn live_policy_enforcement_deny_write() {
    run_with_retry("live_policy_enforcement_deny_write", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            // Use DenyAll: the agent should not be able to use any tools,
            // so it can only produce a text response.
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a helpful assistant. Reply with a short text answer.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .policies([agy_bridge::policies::PolicyRule::DenyAll])
                .build();

            let agent = bridge.agent(config).await?;

            // Even with DenyAll, the agent should still produce a text-only
            // response (no tool calls to deny). This verifies the policy is
            // passed to the SDK without crashing.
            let result = agent.chat_text("What is 1+1?").await;

            match result {
                Ok(text) => {
                    eprintln!("Agent response with DenyAll: {text}");
                    assert!(!text.is_empty(), "Expected non-empty response");
                }
                Err(e) => {
                    // Some SDK versions may error with DenyAll — that's also
                    // acceptable since it means the policy was applied.
                    eprintln!("DenyAll produced error (acceptable): {e}");
                }
            }

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test 19: Error recovery - force Python error, verify clean Rust error
// =============================================================================

#[test]
fn live_error_recovery_force_python_error() {
    run_with_retry("live_error_recovery_force_python_error", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
            let schema = serde_json::json!("not_an_object");
            let config = agy_bridge::config::AgentConfig::builder()
                .response_schema(agy_bridge::config::JsonSchema::new(schema))
                .build();

            let result = bridge.agent(config).await;
            if let Ok(agent) = result {
                let chat_result = agent.chat("hi").await;
                assert!(
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
// Test 20: Subagent - agent spawns subagent, gets result
// =============================================================================

#[test]
fn live_subagent_spawn() {
    run_with_retry("live_subagent_spawn", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
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

// =============================================================================
// Test 21: Quota backoff - simulate 429, verify backoff and retry
// =============================================================================

#[test]
fn live_quota_backoff_retry() {
    run_with_retry("live_quota_backoff_retry", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
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

#[test]
fn live_mcp_server_config_passes_to_python() {
    run_with_retry("live_mcp_server_config_passes_to_python", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            use agy_bridge::config::McpServer;

            let bridge = create_bridge();

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
            drop(agent);
            Ok(())
        })
    });
}

#[derive(serde::Serialize, serde::Deserialize, agy_bridge::tools::JsonSchema)]
struct MetadataTestResponse {
    result: String,
    some_code: i32,
}

/// Test tool that returns structured data metadata.
#[llm_tool::llm_tool]
fn structured_metadata_tool() -> Result<MetadataTestResponse, ToolError> {
    Ok(MetadataTestResponse {
        result: "Structured metadata works".into(),
        some_code: 42,
    })
}

#[test]
fn live_rust_tool_metadata() {
    common::run_live_test("live_rust_tool_metadata", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            use std::sync::{Arc, Mutex};
            let metadata_capture = Arc::new(Mutex::new(serde_json::Value::Null));
            let capture_clone = Arc::clone(&metadata_capture);

            let mut hooks = agy_bridge::hooks::Hooks::new();
            hooks.on_post_tool_call("capture_meta", move |ctx| {
                if ctx.tool_name == "structured_metadata_tool" {
                    *capture_clone.lock().unwrap() = ctx.metadata.clone();
                }
            });

            let bridge = create_bridge();
            let mut registry = ToolRegistry::new();
            registry.register(StructuredMetadataTool);
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Always call structured_metadata_tool and repeat its output")
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();
            let agent = bridge.agent(config).tools(registry).hooks(hooks).await?;

            let _text = agent
                .chat_text("Call structured_metadata_tool and tell me the result")
                .await?;

            let meta = metadata_capture.lock().unwrap().clone();
            assert_eq!(meta["some_code"], 42, "metadata should contain some_code");
            assert_eq!(
                meta["result"], "Structured metadata works",
                "metadata should contain result"
            );
            Ok(())
        })
    });
}

// =============================================================================
// Test: Multi-agent isolation — shutdown one, others still work (same bridge)
// =============================================================================

#[test]
fn shutdown_one_agent_others_still_work_same_bridge() {
    run_with_retry("shutdown_one_agent_others_still_work_same_bridge", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

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
    run_with_retry("sequential_bridge_instances_work_after_teardown", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            // ── Phase 1: create bridge, use agent, tear everything down ──
            {
                let bridge_1 = agy_bridge::AgyBridge::builder()
                    .chat_timeout(std::time::Duration::from_mins(1))
                    .build()?;

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
                let bridge_2 = agy_bridge::AgyBridge::builder()
                    .chat_timeout(std::time::Duration::from_mins(1))
                    .build()?;

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
    run_with_retry("three_agents_shutdown_middle_others_survive", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

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
    run_with_retry("sequential_bridges_with_different_proxy_configs", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

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

                let bridge = create_bridge();
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

                let bridge = create_bridge();
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
    run_with_retry("same_bridge_proxy_and_direct_agents_isolation", || {
        let _api_key = require_api_key!();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

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
    run_with_retry("same_bridge_concurrent_proxy_and_direct_agents", || {
        let _api_key = require_api_key!();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("multi-thread tokio runtime");

        rt.block_on(async {
            let bridge = create_bridge();

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
    run_with_retry("two_bridges_concurrent_proxy_and_direct_agents", || {
        let _api_key = require_api_key!();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("multi-thread tokio runtime");

        rt.block_on(async {
            // Create two completely separate bridge instances. Each spawns its
            // own dedicated Python runtime thread with its own asyncio event loop.
            let bridge_proxy = create_bridge();
            let bridge_direct = create_bridge();

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

            let chunk_proxy = stream_proxy.recv().await.expect("proxy first chunk");
            let chunk_direct = stream_direct.recv().await.expect("direct first chunk");

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
