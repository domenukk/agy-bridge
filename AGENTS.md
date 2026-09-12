# AGENTS.md — agy-bridge

Rust agent SDK with two interchangeable execution backends behind one
`Runtime` trait: a pure-Rust `NativeRuntime` driving the `localharness`
binary (default), and a `PythonRuntime` wrapping the
[antigravity-sdk-python](https://github.com/Google-Antigravity/antigravity-sdk-python)
via PyO3. Rust provides the ergonomic builder/struct API in both cases.

## Architecture

```text
                          ┌─▶ NativeRuntime ──▶ WebSocket + protobuf
                          │   (feature "native", default)      │
Rust caller ──▶ AgentHandle                                localharness
                          │
                          └─▶ PythonRuntime ──▶ PyO3 ──▶ antigravity-sdk-python
                              (feature "python", dedicated thread)
```

- `src/runtime/` — command dispatch over an mpsc channel to an isolated Python thread.
- `src/runtime/py/` — native Python helper scripts that run inside that thread.
- `src/runtime/native/` — pure-Rust backend: spawns and drives the `localharness`
  binary over WebSocket + protobuf, no Python involved. Enabled by default.
- `src/agent/` — `AgentHandle` lifecycle: create → chat → shutdown.
- `src/hooks/` — pre/post turn, tool-call gating, session, and compaction callbacks.
- `src/tools/` — `#[llm_tool]` proc macro and `ToolRegistry` for custom Rust tools.
- `src/config/` — `AgentConfig` builder, MCP servers, capabilities.
- `src/policies/` — declarative allow/deny/confirm rules for tool execution.
- `src/triggers.rs` — periodic and file-change trigger definitions.
- `src/streaming/` — streaming response channels (text, thought, tool-call events).
- `src/content/` — multimodal input types (text, image, audio, video, document).
- `src/proto/` — generated protobuf types for the native harness protocol.
- `src/error.rs` — the crate's `Error` type and retry/quota classification.
- `src/types.rs` — shared domain types.

## Rules

- Never allow `dead_code`. Tests must always pass.
- Write Rust, not C/C++ or Python.
  Don't ignore errors in error handlers —
  handle them or worst case log them.
- The README doubles as the crate-level rustdoc (`#![doc = include_str!("../README.md")]`).
  Keep code examples in the README compilable and runnable (`cargo test --doc`).
- **Tests must NEVER use excessive RAM**:
  - Tests must never cause OOM or host memory starvation (< 1 GB peak RSS).
  - Avoid redundant runtime or engine instantiations across test threads; reuse shared instances.
  - All spawned background processes (e.g. `localharness`) and tasks must be killed and reaped upon drop/shutdown.

## Testing

```sh
just test # all tests, including live integration tests
```

## Formatting

```sh
just fmt  # runs cargo +nightly fmt, taplo fmt, prettier, black, just --fmt
```
