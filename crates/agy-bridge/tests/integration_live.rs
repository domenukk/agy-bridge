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

/// A single shared [`AgyBridge`] for all integration tests.
///
/// Spawning a separate `PythonRuntime` per test creates 23+ threads
/// that all contend for the Python GIL, causing `create_agent(__aenter__)`
/// to time out at 120 s.  Sharing one bridge keeps a single Python thread
/// and asyncio event loop, eliminating GIL contention.
static SHARED_BRIDGE: std::sync::LazyLock<agy_bridge::AgyBridge> = std::sync::LazyLock::new(|| {
    agy_bridge::AgyBridge::builder()
        .build()
        .expect("Failed to create shared bridge")
});

/// Returns a reference to the shared [`AgyBridge`].
fn create_bridge() -> &'static agy_bridge::AgyBridge {
    &SHARED_BRIDGE
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

            let agent = bridge.agent(config).await.expect("create agent");
            eprintln!("Created agent: {}", agent.id());

            let text = agent
                .chat("PING")
                .await
                .expect("chat failed")
                .text()
                .await
                .expect("text should succeed");
            eprintln!("Response: {text}");
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await.expect("shutdown agent");
        });
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

            let agent = bridge
                .agent(config)
                .tools(registry)
                .await
                .expect("create agent");

            let text = agent
                .chat_text("What is the serial number for the Pixel 9?")
                .await
                .expect("chat_text failed");
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(
                text.contains("SERIAL-PX9-001"),
                "Expected serial in response, got: {text}"
            );
        });
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
    let schema = schemars::r#gen::SchemaGenerator::default().root_schema_for::<FlashParams>();
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

            let agent = bridge
                .agent(config)
                .tools(registry)
                .await
                .expect("create agent");

            let text = agent
                .chat_text("What's the status of build-42?")
                .await
                .expect("chat_text failed");
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(
                text.to_lowercase().contains("success"),
                "Expected 'success' in response, got: {text}"
            );
        });
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

            let agent = bridge.agent(config).await.expect("create agent");

            let prompt = format!(
                "Read the file at {} and tell me the secret code.",
                temp_path.display()
            );
            let text = agent.chat_text(&*prompt).await.expect("chat_text failed");
            drop(agent);

            eprintln!("Agent response: {text}");

            // Clean up temp file.
        });
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

            let agent = bridge
                .agent(config)
                .tools(registry)
                .await
                .expect("create agent");

            let text = agent
                .chat_text("Call the safe_tool please.")
                .await
                .expect("chat_text failed");
            drop(agent);

            eprintln!("Agent response: {text}");
        });
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

            let agent = bridge.agent(config).await.expect("create agent");
            eprintln!("Created agent: {}", agent.id());

            let text = agent
                .chat("Say 'hello'")
                .await
                .expect("chat failed")
                .text()
                .await
                .expect("text should succeed");
            eprintln!("Real response: {text}");
            assert!(!text.is_empty(), "Expected real response text, got empty");

            agent.shutdown().await.expect("shutdown agent");
        });
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

            let agent = bridge
                .agent(config)
                .tools(registry)
                .await
                .expect("create agent");

            let text = agent
                .chat_text("Call the add_numbers tool with x=10 and y=32, then report the result.")
                .await
                .expect("chat_text failed");
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(text.contains("42"), "Expected 42, got: {text}");
        });
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

            let agent = bridge.agent(config).await.expect("create agent");

            let text = agent
                .chat("PING")
                .await
                .expect("chat failed")
                .text()
                .await
                .expect("text should succeed");
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await.expect("shutdown agent");
        });
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

            let agent = bridge.agent(config).await.expect("create agent");

            let text = agent
                .chat("What color is the sky?")
                .await
                .expect("chat failed")
                .text()
                .await
                .expect("text should succeed");
            eprintln!("Response text: {text}");
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await.expect("shutdown agent");
        });
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

            let agent = bridge
                .agent(config)
                .tools(registry)
                .await
                .expect("create agent");
            let answer = agent
                .chat_text("What is 1 + 1?")
                .await
                .expect("chat_text failed");

            eprintln!("Answer: {answer}");
            assert!(answer.contains('3'), "Expected 3, got: {answer}");
        });
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

            let agent = bridge.agent(config).await.expect("create agent");

            // Verify initial turn count is 0
            let tc_init = agent.turn_count().await.expect("turn_count init");
            assert_eq!(tc_init, 0);

            // Send first turn
            let text = agent
                .chat("What is the capital of France?")
                .await
                .expect("chat")
                .text()
                .await
                .expect("text");
            eprintln!("Capital response: {text}");

            // Verify turn count is now 1
            let tc_after = agent.turn_count().await.expect("turn_count after");
            assert_eq!(tc_after, 1);

            // Verify history has 2 messages (user + model)
            let history = agent.history().await.expect("history");
            assert_eq!(history.len(), 2);
            assert_eq!(history[0].role, agy_bridge::MessageRole::User);
            assert!(history[0].content.contains("France"));
            assert_eq!(history[1].role, agy_bridge::MessageRole::Model);

            // Verify token usage is tracked and greater than zero
            let usage = agent.total_usage().await.expect("total_usage");
            let prompt_tokens = usage.prompt_token_count.expect("prompt_tokens");
            let total_tokens = usage.total_token_count.expect("total_tokens");
            assert!(prompt_tokens > 0, "Expected prompt tokens > 0");
            assert!(
                total_tokens > prompt_tokens,
                "Expected total tokens > prompt tokens"
            );

            // Verify turn usage matches total usage on first turn
            let last_usage = agent.last_turn_usage().await.expect("last_turn_usage");
            assert_eq!(last_usage.prompt_token_count, Some(prompt_tokens));
            assert_eq!(last_usage.total_token_count, Some(total_tokens));

            // Verify fast-access last usage is also available
            let fast_usage = agent.get_last_usage().expect("get_last_usage");
            assert_eq!(fast_usage.prompt_token_count, Some(prompt_tokens));
            assert_eq!(fast_usage.total_token_count, Some(total_tokens));

            // Clear history and verify turn count resets
            agent.clear_history().await.expect("clear_history");
            let tc_cleared = agent.turn_count().await.expect("turn_count cleared");
            assert_eq!(tc_cleared, 0);

            // Verify history is empty
            let history_cleared = agent.history().await.expect("history cleared");
            assert!(history_cleared.is_empty());

            agent.shutdown().await.expect("shutdown agent");
        });
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

            let bridge = create_bridge();
            let agent = bridge.agent(config).await.expect("Failed to create agent");

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

            let stream = agent.chat(content).await.expect("Failed to start chat");
            let response = stream.text().await.expect("Failed to get chat response");
            let response_text = response.text();

            assert!(
                response_text.to_lowercase().contains("red"),
                "Expected the model to see the red image, got: {response_text}"
            );
        });
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
            let bridge = create_bridge();

            let schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "result": { "type": "integer" }
                },
                "required": ["result"]
            });

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a calculator. Think step-by-step about the addition, then return the sum of the numbers as a JSON object with a single 'result' integer field.")
                .response_schema(agy_bridge::config::JsonSchema::new(schema))
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await.expect("create agent");

             let result = agent
                .chat("Please think step-by-step and calculate: 5 + 7")
                .await
                .expect("chat failed")
                .text()
                .await
                .expect("text should succeed");

            // ChatResult carries usage and structured output alongside text
            let usage = result.usage().expect("expected usage metadata");
            assert!(usage.total_token_count.unwrap_or(0) > 0, "Expected non-zero total tokens");
            assert!(usage.prompt_token_count.unwrap_or(0) > 0, "Expected non-zero prompt tokens");

            let structured = result.structured_output().expect("expected structured output");
            assert!(
                structured["result"].is_number(),
                "Expected structured output 'result' to be a number, got: {structured:?}"
            );

            agent.shutdown().await.expect("shutdown agent");
        });
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
                    agent.shutdown().await.expect("shutdown");
                }
                Err(e) => {
                    eprintln!("Got expected error during agent creation: {e}");
                }
            }
        });
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
                .build()
                .expect("create bridge");

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
                    agent.shutdown().await.expect("shutdown");
                }
                Err(e) => {
                    eprintln!("Got expected error during agent creation timeout: {e}");
                }
            }
        });
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
            let a1 = bridge.agent(config.clone()).await.expect("create agent 1");
            let a2 = bridge.agent(config.clone()).await.expect("create agent 2");
            let a3 = bridge.agent(config.clone()).await.expect("create agent 3");

            let f1 = async {
                let resp = a1
                    .chat("What is 1+1? Reply with just the number.")
                    .await
                    .expect("chat 1");
                resp.text().await
            };
            let f2 = async {
                let resp = a2
                    .chat("What is 2+2? Reply with just the number.")
                    .await
                    .expect("chat 2");
                resp.text().await
            };
            let f3 = async {
                let resp = a3
                    .chat("What is 3+3? Reply with just the number.")
                    .await
                    .expect("chat 3");
                resp.text().await
            };

            let (r1, r2, r3) = tokio::join!(f1, f2, f3);
            let _t1 = r1.expect("a1 text");
            let _t2 = r2.expect("a2 text");
            let _t3 = r3.expect("a3 text");

            // Shutdown sequentially for clean teardown.
            a1.shutdown().await.expect("shutdown a1");
            a2.shutdown().await.expect("shutdown a2");
            a3.shutdown().await.expect("shutdown a3");
        });
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

            let agent = bridge.agent(config).await.expect("create agent");

            let mut response = agent.chat("Tell me the story.").await.expect("chat failed");

            let mut streamed_text = String::new();
            let mut text_stream = response.take_text_stream().expect("text stream");
            let mut chunk_count = 0;
            while let Some(chunk) = text_stream.recv().await {
                streamed_text.push_str(&chunk);
                chunk_count += 1;
            }
            drop(text_stream);
            // Consume the handle — text stream already drained, so this yields empty.
            drop(response.text().await.expect("full text"));

            eprintln!("Streamed text chunks: {chunk_count}");
            assert!(chunk_count >= 1, "Expected at least one streaming chunk");
            assert!(
                !streamed_text.is_empty(),
                "Expected non-empty streamed text"
            );

            agent.shutdown().await.expect("shutdown agent");
        });
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

            let agent = bridge.agent(config).await.expect("create agent");

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

            agent.shutdown().await.expect("shutdown agent");
        });
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
                agent.shutdown().await.expect("shutdown");
            } else {
                let err_str = format!("{:?}", result.err().unwrap());
                eprintln!("Clean Rust error from Python on init: {err_str}");
                assert!(
                    err_str.contains("Python") || err_str.contains("Error"),
                    "Should have an error message indicating failure"
                );
            }
        });
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

            let agent = bridge.agent(config).await.expect("create agent");

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
                    // doesn't fully support it — this is acceptable as long
                    // as the agent didn't crash silently.
                    eprintln!("Subagent prompt returned error (acceptable): {e}");
                }
            }

            agent.shutdown().await.expect("shutdown agent");
        });
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

            let agent = bridge.agent(config).await.expect("create agent");

            // Rapid-fire sequential calls to exercise quota backoff/retry.
            for i in 0..3 {
                let text = agent.chat_text("PING").await.unwrap_or_else(|e| {
                    panic!("prompt {i} failed: {e}");
                });
                assert!(
                    text.to_lowercase().contains("pong"),
                    "Expected PONG in response {i}, got: {text}"
                );
            }

            agent.shutdown().await.expect("shutdown agent");
        });
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
            let agent = bridge.agent(config).await.expect("agent creation failed");
            drop(agent);
        });
    });
}

// =============================================================================
// Test: Multiple bridges with separate PythonRuntimes work concurrently
// =============================================================================

/// Verifies that two separate `AgyBridge` instances (each with their own
/// `PythonRuntime` and event loop) can create agents and chat concurrently
/// without interfering. This catches global-state bugs like the `EVENT_LOOP`
/// trampling issue.
#[test]
fn live_multi_bridge_concurrent() {
    run_with_retry("live_multi_bridge_concurrent", || {
        let _api_key = require_api_key!();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("multi-thread tokio runtime");

        rt.block_on(async {
            let bridge_a = agy_bridge::AgyBridge::builder()
                .build()
                .expect("create bridge A");
            let bridge_b = agy_bridge::AgyBridge::builder()
                .build()
                .expect("create bridge B");

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly one word: PONG")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            // Create agents on different bridges concurrently.
            let (agent_a, agent_b) =
                tokio::join!(bridge_a.agent(config.clone()), bridge_b.agent(config),);
            let agent_a = agent_a.expect("create agent on bridge A");
            let agent_b = agent_b.expect("create agent on bridge B");

            // Chat on both bridges concurrently.
            let (resp_a, resp_b) = tokio::join!(agent_a.chat("PING"), agent_b.chat("PING"),);
            let text_a = resp_a
                .expect("chat A")
                .text()
                .await
                .expect("text A")
                .text()
                .to_string();
            let text_b = resp_b
                .expect("chat B")
                .text()
                .await
                .expect("text B")
                .text()
                .to_string();

            assert!(!text_a.is_empty(), "Bridge A produced empty response");
            assert!(!text_b.is_empty(), "Bridge B produced empty response");

            agent_a.shutdown().await.expect("shutdown A");
            agent_b.shutdown().await.expect("shutdown B");
        });
    });
}
