# agy-bridge

Build LLM agents in Rust — with tools, hooks, streaming, and multimodal input.

Rust bridge for the
[Google Antigravity SDK](https://github.com/Google-Antigravity/antigravity-sdk-python)
via [PyO3](https://pyo3.rs).

> Rust's compile-time checks make it a natural fit for vibe coding agents —
> schema mismatches, missing parameters, and typos in tool definitions
> (via [`#[llm_tool]`](#custom-tools)) are caught at build time, not at runtime.

## Installation

Add `agy-bridge` to your `Cargo.toml`:

```toml
[dependencies]
agy-bridge = "0.1.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Install the Python SDK:

```bash
pip install google-antigravity watchfiles
```

> `watchfiles` is only needed for file-change triggers; timer triggers
> work without it.

## Quick Start

```rust
use agy_bridge::AgyBridge;

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    # agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;
    let agent = bridge.default_agent().await?;

    // Send a message and get the full reply text.
    let text = agent.chat("Hello!").await?.text().await?;
    println!("{text}");

    agent.shutdown().await?;
    Ok(())
}
```

### Streaming Responses

```rust
use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    # agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // bridge.agent() returns an AgentBuilder — chain .tools() / .hooks()
    // before .await, or .await directly for a bare agent.
    let agent = bridge
        .agent(
            AgentConfig::builder()
                .system_instructions("You are a poet.")
                .build(),
        )
        .await?;

    let mut response = agent.chat("Write a short poem about space.").await?;

    if let Some(mut stream) = response.take_text_stream() {
        while let Some(chunk) = stream.recv().await {
            print!("{chunk}");
        }
    }
    println!();

    agent.shutdown().await?;
    Ok(())
}
```

## Features

### Multimodal Content

Pass text, images, audio, video, and documents in a single chat turn:

```rust
use agy_bridge::{
    AgyBridge,
    content::{Content, ContentPrimitive, Image},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    # agy_bridge::load_dotenv();

    let bridge = AgyBridge::builder().build()?;
    let agent = bridge.default_agent().await?;

    // Load an image from a file path (auto-detects MIME type):
    let image = Image::from_file("blank.png")?;

    let content = Content::Multi {
        parts: vec![
            ContentPrimitive::Text { text: "Describe this image.".into() },
            ContentPrimitive::Image(image),
        ],
    };

    let text = agent.chat(content).await?.text().await?;
    println!("{text}");

    agent.shutdown().await?;
    Ok(())
}
```

### Custom Tools

Define tools with the `#[llm_tool]` proc macro — doc comments become descriptions:

```rust
use agy_bridge::{AgyBridge, config::AgentConfig, prelude::*, tools::ToolRegistry};

/// Gets the current weather for a city.
#[llm_tool]
fn get_weather(
    /// The city to look up.
    city: &str,
) -> Result<String, String> {
    Ok(format!("It's sunny in {city}."))
}

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    # agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    let mut registry = ToolRegistry::new();
    registry.register(GetWeather);

    let agent = bridge
        .agent(AgentConfig::builder().build())
        .tools(registry)
        .await?;

    let text = agent.chat("What's the weather in Tokyo?").await?.text().await?;
    println!("{text}");

    agent.shutdown().await?;
    Ok(())
}
```

For full control, implement the `RustTool` trait directly:

```rust
use agy_bridge::tools::{JsonSchema, RustTool, ToolContext, ToolError, ToolOutput, ToolRegistry};
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
struct SearchParams {
    /// The search query string.
    query: String,
}

struct SearchTool;

impl RustTool for SearchTool {
    type Params = SearchParams;
    const NAME: &'static str = "search";
    const DESCRIPTION: &'static str = "Search a knowledge base";

    async fn call(&self, params: Self::Params, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(format!("Results for: {}", params.query).into())
    }
}

let registry = ToolRegistry::new().with_tool(SearchTool);
assert_eq!(registry.definitions().len(), 1);
```

### MCP Integration

Connect external [MCP](https://modelcontextprotocol.io/) servers:

```rust
use agy_bridge::config::{AgentConfig, McpServer};

let server = McpServer::stdio("npx")
    .args([
        "-y",
        "@modelcontextprotocol/server-postgres",
        "postgresql://postgres:postgres@localhost:5432/postgres",
    ])
    .build();

let config = AgentConfig::builder().mcp_servers([server]).build();
assert_eq!(config.mcp_servers.len(), 1);
```

### Hooks and Policies

Control agent behavior with hooks and a declarative policy system:

```rust
use agy_bridge::{
    AgyBridge,
    config::AgentConfig,
    hooks::{HookResult, Hooks},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    # agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // 1. Register lifecycle hooks
    let mut hooks = Hooks::new();

    hooks.on_pre_turn("turn_logger", |ctx| {
        println!("[turn {}] {}", ctx.turn_number, ctx.prompt);
    });

    hooks.on_pre_tool_call_decide("safety_gate", |ctx| {
        if ctx.tool_name == "dangerous_tool" {
            HookResult::deny("blocked by policy")
        } else {
            HookResult::allow()
        }
    });

    // 2. Create the agent configuration
    let config = AgentConfig::builder().build();

    // 3. Pass hooks to the agent builder
    let agent = bridge
        .agent(config)
        .hooks(hooks)
        .await?;

    let text = agent.chat("What is the capital of Japan?").await?.text().await?;
    println!("{text}");

    agent.shutdown().await?;
    Ok(())
}
```

```rust
use agy_bridge::policies::{PolicyRule, PolicySet};

let mut policies = PolicySet::new();
policies.push(PolicyRule::allow("view_file")).unwrap();
policies.push(PolicyRule::deny("run_command")).unwrap();
policies.push(PolicyRule::DenyAll).unwrap();

assert!(policies.evaluate("view_file").is_allowed());
assert!(policies.evaluate("run_command").is_denied());
assert!(policies.evaluate("unknown_tool").is_denied());
```

### Triggers

Run background tasks that react to timers or file changes:

```rust
use agy_bridge::{
    AgyBridge,
    config::AgentConfig,
    triggers::{TriggerConfig, TriggerEntry},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    # agy_bridge::load_dotenv();
    # std::fs::create_dir_all("/tmp/workspace")?;
    let bridge = AgyBridge::builder().build()?;

    let periodic = TriggerEntry {
        name: "poll_status".into(),
        config: TriggerConfig::every_secs(30),
        message_template: "Check deployment status".into(),
    };

    let file_watch = TriggerEntry {
        name: "watch_workspace".into(),
        config: TriggerConfig::on_file_change(std::env::current_dir()?),
        message_template: "Files changed: {changes}".into(),
    };

    let config = AgentConfig::builder()
        .triggers(vec![periodic, file_watch])
        .build();

    let agent = bridge.agent(config).await?;

    // The agent will now automatically run triggers in the background.
    let text = agent.chat("Hello!").await?.text().await?;
    println!("{text}");

    agent.shutdown().await?;
    # let _ = std::fs::remove_dir_all("/tmp/workspace");
    Ok(())
}
```

> **Note:** `on_file_change` triggers require the `watchfiles` Python package
> (`pip install watchfiles`). Timer triggers (`every`) work without extra
> dependencies.

### Subagents

Spawn child agents that share the parent's runtime:

```rust
use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    # agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    let parent = bridge.agent(
        AgentConfig::builder()
            .system_instructions("You are a coordinator.")
            .build(),
    ).await?;

    let child = parent.spawn_subagent(
        AgentConfig::builder()
            .system_instructions("You are a math specialist.")
            .model("gemini-3.5-flash")
            .build(),
        None,
    ).await?;

    let text = child.chat("What is 17 * 23?").await?.text().await?;
    println!("{text}");

    child.shutdown().await?;
    parent.shutdown().await?;
    Ok(())
}
```

## Examples

The [`examples/`](examples/) directory contains runnable programs for every feature:

### Getting Started

| Example                                                                      | Description                            |
| :--------------------------------------------------------------------------- | :------------------------------------- |
| [`hello_world`](examples/getting_started/hello_world.rs)                     | Minimal agent — create, prompt, print  |
| [`streaming`](examples/getting_started/streaming.rs)                         | Stream text tokens as they arrive      |
| [`custom_tools`](examples/getting_started/custom_tools.rs)                   | `#[llm_tool]` proc macro               |
| [`multimodal`](examples/getting_started/multimodal.rs)                       | Text + image input                     |
| [`mcp_tools`](examples/getting_started/mcp_tools.rs)                         | MCP server integration                 |
| [`structured_output`](examples/getting_started/structured_output.rs)         | JSON schema–constrained responses      |
| [`hooks`](examples/getting_started/hooks.rs)                                 | Lifecycle hooks                        |
| [`policies`](examples/getting_started/policies.rs)                           | Declarative tool access control        |
| [`triggers`](examples/getting_started/triggers.rs)                           | Periodic and file-change triggers      |
| [`subagents`](examples/getting_started/subagents.rs)                         | Parent/child agent spawning            |
| [`human_in_the_loop`](examples/getting_started/human_in_the_loop.rs)         | Human-in-the-loop chat patterns        |
| [`persistence`](examples/getting_started/persistence.rs)                     | Conversation persistence               |
| [`observability`](examples/getting_started/observability.rs)                 | Tracing and usage metadata             |
| [`error_handler`](examples/getting_started/error_handler.rs)                 | Structured error handling patterns     |
| [`autonomous_shell`](examples/getting_started/autonomous_shell.rs)           | Autonomous shell command execution     |
| [`persona_config`](examples/getting_started/persona_config.rs)               | Custom persona and model configuration |
| [`agent_skills`](examples/getting_started/agent_skills.rs)                   | Agent skill registration               |
| [`app_data_dir_override`](examples/getting_started/app_data_dir_override.rs) | Custom app data directory              |

### Deep Dives

| Example                                                                             | Description                          |
| :---------------------------------------------------------------------------------- | :----------------------------------- |
| [`async_chat`](examples/deep_dives/async_chat.rs)                                   | Advanced async chat patterns         |
| [`round_based_chat`](examples/deep_dives/round_based_chat.rs)                       | Multi-round structured conversations |
| [`agent_middleware`](examples/deep_dives/agent_middleware.rs)                       | Hook-based middleware pipeline       |
| [`host_tool_hooks`](examples/deep_dives/host_tool_hooks.rs)                         | Host-side tool interception hooks    |
| [`interactive_cli`](examples/deep_dives/interactive_cli.rs)                         | Multi-turn CLI chat application      |
| [`multimodal_pipeline`](examples/deep_dives/multimodal_pipeline.rs)                 | Multi-stage multimodal processing    |
| [`doc_maintenance_agent`](examples/deep_dives/doc_maintenance_agent.rs)             | Documentation maintenance agent      |
| [`docstring_maintenance_agent`](examples/deep_dives/docstring_maintenance_agent.rs) | Code docstring maintenance agent     |

```bash
cargo run --example getting_started_hello_world
```

## License

Licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.

---

_This is not a Google product. Use at your own risk._
