//! Custom Rust tool tests exercised via live Gemini API.
//!
//! Tests for tool registration, serde round-trips, agentic loops, and
//! tool metadata extraction.
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY="..." cargo test --test tools_live_test -- --nocapture
//! ```

use agy_bridge::tools::{JsonSchema, RustTool, ToolError, ToolOutput, ToolRegistry};
use serde::Deserialize;

mod common;

use common::{api_key, create_bridge, run_live_test, test_runtime};

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

    // NOLINT: forward-compat with future clippy::unused_async_trait_impl lint
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
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

    // NOLINT: forward-compat with future clippy::unused_async_trait_impl lint
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
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

    // NOLINT: forward-compat with future clippy::unused_async_trait_impl lint
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(format!("{}", params.x + params.y).into())
    }
}

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

#[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
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

// =============================================================================
// Test: Custom Rust tool (GetDeviceSerial) via chat_text()
// =============================================================================

#[test]
fn live_agent_with_custom_rust_tool() {
    run_live_test("live_agent_with_custom_rust_tool", || {
        let _api_key = api_key();
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
            agent.shutdown().await?;

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
// Test: ToolDefinition serde round-trip
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
// Test: CheckBuildStatus tool via chat_text()
// =============================================================================

#[test]
fn live_rust_tool_called_by_agent() {
    run_live_test("live_rust_tool_called_by_agent", || {
        let _api_key = api_key();
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

            eprintln!("Agent response: {text}");
            assert!(
                text.to_lowercase().contains("success"),
                "Expected 'success' in response, got: {text}"
            );
            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Agentic loop with AddNumbers tool via chat_text()
// =============================================================================

#[test]
fn live_agentic_loop() {
    run_live_test("live_agentic_loop", || {
        let _api_key = api_key();
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
            agent.shutdown().await?;

            eprintln!("Agent response: {text}");
            assert!(text.contains("42"), "Expected 42, got: {text}");
            Ok(())
        })
    });
}

// =============================================================================
// Test: README example (wonky_add)
// =============================================================================

#[test]
fn readme_example_wonky_add() {
    run_live_test("readme_example_wonky_add", || {
        let _api_key = api_key();
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
// Test: Rust tool metadata
// =============================================================================

#[test]
fn live_rust_tool_metadata() {
    run_live_test("live_rust_tool_metadata", || {
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
// Test: custom tool observes the agent's conversation_id via ToolContext
// =============================================================================

/// A tool that records the `conversation_id` present in its [`ToolContext`],
/// proving the bridge threads `AgentConfig::conversation_id` all the way into
/// custom Rust tool dispatch.
struct WhoAmI {
    /// Captures the observed conversation ID as a side effect, so the assertion
    /// does not depend on the model echoing the value back in prose.
    captured: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl RustTool for WhoAmI {
    type Params = agy_bridge::tools::EmptyParams;
    const NAME: &'static str = "who_am_i";
    const DESCRIPTION: &'static str = "Returns the caller's conversation identifier.";

    // NOLINT: forward-compat with future clippy::unused_async_trait_impl lint
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn call(
        &self,
        _params: Self::Params,
        ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let observed = ctx.conversation_id().map(str::to_owned);
        match self.captured.lock() {
            Ok(mut slot) => slot.clone_from(&observed),
            // Log rather than swallow: the assertion will fail loudly anyway,
            // but a poisoned mutex should never be silently ignored.
            Err(e) => eprintln!("who_am_i: captured mutex poisoned: {e}"),
        }
        Ok(observed.unwrap_or_else(|| "unknown".to_owned()).into())
    }
}

#[test]
fn live_custom_tool_observes_conversation_id() {
    run_live_test("live_custom_tool_observes_conversation_id", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let captured = std::sync::Arc::new(std::sync::Mutex::new(None));

            let bridge = create_bridge();
            let mut registry = ToolRegistry::new();
            registry.register(WhoAmI {
                captured: std::sync::Arc::clone(&captured),
            });

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions(
                    "When asked who you are or for your conversation id, ALWAYS call the \
                     who_am_i tool and report exactly what it returns.",
                )
                .conversation_id("conv-live-abc123")
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;
            let _text = agent
                .chat_text("Call the who_am_i tool and tell me the result.")
                .await?;
            agent.shutdown().await?;

            let observed = captured.lock().unwrap().clone();
            assert_eq!(
                observed.as_deref(),
                Some("conv-live-abc123"),
                "custom tool must observe the agent's conversation_id via ToolContext"
            );
            Ok(())
        })
    });
}
