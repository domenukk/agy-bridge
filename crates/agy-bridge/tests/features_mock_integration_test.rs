//! Mock-server integration tests: MCP servers, capabilities, and combined features.
//!
//! Covers MCP stdio server connection + tool round-trips, capability configs
//! (`custom_tools_only`, `read_only`), and combined tools+hooks+policies /
//! tools+MCP scenarios. **No API key required.**
//!
//! Run with:
//! ```sh
//! cargo test --test features_mock_integration_test -- --nocapture
//! ```

use std::sync::{Arc, Mutex};

use agy_bridge::{
    hooks::{HookResult, Hooks},
    policies::PolicyRule,
    tools::ToolRegistry,
};
use agy_bridge_test_support::*;

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
