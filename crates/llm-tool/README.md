# llm-tool

Framework-agnostic Rust tool definitions for LLM agents.

Define strongly-typed tools with the `#[llm_tool]` attribute macro and register
them in a `ToolRegistry`. The registry produces `ToolDefinition` structs
(name + description + JSON Schema) that **any** LLM framework can consume —
there is zero coupling to a specific SDK or agent runtime.

## Quick start

Add `llm-tool` to your `Cargo.toml`:

```toml
[dependencies]
llm-tool = "0.1"
```

### Defining a tool with `#[llm_tool]`

The easiest way to create a tool is the `#[llm_tool]` attribute macro.
It generates a params struct and a `RustTool` implementation from a plain
function:

```rust
use llm_tool::{llm_tool, ToolError, ToolRegistry};

/// Adds two numbers together.
#[llm_tool]
fn add(
    /// First number.
    a: i64,
    /// Second number.
    b: i64,
) -> Result<String, ToolError> {
    Ok(format!("{}", a + b))
}

// The macro generates an `Add` struct (PascalCase of the fn name).
let registry = ToolRegistry::new().with_tool(Add);
let defs = registry.definitions();
assert_eq!(defs.len(), 1);
assert_eq!(defs[0].name, "add");
```

**Rules for `#[llm_tool]` functions:**

- Must have a **doc comment** (becomes the tool description).
- Every parameter must have a **doc comment** (becomes the JSON Schema
  description for that field).
- Return type can be `Result<T, E>` or a bare `T` (infallible tools):
  - `T` is `String` (auto-wrapped into `ToolOutput`), `ToolOutput`
    (passed through), any `T: Serialize` (auto-serialized to JSON),
    or any `T: Into<ToolOutput>`.
  - `E` is any `E: Into<ToolError>` — built-in conversions exist for
    `ToolError`, `String`, `std::io::Error`, `serde_json::Error`,
    and `Box<dyn Error + Send + Sync>`.
- Can be `async fn` — the generated `RustTool::call` is always async.
- `&str` parameters are accepted — the generated struct stores `String` and
  auto-borrows.
- `Option<T>` parameters automatically get `#[serde(default)]`, so they are
  omitted from the JSON Schema `required` array.
- A `&ToolContext` parameter is recognized as the execution context and
  forwarded from the registry — it is **not** included in the params struct.

### Returning `ToolOutput` with metadata

Tools can return a `ToolOutput` directly, attaching structured metadata
for hooks, policies, and logging pipelines. Metadata is **never** sent to
the model — only the `content` string is:

```rust
use llm_tool::{llm_tool, ToolOutput, ToolError};

/// Runs a shell command and returns its stdout.
#[llm_tool]
fn run_command(
    /// The command to execute.
    command: String,
) -> Result<ToolOutput, ToolError> {
    // In real code, you'd actually run the command.
    let stdout = format!("output of `{command}`");
    let exit_code = 0;
    Ok(ToolOutput::new(stdout)
        .with_meta("exit_code", serde_json::json!(exit_code))
        .with_meta("command", serde_json::json!(command)))
}
```

Similarly, `ToolError` supports metadata for error diagnostics:

```rust
use llm_tool::{llm_tool, ToolError};

/// Fetches a URL.
#[llm_tool]
fn fetch_url(
    /// The URL to fetch.
    url: String,
) -> Result<String, ToolError> {
    // Simulate an HTTP failure with structured error metadata.
    Err(ToolError::new("HTTP request failed")
        .with_meta("status_code", serde_json::json!(503))
        .with_meta("url", serde_json::json!(url)))
}
```

### The `?` operator — zero-boilerplate error handling

`ToolError` implements `From<std::io::Error>`, `From<serde_json::Error>`,
and `From<Box<dyn Error + Send + Sync>>`, so the `?` operator works
without manual `.map_err()`:

```rust
use llm_tool::{llm_tool, ToolError, ToolContext, ToolRegistry};

/// Reads a file from disk.
#[llm_tool]
async fn read_file(
    /// Path to the file.
    path: String,
) -> Result<String, ToolError> {
    // `?` auto-converts std::io::Error into ToolError with error_kind metadata.
    let content = std::fs::read_to_string(&path)?;
    Ok(content)
}

# futures::executor::block_on(async {
# let tmp = std::env::temp_dir().join("llm_tool_doctest_read_file.txt");
# std::fs::write(&tmp, "hello from test").unwrap();
# let registry = ToolRegistry::new().with_tool(ReadFile);
# let ctx = ToolContext::new(None);
# let result = registry.dispatch("read_file", serde_json::json!({"path": tmp.to_str().unwrap()}), &ctx).await.unwrap();
# assert_eq!(result.content(), "hello from test");
# std::fs::remove_file(&tmp).ok();
# });
```

### Infallible tools — no `Result` needed

Tools that can never fail can return a bare type instead of `Result`:

```rust
use llm_tool::{llm_tool, ToolRegistry};

/// Returns a friendly greeting.
#[llm_tool]
fn greet(
    /// Name to greet.
    name: String,
) -> String {
    format!("Hello, {name}!")
}

let registry = ToolRegistry::new().with_tool(Greet);
# futures::executor::block_on(async {
let ctx = llm_tool::ToolContext::new(None);
let result = registry
    .dispatch("greet", serde_json::json!({"name": "World"}), &ctx)
    .await
    .unwrap();
assert_eq!(result.content(), "Hello, World!");
# });
```

### Returning structured data with `ToolOutput::json()`

Use `ToolOutput::json()` to serialize any `T: Serialize` to JSON:

```rust
use llm_tool::{llm_tool, ToolOutput, ToolError};
use serde::Serialize;

#[derive(Serialize)]
struct Weather {
    temp: f64,
    unit: String,
    city: String,
}

/// Gets the current weather for a city.
#[llm_tool]
fn get_weather(
    /// City name.
    city: String,
) -> Result<ToolOutput, ToolError> {
    let data = Weather {
        temp: 72.0,
        unit: "F".into(),
        city,
    };
    ToolOutput::json(&data)
}
```

### Auto-serialized return types

Any `T: Serialize` returned from a tool is automatically serialized to JSON
by the macro's compile-time dispatch — no `ToolOutput::json()` call needed:

```rust
use llm_tool::{llm_tool, ToolError};
use serde::Serialize;

#[derive(Serialize)]
struct FileInfo {
    path: String,
    size: u64,
    is_dir: bool,
}

/// Returns metadata about a file.
#[llm_tool]
fn inspect_file(
    /// Path to inspect.
    path: String,
) -> Result<FileInfo, ToolError> {
    // The macro auto-serializes `FileInfo` to JSON via serde.
    Ok(FileInfo {
        path,
        size: 42,
        is_dir: false,
    })
}
```

### `Json<T>` wrapper for infallible serialization

Use `Json<T>` when your tool is infallible but returns a serializable
struct. It implements `Into<ToolOutput>`, panicking only if `Serialize`
is broken (use `ToolOutput::json()` for fallible serialization):

```rust
use llm_tool::{llm_tool, Json};
use serde::Serialize;

#[derive(Serialize)]
struct Stats {
    count: usize,
    label: String,
}

/// Computes statistics.
#[llm_tool]
fn compute_stats(
    /// Number of items.
    count: usize,
) -> Json<Stats> {
    Json(Stats {
        count,
        label: format!("{count} items processed"),
    })
}
```

### Custom `From<T>` for `ToolOutput`

Implement `From<YourType> for ToolOutput` for domain types that should
convert directly into tool output, then call `.into()` in the tool body:

```rust
use llm_tool::{llm_tool, ToolOutput, ToolError};

struct Markdown(String);

impl From<Markdown> for ToolOutput {
    fn from(md: Markdown) -> Self {
        ToolOutput::new(md.0)
            .with_meta("format", serde_json::json!("markdown"))
    }
}

/// Renders documentation as Markdown.
#[llm_tool]
fn render_docs(
    /// Topic to document.
    topic: String,
) -> Result<ToolOutput, ToolError> {
    Ok(Markdown(format!("# {topic}\n\nDocumentation for {topic}.")).into())
}
```

### Async tools

Async functions work out of the box. The body can `.await` freely:

```rust
use llm_tool::{llm_tool, ToolError, ToolContext, ToolRegistry};

/// Reads a file from disk.
#[llm_tool]
async fn read_file_async(
    /// Path to the file.
    path: String,
) -> Result<String, ToolError> {
    std::fs::read_to_string(&path)
        .map_err(|e| ToolError::new(format!("IO error: {e}")))
}

# futures::executor::block_on(async {
# let tmp = std::env::temp_dir().join("llm_tool_doctest_read_file_async.txt");
# std::fs::write(&tmp, "async hello").unwrap();
# let registry = ToolRegistry::new().with_tool(ReadFileAsync);
# let ctx = ToolContext::new(None);
# let result = registry.dispatch("read_file_async", serde_json::json!({"path": tmp.to_str().unwrap()}), &ctx).await.unwrap();
# assert_eq!(result.content(), "async hello");
# std::fs::remove_file(&tmp).ok();
# });
```

### Optional parameters

`Option<T>` fields are not required in the JSON the model sends:

```rust
use llm_tool::{llm_tool, ToolError};

/// Greets someone.
#[llm_tool]
fn greet_optional(
    /// Name to greet.
    name: String,
    /// Custom greeting (defaults to "Hello" if omitted).
    greeting: Option<String>,
) -> Result<String, ToolError> {
    let g = greeting.as_deref().unwrap_or("Hello");
    Ok(format!("{g}, {name}!"))
}
```

### Accessing `ToolContext`

If your tool needs the conversation ID or shared state, accept a
`&ToolContext` parameter. It is automatically wired by the registry and
**excluded** from the generated params struct:

```rust
use llm_tool::{llm_tool, ToolContext, ToolError};

/// Returns the current conversation ID.
#[llm_tool]
fn whoami(
    /// Unused placeholder.
    _label: String,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    Ok(ctx.conversation_id().unwrap_or("unknown").to_string())
}
```

### Manual `RustTool` implementation

For full control, implement `RustTool` directly:

```rust
use llm_tool::{JsonSchema, RustTool, ToolContext, ToolError, ToolOutput};
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
struct FlashParams {
    /// Target device identifier.
    device_id: String,
    /// Path to the firmware image.
    image_path: String,
}

struct FlashDevice;

impl RustTool for FlashDevice {
    type Params = FlashParams;
    const NAME: &'static str = "flash_device";
    const DESCRIPTION: &'static str = "Flashes firmware to a connected device.";

    async fn call(
        &self,
        params: Self::Params,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(format!(
            "Flashed {} to {}",
            params.image_path, params.device_id
        ).into())
    }
}
```

## `ToolRegistry`

The registry stores tools and provides two operations:

1. **`definitions()`** — returns `Vec<ToolDefinition>` with the name,
   description, and JSON Schema for each tool. Feed these to your framework.
2. **`dispatch(name, args, ctx)`** — deserializes JSON args, calls the tool,
   and returns a `ToolOutput` or a `ToolError`.

```rust
use llm_tool::{llm_tool, ToolContext, ToolError, ToolRegistry};

/// Echoes its input.
#[llm_tool]
fn echo(
    /// The message.
    message: String,
) -> Result<String, ToolError> {
    Ok(message)
}

# futures::executor::block_on(async {
let registry = ToolRegistry::new().with_tool(Echo);

// 1. Get definitions to send to the model.
let defs = registry.definitions();
assert_eq!(defs[0].name, "echo");

// 2. Dispatch a tool call from the model.
let ctx = ToolContext::new(None);
let result = registry
    .dispatch("echo", serde_json::json!({"message": "hi"}), &ctx)
    .await
    .unwrap();
assert_eq!(result.content(), "hi");
# });
```

## Plugging into any agent framework

`llm-tool` is deliberately framework-agnostic. To integrate with a new
framework:

1. **Register your tools** in a `ToolRegistry`.
2. **Extract definitions** via `registry.definitions()` — each
   `ToolDefinition` has `.name`, `.description`, and `.parameter_schema`
   (a `serde_json::Value` containing a JSON Schema object).
3. **Convert** `ToolDefinition`s into whatever format your framework expects
   (e.g. `OpenAI` function-calling JSON, Anthropic tool-use blocks, Gemini
   `FunctionDeclaration`s, etc.). The `parameter_schema` is standard
   JSON Schema (draft 7, with nullable arrays already sanitized to scalar
   `"type"` strings for Go genai compatibility).
4. **On tool call**, extract the tool name and JSON arguments from your
   framework's response, then call `registry.dispatch(name, args, &ctx)`.
5. **Return the result** (or error message) to the model as the tool
   response.

Minimal integration sketch:

```rust
use llm_tool::{ToolContext, ToolDefinition, ToolRegistry};

fn send_definitions_to_model(defs: &[ToolDefinition]) {
    // Convert each def.parameter_schema to your framework's format.
    for def in defs {
        println!(
            "Tool: {} — {} — schema: {}",
            def.name, def.description, def.parameter_schema
        );
    }
}

async fn handle_tool_call(
    registry: &ToolRegistry,
    name: &str,
    args: serde_json::Value,
) -> String {
    let ctx = ToolContext::new(Some("conv-123".into()));
    match registry.dispatch(name, args, &ctx).await {
        Ok(output) => output.into_content(),
        Err(e) => format!("Tool error: {e}"),
    }
}

# let registry = ToolRegistry::new();
# send_definitions_to_model(&registry.definitions());
# futures::executor::block_on(async {
#     let result = handle_tool_call(&registry, "nonexistent", serde_json::json!({})).await;
#     assert!(result.starts_with("Tool error:"));
# });
```

## Key types

| Type             | Description                                                           |
| ---------------- | --------------------------------------------------------------------- |
| `RustTool`       | Trait for implementing a tool with typed parameters.                  |
| `ToolRegistry`   | Registry for storing and dispatching tools by name.                   |
| `ToolDefinition` | Serializable metadata (name, description, JSON Schema).               |
| `ToolContext`    | Execution context with conversation state and shared key-value store. |
| `ToolOutput`     | Structured return value (content + metadata) from tool execution.     |
| `ToolError`      | Error type with `From` impls for `io::Error`, `serde_json::Error`.    |
| `Json<T>`        | Wrapper for infallible serialization of `T: Serialize` into output.   |
| `EmptyParams`    | Convenience struct for tools that take no parameters.                 |
| `#[llm_tool]`    | Proc-macro attribute for defining tools from plain functions.         |

## License

Dual-licensed under Apache-2.0 OR MIT.
