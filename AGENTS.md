# AGENTS.md — agy-bridge

Rust bridge wrapping the
[antigravity-sdk-python](https://github.com/Google-Antigravity/antigravity-sdk-python)
via PyO3. Rust provides the ergonomic builder/struct API;
Python is the execution backend.

## Architecture

```text
Rust caller ──▶ AgentHandle ──▶ PythonRuntime (dedicated thread)
                                      │
                                    PyO3
                                      │
                                antigravity-sdk-python
```

- `src/runtime/` — command dispatch over an mpsc channel to an isolated Python thread.
- `src/runtime/py/` — native Python helper scripts that run inside that thread.
- `src/agent/` — `AgentHandle` lifecycle: create → chat → shutdown.
- `src/hooks/` — pre/post turn, tool-call gating, session, and compaction callbacks.
- `src/tools/` — `#[llm_tool]` proc macro and `ToolRegistry` for custom Rust tools.
- `src/config/` — `AgentConfig` builder, MCP servers, capabilities.
- `src/policies/` — declarative allow/deny/confirm rules for tool execution.
- `src/triggers.rs` — periodic and file-change trigger definitions.
- `src/streaming/` — streaming response channels (text, thought, tool-call events).
- `src/content/` — multimodal input types (text, image, audio, video, document).
- `src/quota.rs` — quota tracking and backoff state.
- `src/safety.rs` — safety filter detection heuristics.
- `src/types.rs` — shared domain types.

## Rules

- Never allow `dead_code`. Tests must always pass.
- Write Rust, not C/C++ or Python.
  Don't ignore errors in error handlers —
  handle them or worst case log them.
- The README doubles as the crate-level rustdoc (`#![doc = include_str!("../README.md")]`).
  Keep code examples in the README compilable and runnable (`cargo test --doc`).

## Testing

```sh
just test # all tests, including live integration tests
```

## Formatting

```sh
just fmt  # runs cargo +nightly fmt, taplo fmt, prettier, black, just --fmt
```
