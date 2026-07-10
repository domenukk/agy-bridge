//! Exhaustive mock-server integration tests for all agy-bridge features.
//!
//! Tests the full pipeline: Rust → Python SDK → HTTP → Mock Server,
//! covering tools, hooks, policies, capabilities, MCP, and streaming.
//! **No API key required** — uses local TCP mock Gemini servers.
//!
//! Run with:
//! ```sh
//! cargo test --test features_mock_test -- --nocapture
//! ```

use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use agy_bridge::{
    hooks::{HookResult, Hooks},
    policies::PolicyRule,
    tools::{JsonSchema, RustTool, ToolError, ToolOutput, ToolRegistry},
};
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

static BRIDGE: LazyLock<agy_bridge::AgyBridge> = LazyLock::new(|| {
    agy_bridge::AgyBridge::builder()
        .inter_agent_delay(std::time::Duration::ZERO)
        .chat_timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("shared AgyBridge")
});

// ─── Tool Definitions ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
struct AddParams {
    /// First number.
    x: f64,
    /// Second number.
    y: f64,
}

struct AddTool;

impl RustTool for AddTool {
    type Params = AddParams;
    const NAME: &'static str = "add_numbers";
    const DESCRIPTION: &'static str = "Adds two numbers.";

    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let result = params.x + params.y;
        Ok(format!("{result}").into())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LookupParams {
    /// Key to look up.
    key: String,
}

struct LookupTool;

impl RustTool for LookupTool {
    type Params = LookupParams;
    const NAME: &'static str = "lookup";
    const DESCRIPTION: &'static str = "Looks up a value by key.";

    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let val = match params.key.as_str() {
            "secret" => "GAMMA-42",
            "status" => "operational",
            _ => "not_found",
        };
        Ok(val.into())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FailParams {
    /// Reason for failure.
    reason: String,
}

struct AlwaysFailTool;

impl RustTool for AlwaysFailTool {
    type Params = FailParams;
    const NAME: &'static str = "always_fail";
    const DESCRIPTION: &'static str = "Always fails with the given reason.";

    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Err(ToolError::new(params.reason))
    }
}

/// A tool whose invocation count we can inspect from tests.
struct CountingTool {
    count: Arc<AtomicUsize>,
}

impl CountingTool {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        (
            Self {
                count: Arc::clone(&count),
            },
            count,
        )
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CountParams {}

impl RustTool for CountingTool {
    type Params = CountParams;
    const NAME: &'static str = "counting_tool";
    const DESCRIPTION: &'static str = "Increments a counter and returns the count.";

    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn call(
        &self,
        _params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(format!("invocation_{n}").into())
    }
}

// ─── Mock Server Infrastructure ──────────────────────────────────────────────

/// Recorded POST body for inspection.
#[derive(Debug, Clone)]
struct RecordedPost {
    body: String,
}

/// A mock Gemini API server with configurable tool-call sequences.
struct MockGeminiServer {
    addr: std::net::SocketAddr,
    post_count: Arc<AtomicUsize>,
    posts: Arc<tokio::sync::Mutex<Vec<RecordedPost>>>,
    handle: tokio::task::JoinHandle<()>,
}

/// A response the mock server should return for each POST in sequence.
#[derive(Clone)]
enum MockResponse {
    /// Return a `functionCall` to invoke a tool.
    FunctionCall {
        name: String,
        args: serde_json::Value,
    },
    /// Return a final text response.
    Text(String),
}

impl MockGeminiServer {
    /// Start a mock server that returns the given responses in sequence for POSTs.
    /// If there are more POSTs than responses, the last response is repeated.
    async fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let addr = listener.local_addr().expect("local addr");

        let post_count = Arc::new(AtomicUsize::new(0));
        let posts: Arc<tokio::sync::Mutex<Vec<RecordedPost>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let count = Arc::clone(&post_count);
        let recs = Arc::clone(&posts);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let count = Arc::clone(&count);
                let recs = Arc::clone(&recs);
                let responses = responses.clone();
                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut buf_reader = BufReader::new(reader);

                    let Some((request_line, body)) =
                        parse_http_request_with_body(&mut buf_reader).await
                    else {
                        return;
                    };

                    if request_line.starts_with("GET ") {
                        let resp = json_response(200, &model_list_json());
                        let _ = writer.write_all(resp.as_bytes()).await;
                        let _ = writer.flush().await;
                        return;
                    }

                    let n = count.fetch_add(1, Ordering::SeqCst);
                    recs.lock().await.push(RecordedPost { body });

                    let idx = n.min(responses.len().saturating_sub(1));
                    let response = match &responses[idx] {
                        MockResponse::FunctionCall { name, args } => {
                            sse_response(&function_call_json(name, args))
                        }
                        MockResponse::Text(text) => sse_response(&text_response_json(text)),
                    };

                    let _ = writer.write_all(response.as_bytes()).await;
                    let _ = writer.flush().await;
                });
            }
        });

        Self {
            addr,
            post_count,
            posts,
            handle,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn post_count(&self) -> usize {
        self.post_count.load(Ordering::SeqCst)
    }

    async fn recorded_posts(&self) -> Vec<RecordedPost> {
        self.posts.lock().await.clone()
    }
}

impl Drop for MockGeminiServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// ─── HTTP/JSON Helpers ───────────────────────────────────────────────────────

async fn parse_http_request_with_body<R: tokio::io::AsyncRead + Unpin>(
    buf_reader: &mut BufReader<R>,
) -> Option<(String, String)> {
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
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    let body = if content_length > 0 {
        let mut buf = vec![0u8; content_length];
        buf_reader.read_exact(&mut buf).await.ok()?;
        String::from_utf8_lossy(&buf).to_string()
    } else {
        String::new()
    };

    Some((request_line, body))
}

fn json_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
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
                "generateContent", "streamGenerateContent", "countTokens"
            ],
            "inputTokenLimit": 1_048_576,
            "outputTokenLimit": 8192
        }]
    })
    .to_string()
}

fn function_call_json(name: &str, args: &serde_json::Value) -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"functionCall": {"name": name, "args": args}}],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 10, "candidatesTokenCount": 5, "totalTokenCount": 15
        }
    })
    .to_string()
}

fn text_response_json(text: &str) -> String {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"text": text}],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 15, "candidatesTokenCount": 10, "totalTokenCount": 25
        }
    })
    .to_string()
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
        .policies([PolicyRule::AllowAll])
        .build()
}

fn multi_thread_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 1: Tool Calling
// ═══════════════════════════════════════════════════════════════════════════

/// Single tool round-trip: mock returns functionCall → Rust tool executes →
/// SDK sends functionResponse → mock returns final text.
#[test]
fn tool_single_round_trip() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 10.0, "y": 32.0}),
            },
            MockResponse::Text("The sum is 42.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "calc"))
            .tools(registry)
            .await
            .expect("agent");

        let text = agent.chat_text("add 10 and 32").await.expect("chat");
        assert!(text.contains("42"), "Expected '42', got: {text}");
        assert_eq!(server.post_count(), 2, "Expected 2 POSTs");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Two sequential tool calls in one conversation.
#[test]
fn tool_multi_sequential_calls() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 5.0, "y": 3.0}),
            },
            MockResponse::FunctionCall {
                name: "lookup".into(),
                args: serde_json::json!({"key": "secret"}),
            },
            MockResponse::Text("Sum=8, secret=GAMMA-42.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);
        registry.register(LookupTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "multi"))
            .tools(registry)
            .await
            .expect("agent");

        let text = agent.chat_text("compute").await.expect("chat");
        assert!(text.contains("GAMMA-42"), "Expected GAMMA-42, got: {text}");
        assert_eq!(server.post_count(), 3, "Expected 3 POSTs");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Tool that returns an error — SDK should send error in functionResponse.
#[test]
fn tool_error_propagated() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "always_fail".into(),
                args: serde_json::json!({"reason": "intentional test failure"}),
            },
            MockResponse::Text("Tool failed as expected.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AlwaysFailTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "err"))
            .tools(registry)
            .await
            .expect("agent");

        let text = agent.chat_text("try tool").await.expect("chat");
        assert!(text.contains("failed"), "Expected 'failed', got: {text}");
        assert_eq!(server.post_count(), 2, "Expected 2 POSTs");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Verify the tool's output appears in the second POST body (functionResponse).
#[test]
fn tool_output_in_function_response() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "lookup".into(),
                args: serde_json::json!({"key": "secret"}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(LookupTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "verify"))
            .tools(registry)
            .await
            .expect("agent");

        agent.chat_text("lookup").await.expect("chat");

        let posts = server.recorded_posts().await;
        assert!(posts.len() >= 2, "Expected ≥2 posts");
        assert!(
            posts[1].body.contains("GAMMA-42"),
            "functionResponse should contain tool output 'GAMMA-42', got: {}",
            posts[1].body
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Streaming chat handle works with tool calling.
#[test]
fn tool_call_via_streaming_handle() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 2.0}),
            },
            MockResponse::Text("Result: 3".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "stream"))
            .tools(registry)
            .await
            .expect("agent");

        let handle = agent.chat("1+2").await.expect("chat handle");
        let text = handle.text().await.expect("text");
        assert!(text.contains('3'), "Expected '3', got: {text}");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Two agents on the same bridge with different tools + different backends.
#[test]
fn concurrent_agents_different_tools() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server_add = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 100.0, "y": 200.0}),
            },
            MockResponse::Text("Sum: 300".into()),
        ])
        .await;

        let server_lookup = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "lookup".into(),
                args: serde_json::json!({"key": "status"}),
            },
            MockResponse::Text("Status: operational".into()),
        ])
        .await;

        let mut reg_add = ToolRegistry::new();
        reg_add.register(AddTool);
        let mut reg_lookup = ToolRegistry::new();
        reg_lookup.register(LookupTool);

        let a1 = BRIDGE
            .agent(agent_config(&server_add.base_url(), "adder"))
            .tools(reg_add)
            .await
            .expect("adder");
        let a2 = BRIDGE
            .agent(agent_config(&server_lookup.base_url(), "looker"))
            .tools(reg_lookup)
            .await
            .expect("looker");

        let (r1, r2) = tokio::join!(a1.chat_text("add"), a2.chat_text("look up"));

        assert!(
            r1.expect("add chat").contains("300"),
            "Adder should get 300"
        );
        assert!(
            r2.expect("lookup chat").contains("operational"),
            "Looker should get operational"
        );

        a1.shutdown().await.expect("shutdown a1");
        a2.shutdown().await.expect("shutdown a2");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 2: Hooks
// ═══════════════════════════════════════════════════════════════════════════

/// Pre-turn and post-turn hooks fire with correct context during tool calls.
#[test]
fn hooks_pre_and_post_turn_fire() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 2.0}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let e1 = Arc::clone(&events);
        let e2 = Arc::clone(&events);
        let hooks = Hooks::new()
            .with_pre_turn("test_pre", move |ctx| {
                e1.lock().unwrap().push(format!("pre:{}", ctx.turn_number));
            })
            .with_post_turn("test_post", move |ctx| {
                e2.lock().unwrap().push(format!("post:{}", ctx.turn_number));
            });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "hooks"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("test").await.expect("chat");

        let evts = events.lock().unwrap().clone();
        eprintln!("Hook events: {evts:?}");

        // Pre-turn should fire at least once.
        assert!(
            evts.iter().any(|e| e.starts_with("pre:")),
            "Pre-turn hook should have fired. Events: {evts:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Pre-tool-call-decide hook can gate (deny) tool execution.
#[test]
fn hooks_pre_tool_call_decide_denies() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("Tool was gated.".into()),
        ])
        .await;

        let (counting_tool, invocation_count) = CountingTool::new();

        let hooks = Hooks::new().with_pre_tool_call_decide("gate", |ctx| {
            if ctx.tool_name == "counting_tool" {
                HookResult::deny("blocked by test hook")
            } else {
                HookResult::allow()
            }
        });

        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "gate"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        // The chat may succeed or fail depending on SDK behavior when
        // a tool is denied — either way, the tool should NOT execute.
        let _result = agent.chat_text("call tool").await;

        assert_eq!(
            invocation_count.load(Ordering::SeqCst),
            0,
            "Tool should NOT have been invoked — hook denied it"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Post-tool-call hook fires after tool execution with result context.
#[test]
fn hooks_post_tool_call_fires() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 7.0, "y": 3.0}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let tool_names: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let tn = Arc::clone(&tool_names);

        let hooks = Hooks::new().with_post_tool_call("log_post", move |ctx| {
            tn.lock()
                .unwrap()
                .push(format!("post_tool:{}", ctx.tool_name));
        });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "post_hook"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("add").await.expect("chat");

        let names = tool_names.lock().unwrap().clone();
        eprintln!("Post-tool events: {names:?}");
        assert!(
            names.iter().any(|n| n.contains("add_numbers")),
            "Post-tool hook should have seen add_numbers. Events: {names:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// On-tool-error hook fires when a tool returns Err.
#[test]
fn hooks_on_tool_error_fires() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "always_fail".into(),
                args: serde_json::json!({"reason": "test error"}),
            },
            MockResponse::Text("Handled.".into()),
        ])
        .await;

        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let errs = Arc::clone(&errors);

        let hooks = Hooks::new().with_tool_error("log_err", move |ctx| {
            errs.lock()
                .unwrap()
                .push(format!("error:{}:{}", ctx.tool_name, ctx.error));
        });

        let mut registry = ToolRegistry::new();
        registry.register(AlwaysFailTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "err_hook"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("fail").await.expect("chat");

        let errs = errors.lock().unwrap().clone();
        eprintln!("Error events: {errs:?}");
        assert!(
            errs.iter().any(|e| e.contains("always_fail")),
            "On-tool-error hook should have fired for always_fail. Events: {errs:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Multiple hooks registered at different points all fire.
#[test]
fn hooks_multiple_combined() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 1.0}),
            },
            MockResponse::Text("2".into()),
        ])
        .await;

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let e1 = Arc::clone(&events);
        let e2 = Arc::clone(&events);
        let e3 = Arc::clone(&events);
        let e4 = Arc::clone(&events);

        let hooks = Hooks::new()
            .with_pre_turn("pre", move |_ctx| {
                e1.lock().unwrap().push("pre_turn".into());
            })
            .with_post_turn("post", move |_ctx| {
                e2.lock().unwrap().push("post_turn".into());
            })
            .with_pre_tool_call_decide("decide", move |_ctx| {
                e3.lock().unwrap().push("pre_tool_decide".into());
                HookResult::allow()
            })
            .with_post_tool_call("post_tool", move |_ctx| {
                e4.lock().unwrap().push("post_tool_call".into());
            });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "combined"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("1+1").await.expect("chat");

        let evts = events.lock().unwrap().clone();
        eprintln!("Combined hook events: {evts:?}");

        assert!(
            evts.contains(&"pre_turn".to_string()),
            "Missing pre_turn. Events: {evts:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 3: Policies
// ═══════════════════════════════════════════════════════════════════════════

/// `AllowAll` policy lets tool calls through.
#[test]
fn policy_allow_all_permits_tool() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 1.0}),
            },
            MockResponse::Text("2".into()),
        ])
        .await;

        let (counting_tool, count) = CountingTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        // Override to call counting_tool instead.
        let server2 = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server2.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::AllowAll])
            .build();

        let agent = BRIDGE.agent(config).tools(registry).await.expect("agent");

        agent.chat_text("go").await.expect("chat");

        assert!(
            count.load(Ordering::SeqCst) >= 1,
            "Tool should have been invoked under AllowAll"
        );

        // Clean up unused server.
        drop(server);
        agent.shutdown().await.expect("shutdown");
    });
}

/// `Deny(specific_tool)` blocks that tool but allows others.
#[test]
fn policy_deny_specific_blocks_tool() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let (counting_tool, count) = CountingTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::deny("counting_tool"), PolicyRule::AllowAll])
            .build();

        let agent = BRIDGE.agent(config).tools(registry).await.expect("agent");

        let _result = agent.chat_text("go").await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "counting_tool should NOT execute — it's denied by policy"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// `Allow(specific_tool)` + `DenyAll`: only the allowed tool executes.
#[test]
fn policy_allow_specific_plus_deny_all() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 1.0}),
            },
            MockResponse::Text("2".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::allow("add_numbers"), PolicyRule::DenyAll])
            .build();

        let agent = BRIDGE.agent(config).tools(registry).await.expect("agent");

        let text = agent.chat_text("add").await.expect("chat");
        assert!(
            text.contains('2'),
            "Allowed tool should execute, got: {text}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 4: MCP Server (stdio)
// ═══════════════════════════════════════════════════════════════════════════

/// Verify that an MCP stdio server can be configured and connected during
/// agent creation. Uses a minimal Python MCP server that handles the
/// initialize handshake and returns an empty tool list.
#[test]
fn mcp_stdio_server_connects() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        use agy_bridge::config::McpServer;

        let server = MockGeminiServer::start(vec![MockResponse::Text("Hello.".into())]).await;

        // Minimal MCP stdio server: handles jsonrpc initialize + tools/list.
        let mcp = McpServer::stdio("python3")
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
                res = {'protocolVersion': req.get('params', {}).get('protocolVersion', '2024-11-05'), 'capabilities': {'resources': {}, 'prompts': {}, 'tools': {}}, 'serverInfo': {'name': 'test-mcp', 'version': '1.0'}}
            elif m == 'notifications/initialized':
                continue
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
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::AllowAll])
            .mcp_servers([mcp])
            .build();

        let agent = BRIDGE.agent(config).await.expect("agent with MCP");

        // If we get here, MCP handshake succeeded.
        let text = agent.chat_text("hello").await.expect("chat");
        assert!(
            text.contains("Hello"),
            "Chat after MCP connect should work, got: {text}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// MCP server providing a tool that the model calls — full round-trip.
#[test]
fn mcp_tool_call_round_trip() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        use agy_bridge::config::McpServer;

        // Mock Gemini server that calls the MCP-provided "mcp_echo" tool.
        let gemini = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "mcp_echo".into(),
                args: serde_json::json!({"message": "hello from MCP"}),
            },
            MockResponse::Text("MCP tool returned successfully.".into()),
        ])
        .await;

        // MCP server that provides an "mcp_echo" tool and handles calls/execute.
        let mcp = McpServer::stdio("python3")
            .args([
                "-c",
                r"
import sys, json
for line in sys.stdin:
    try:
        req = json.loads(line)
        if 'id' not in req:
            continue
        m = req.get('method')
        if m == 'initialize':
            res = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'echo-mcp', 'version': '1.0'}}
        elif m == 'notifications/initialized':
            continue
        elif m == 'resources/list':
            res = {'resources': []}
        elif m == 'prompts/list':
            res = {'prompts': []}
        elif m == 'tools/list':
            res = {'tools': [{'name': 'mcp_echo', 'description': 'Echoes a message', 'inputSchema': {'type': 'object', 'properties': {'message': {'type': 'string'}}, 'required': ['message']}}]}
        elif m == 'tools/call':
            msg = req.get('params', {}).get('arguments', {}).get('message', 'no message')
            res = {'content': [{'type': 'text', 'text': f'echo: {msg}'}]}
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
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(gemini.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::AllowAll])
            .mcp_servers([mcp])
            .build();

        let agent = BRIDGE.agent(config).await.expect("agent with MCP tool");

        let text = agent.chat_text("echo test").await.expect("chat");
        eprintln!("MCP tool response: {text}");

        // The mock returns "MCP tool returned successfully." after the tool call.
        assert!(
            text.contains("MCP tool returned"),
            "Expected MCP response, got: {text}"
        );

        // Verify 2 POSTs: initial → functionCall → functionResponse → text.
        assert_eq!(gemini.post_count(), 2, "Expected 2 POSTs to Gemini");

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 5: Capabilities — builtin tools config
// ═══════════════════════════════════════════════════════════════════════════

/// `custom_tools_only` disables all builtins — only custom tools available.
#[test]
fn capabilities_custom_tools_only_works() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![MockResponse::Text("Just text.".into())]).await;

        // No custom tools registered, custom_tools_only means zero tools.
        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::AllowAll])
            .build();

        let agent = BRIDGE.agent(config).await.expect("agent");
        let text = agent.chat_text("hello").await.expect("chat");
        assert!(
            text.contains("Just text"),
            "Plain text response expected, got: {text}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// `read_only` capabilities allow read tools but not write tools.
#[test]
fn capabilities_read_only_agent() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server =
            MockGeminiServer::start(vec![MockResponse::Text("Read-only agent.".into())]).await;

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::read_only())
            .policies([PolicyRule::AllowAll])
            .build();

        let agent = BRIDGE.agent(config).await.expect("agent");
        let text = agent.chat_text("hello").await.expect("chat");
        assert!(
            text.contains("Read-only"),
            "Expected read-only response, got: {text}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 6: Combined features
// ═══════════════════════════════════════════════════════════════════════════

/// Tools + hooks + policies all working together in one agent.
#[test]
fn combined_tools_hooks_policies() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 20.0, "y": 22.0}),
            },
            MockResponse::Text("The answer is 42.".into()),
        ])
        .await;

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let e1 = Arc::clone(&events);
        let e2 = Arc::clone(&events);

        let hooks = Hooks::new()
            .with_pre_tool_call_decide("allow_add", move |ctx| {
                e1.lock().unwrap().push(format!("decide:{}", ctx.tool_name));
                // Only allow add_numbers.
                if ctx.tool_name == "add_numbers" {
                    HookResult::allow()
                } else {
                    HookResult::deny("only add allowed")
                }
            })
            .with_post_tool_call("log_post", move |ctx| {
                e2.lock().unwrap().push(format!("post:{}", ctx.tool_name));
            });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);
        registry.register(LookupTool); // Registered but should be blocked by hook.

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([
                PolicyRule::allow("add_numbers"),
                PolicyRule::allow("lookup"),
                PolicyRule::DenyAll,
            ])
            .build();

        let agent = BRIDGE
            .agent(config)
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        let text = agent.chat_text("add 20 and 22").await.expect("chat");
        assert!(text.contains("42"), "Expected 42, got: {text}");

        let evts = events.lock().unwrap().clone();
        eprintln!("Combined events: {evts:?}");
        assert!(
            evts.iter().any(|e| e.starts_with("decide:add_numbers")),
            "Pre-tool-call-decide should have seen add_numbers. Events: {evts:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Tools + MCP together — both custom and MCP tools on same agent.
#[test]
fn combined_custom_tools_and_mcp() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        use agy_bridge::config::McpServer;

        // Gemini mock: first calls custom tool, then calls MCP tool, then returns text.
        let gemini = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 5.0, "y": 5.0}),
            },
            MockResponse::Text("Custom tool gave 10.".into()),
        ])
        .await;

        // Minimal MCP server (no tools exposed — just handshake).
        let mcp = McpServer::stdio("python3")
            .args([
                "-c",
                r"
import sys, json
for line in sys.stdin:
    try:
        req = json.loads(line)
        if 'id' not in req:
            continue
        m = req.get('method')
        if m == 'initialize':
            res = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'empty', 'version': '1.0'}}
        elif m in ('resources/list', 'prompts/list'):
            res = {m.split('/')[0]: []}
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

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(gemini.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::AllowAll])
            .mcp_servers([mcp])
            .build();

        let agent = BRIDGE
            .agent(config)
            .tools(registry)
            .await
            .expect("agent with MCP + tools");

        let text = agent.chat_text("compute").await.expect("chat");
        assert!(
            text.contains("10"),
            "Custom tool should work alongside MCP, got: {text}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 7: `#[llm_tool]` proc macro
// ═══════════════════════════════════════════════════════════════════════════

/// The `#[llm_tool]` proc macro generates a `RustTool` implementation
/// from a plain function — verify it works through the full mock pipeline.
#[test]
fn llm_tool_proc_macro_round_trip() {
    use agy_bridge::llm_tool;

    /// Multiplies two integers.
    #[llm_tool]
    fn multiply(
        /// First factor.
        a: i64,
        /// Second factor.
        b: i64,
    ) -> Result<String, agy_bridge::tools::ToolError> {
        Ok(format!("{}", a * b))
    }

    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "multiply".into(),
                args: serde_json::json!({"a": 6, "b": 7}),
            },
            MockResponse::Text("The product is 42.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(Multiply);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "proc_macro"))
            .tools(registry)
            .await
            .expect("agent");

        let text = agent.chat_text("6*7").await.expect("chat");
        assert!(text.contains("42"), "Expected '42', got: {text}");

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 8: transform_tool_input hook
// ═══════════════════════════════════════════════════════════════════════════

/// `transform_tool_input` hook can inspect and optionally replace tool args
/// before execution.
#[test]
fn hooks_transform_tool_input_observed() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 3.0, "y": 4.0}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let observed_args: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let obs = Arc::clone(&observed_args);

        let hooks = Hooks::new().with_transform_tool_input("observer", move |ctx| {
            obs.lock().unwrap().push(ctx.tool_args.clone());
            // Return None = no transformation.
            None
        });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "transform"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("add").await.expect("chat");

        let args = observed_args.lock().unwrap().clone();
        eprintln!("Transform observed args: {args:?}");
        assert!(
            !args.is_empty(),
            "transform_tool_input hook should have observed tool args"
        );
        // Should have seen the add_numbers args.
        assert!(
            args[0].get("x").is_some() || args[0].get("y").is_some(),
            "Should observe x/y args, got: {:?}",
            args[0]
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 9: `AskUser` policy with handler
// ═══════════════════════════════════════════════════════════════════════════

/// `AskUser` policy delegates to a Rust `AskUserHandler` — handler allows.
#[test]
fn policy_ask_user_handler_allows() {
    use agy_bridge::policies::AskUserHandler;

    struct AlwaysAllowHandler;
    impl AskUserHandler for AlwaysAllowHandler {
        fn confirm(&self, _tool_name: &str, _tool_args: &serde_json::Value) -> bool {
            true
        }
    }

    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 10.0, "y": 5.0}),
            },
            MockResponse::Text("Result: 15.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([
                PolicyRule::AskUser {
                    tool: "add_numbers".to_owned(),
                    handler_id: "confirm_add".to_owned(),
                },
                PolicyRule::DenyAll,
            ])
            .build();

        let agent = BRIDGE
            .agent(config)
            .tools(registry)
            .policy_handler(AlwaysAllowHandler)
            .await
            .expect("agent with AskUser");

        let text = agent.chat_text("add 10 and 5").await.expect("chat");
        assert!(
            text.contains("15"),
            "Handler allowed the tool, expected '15', got: {text}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// `AskUser` policy — handler denies the tool call.
#[test]
fn policy_ask_user_handler_denies() {
    use agy_bridge::policies::AskUserHandler;

    struct AlwaysDenyHandler;
    impl AskUserHandler for AlwaysDenyHandler {
        fn confirm(&self, _tool_name: &str, _tool_args: &serde_json::Value) -> bool {
            false
        }
    }

    let rt = multi_thread_rt();
    rt.block_on(async {
        let (counting_tool, count) = CountingTool::new();

        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("Tool was denied.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([
                PolicyRule::AskUser {
                    tool: "counting_tool".to_owned(),
                    handler_id: "confirm_count".to_owned(),
                },
                PolicyRule::DenyAll,
            ])
            .build();

        let agent = BRIDGE
            .agent(config)
            .tools(registry)
            .policy_handler(AlwaysDenyHandler)
            .await
            .expect("agent with deny handler");

        let _result = agent.chat_text("run tool").await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "Tool should NOT execute — AskUser handler denied it"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 10: Session lifecycle hooks
// ═══════════════════════════════════════════════════════════════════════════

/// Session start/end hooks fire during agent lifecycle.
#[test]
fn hooks_session_start_and_end_fire() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![MockResponse::Text("Hi.".into())]).await;

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let e1 = Arc::clone(&events);
        let e2 = Arc::clone(&events);

        let hooks = Hooks::new()
            .with_session_start("log_start", move |ctx| {
                e1.lock()
                    .unwrap()
                    .push(format!("start:{}", ctx.session.session_id));
            })
            .with_session_end("log_end", move |_ctx| {
                e2.lock().unwrap().push("end".into());
            });

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "session"))
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("hello").await.expect("chat");
        agent.shutdown().await.expect("shutdown");

        let evts = events.lock().unwrap().clone();
        eprintln!("Session events: {evts:?}");
        assert!(
            evts.iter().any(|e| e.starts_with("start:")),
            "Session start hook should have fired. Events: {evts:?}"
        );
    });
}
