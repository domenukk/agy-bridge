# AGENTS.md — agy-bridge

> **Living Design & Engineering Standards**
>
> - **`README.md` (`crates/agy-bridge/src/lib.rs` crate rustdoc)** is the
>   normative user-facing specification and API reference for `agy-bridge`.
> - **`AGENTS.md` (and `AGENT.md` symlink)** is the **enforceable operational
>   summary** of architecture, domain invariants, and multi-language code
>   quality standards.
> - **`scripts/lint_hygiene.py` and `justfile`** are the **Single Source of
>   Truth** for automated hygiene, formatting, linting, and test gates. Every
>   rule in `AGENTS.md` must have a corresponding automated check in
>   `scripts/lint_hygiene.py` or `justfile`.

Rust SDK for building LLM agents with tools, lifecycle hooks, declarative
policies, streaming, subagents, workflows, and multimodal input on top of the
[Google Antigravity SDK](https://github.com/Google-Antigravity/antigravity-sdk-python)
(`localharness`).

---

## 1. Architecture & Single Source of Truth (SSOT)

`agy-bridge` exposes a single unified Rust API (`AgyBridge`, `AgentConfig`,
`AgentHandle`, `ToolRegistry`, `Hooks`, `PolicySet`) backed by two
interchangeable execution runtimes implementing the internal `Runtime` trait:

```text
                          ┌─▶ NativeRuntime ──▶ stdio protobuf + WebSocket JSON
                          │   (feature "native", default)            │
Rust caller ──▶ AgentHandle                                      localharness
                          │
                          └─▶ PythonRuntime ──▶ PyO3 ──▶ google-antigravity SDK
                              (feature "python", dedicated thread)   │
                                                                 localharness
```

### Workspace Crates & Directory Layout

- `crates/agy-bridge/` — Primary published library crate (`agy-bridge`):
  - `build.rs` — Compiles `proto/{content,skill,localharness}.proto` via
    `prost-build` + `pbjson-build` and provisions the version-pinned
    `localharness` binary (`TARGET_ANTIGRAVITY_SDK_VERSION = "0.1.22"`) into
    `~/.gemini/antigravity/bin/`.
  - `src/agent/` — `AgentHandle`, `AgentBuilder`, `AgentId`, and `MockRuntime`
    lifecycle (`create` → `chat` / `send` → `shutdown` / `disconnect` /
    `delete`).
  - `src/config/` — Strongly-typed builders and serde models (`AgentConfig`,
    `CapabilitiesConfig`, `CompactionConfig`, `GeminiConfig`, `ModelConfig`,
    `RetryConfig`, `BudgetConfig`, `InlineSkill`, `SubagentConfig`,
    `SubagentSkillsConfig`, `ToolOutputTruncationConfig`, `McpServer`,
    `JsonSchema`).
  - `src/content/` — Multimodal content primitives (`Content`, `Media`, MIME
    detection, and base64 encoding).
  - `src/hooks/` — Lifecycle callback types (`Hooks`, `PreTurn`, `PostTurn`,
    `PreToolCallDecide`, `TransformToolInput`, `PostToolCall`, `OnToolError`,
    `OnSessionStart`, `OnSessionEnd`, `OnCompaction`, `Stop`).
  - `src/policies/` — Declarative tool execution policies (`PolicyRule`,
    `PolicySet`, `PolicyConfirmationHandler`, `WorkspaceOnly` containment).
  - `src/runtime/native/` — Pure-Rust `NativeRuntime`: child process management
    (`process.rs`), length-delimited protobuf handshake, WebSocket JSON event
    loop (`ws.rs`), protobuf config translation (`config/`), and step/hook/tool
    dispatch (`events/`, `tools.rs`, `session.rs`).
  - `src/runtime/` (`feature = "python"`) — `PythonRuntime` command loop over
    `mpsc` to a dedicated Python interpreter thread (`handlers/`, `streaming/`,
    `venv.rs`, `ffi_dispatch.rs`, and embedded helpers in `src/runtime/py/`).
  - `src/streaming/` — Triple-channel subscription-gated fan-out
    (`ChatResponseHandle`, `ChatResponseWriter`, `ResponseEvent`,
    `StreamChunk`, `ToolCallEvent`).
  - `src/tools/` — Re-exports `#[llm_tool]`, `ToolRegistry`, `ToolContext`,
    `ToolEffect`, `SandboxStatus`, and prompt-injection output quarantine
    utilities from `llm-tool`.
  - `src/triggers.rs` — Periodic and filesystem-watch (`notify`) triggers.
  - `src/error.rs` & `src/types/` — Typed `Error` classification and domain
    models (`Step`, `UsageMetadata`, `WorkflowProgress`, `ServiceTier`).
- `crates/agy-bridge-test-support/` — Internal (`publish = false`) mock Gemini
  HTTP/SSE server (`MockGeminiServer`) and shared test runtime helpers.
- `scripts/lint_hygiene.py` — Automated structural and error-handling hygiene
  gate.

---

## 2. Golden Rules (The 12 Non-Negotiable Engineering Pillars)

1. **No Unjustified Lint Suppressions at Any Scope**:
   - Fix the underlying code rather than silencing warnings. File- or
     module-level suppressions (`#![allow(...)]`, `#![expect(...)]`,
     `# flake8: noqa`, `# type: ignore` blanket headers) and `lint_hygiene.py`
     path allowlists are forbidden (except on generated protobuf bindings in
     `src/proto/mod.rs`).
   - **Escape hatch**: Only when a check is a genuine, locally unavoidable
     false positive, place `// NOLINT: <specific reason>` on the line
     immediately above the item. Bare `// NOLINT` without a reason and stale
     `// NOLINT` comments fail `scripts/lint_hygiene.py`.
   - **Non-suppressible structural lints (`suppressible=False`)**: Never
     suppress `clippy::too_many_lines`, `clippy::too_many_arguments`,
     `clippy::type_complexity`, `clippy::cognitive_complexity`, or
     `#[path = "..."]` module overrides. Refactor functions into helpers,
     introduce context structs, and use `<module>/mod.rs` + `<module>/tests.rs`
     directory hierarchies.
2. **No Hardcoded Magic Values, Machine Paths, or Fake Telemetry**:
   - Define protocol constants, wire enum values, environment variable names,
     default models, and timeouts as named constants (`pub const` / `const` in
     Rust, `Final` in Python).
   - Never hardcode developer-specific paths (`/usr/local/google/home/...`,
     `/Users/...`) in source, tests, or documentation.
   - Never fabricate telemetry: represent unmeasured token counts or absent
     structured output as `Option::None`, and fail loudly on invalid or missing
     configuration.
3. **Zero Library Panics & Zero Swallowed Errors or Ignored Return Values**:
   - Library code returns typed `Result<T, Error>` (`src/error.rs`) and must
     never panic (`todo!()`, `unimplemented!()`, or unchecked `.unwrap()` in
     production paths).
   - **Never discard return values**: `let _ = ...` in Rust and `_ = fn()` in
     Python are banned unless explicitly justified with `// NOLINT: <reason>`.
   - **Never silently swallow errors**: `.ok()`, `.is_ok()` / `.is_err()`
     outside boolean conditions/assertions, `.unwrap_or_default()`,
     `.unwrap_or(())`, `.unwrap_or(0)`, `.unwrap_or(false)`,
     `.unwrap_or(T::MAX)`, `.unwrap_or(serde_json::Value::Null)`,
     `.map_or_else(String::new, ...)`, `Err(_) => ...`, `if let Ok(...)`
     without handling `Err`, discarded closure arguments `|_|`, and bare
     `except:` / `except Exception: pass` in Python are banned. Propagate
     errors with `?` or explicitly bind and log them via `tracing::warn!` /
     `tracing::debug!`. Never evade `lint_hygiene.py` regexes by extracting
     `0` into a constant passed to `Result::unwrap_or`.
4. **Strict File Length (`< 1,200` Lines), Function Size (`<= 100` Lines), and
   Standard Module Hierarchy**:
   - Every `.rs`, `.py`, and `.md` file must stay strictly below **1,200 lines**
     (`MAX_FILE_LINES = 1200` in `scripts/lint_hygiene.py`, target `< 800`
     lines).
   - Every Rust function must stay `<= 100` lines (`clippy::too_many_lines`).
   - Always use standard `<module>/mod.rs` and `<module>/tests.rs` directory
     hierarchies for multi-file modules; never use `#[path = "..."]` or
     `include!(...)` outside `OUT_DIR`.
5. **Strong Typing & Structured Schemas Across Rust and Python**:
   - Prefer domain enums (`ThinkingLevel`, `ServiceTier`, `AgentBehavior`,
     `SubagentSkillsConfig`, `PolicyRule`, `BuiltinTools`) over raw strings or
     bare booleans.
   - Construct JSON schemas via `JsonSchema::from_schema::<T>()` or
     `#[llm_tool]` (`llm_tool::RustTool`), never
     `serde_json::to_value(schemars::schema_for!(...))`.
   - Never build untyped JSON with `serde_json::json!` in production library,
     handler, or example code; define `#[derive(Serialize)]` structs instead
     (`json!` is allowed only in `#[cfg(test)]` modules, `tests/`, and
     `agy-bridge-test-support`).
   - Use `fast_rands` for non-cryptographic random number generation in tests
     and jitter helpers, never `fastrand`.
6. **Single Source of Truth in Rust & Full Runtime Parity**:
   - Keep `NativeRuntime` (`feature = "native"`, default) and `PythonRuntime`
     (`feature = "python"`) in behavioral parity across `AgentConfig` options
     (`initial_history`, `inline_skills`, `subagents`, `compaction_config`,
     `capabilities`, `retry_config`, `budget`, `policies`, `hooks`, `tools`).
   - When `md-tmpl` (`feature = "md-tmpl"`) is used, keep prompt and tool
     templates in `.tmpl.md` files with multiline YAML frontmatter and
     `untrusted str` / `sanitize` defenses for untrusted inputs.
7. **Boundary & Identifier Normalization**:
   - Normalize MCP server names (`sanitize_mcp_server_name`), environment keys,
     and workspace paths consistently at configuration boundaries.
8. **Zero-Overhead Streaming & Subscription-Gated Fan-Out**:
   - Preserve subscription-gated non-blocking fan-out in `ChatResponseWriter`
     so unconsumed streams (`text_tx`, `thought_tx`, `tool_call_tx`,
     `chunk_tx`, `event_tx`, `step_tx`) never fill bounded buffers or deadlock
     active turns.
9. **No Dead Code, Stubs, or Orphaned Doc Comments**:
   - Every public struct, enum variant, builder method, and helper must be
     wired and tested. Remove dead code immediately and verify no `///` doc
     comment is left stranded after edits.
   - `"Pre-existing"` is never an excuse for failing tests, clippy warnings, or
     hygiene violations.
10. **Mandatory Red-Before-Green (`Gate R`) & Behavioral Testing**:
    - Test real behavior against `MockRuntime`, `MockGeminiServer`, and the
      real `localharness` binary (`tests/v017_features_test.rs`), never source
      text inspection (`inspect.getsource()`).
    - Tests fail loudly: no silent skip-guards (`.ok()?`, `else { return; }`)
      in test code; use `expect(...)` / `assert!` instead.
    - Keep `README.md` code examples compilable as doctests (`cargo test --doc`
      via `lib.rs` `#![doc = include_str!("../README.md")]`).
11. **Host Load, RAM Budget (`< 1 GB` Peak RSS), & Process Cleanup**:
    - Tests must stay under **1 GB peak RSS**, run with bounded threads
      (`RUST_TEST_THREADS=4` in `.cargo/config.toml`), and reuse shared bridges
      where appropriate.
    - Every spawned `localharness` child process and background task must be
      terminated and reaped deterministically on `shutdown()` / `Drop`.
12. **Version Control Safety & Script Hygiene**:
    - **NEVER push (`git push`) or commit (`git commit`) without explicit user
      instruction.**
    - **NEVER destroy or revert uncommitted work** (`git checkout --`,
      `git restore`, `git reset --hard`, `git clean`, `git stash drop` are
      forbidden unless explicitly requested by the user).
    - Place temporary one-off scripts in `/tmp` and delete them after use.

---

## 3. Multi-Language Standards Matrix

| Language             | Typing & Data Modeling                                                                                                                   | Error & Resource Handling                                                                                                                                      | Linter / Formatter / Gate                                                                                                                                                                                    |
| :------------------- | :--------------------------------------------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------- | :----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Rust**             | Typed config builders (`TypedBuilder`), domain enums over `String`/`bool`, `#[llm_tool]`, `JsonSchema::from_schema::<T>()`, `fast_rands` | Typed `Error` (`thiserror`), zero `todo!()`/`unimplemented!()`/`let _ =`/`.ok()`/`.unwrap_or_default()` without `// NOLINT:`, deterministic child process reap | `cargo +nightly fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc`                                                                             |
| **Python**           | Type annotations (`from __future__ import annotations`), `mypy --strict` on tooling, `google-antigravity>=0.1.22`                        | Specific exception handling, zero bare `except:` or silent `except Exception: pass`, zero `inspect.getsource()`                                                | `uv run black --check .`, `uv run ruff check .`, `uv run mypy --strict scripts/lint_hygiene.py`, `uv run mypy .`, `uv run python scripts/lint_hygiene.py`, `uv run pytest crates/agy-bridge/tests/python -q` |
| **TOML / MD / Just** | Workspace lint inheritance in `Cargo.toml`, synced `pyproject.toml` & `build.rs` SDK version                                             | Validated formatting and link structure across all manifests and docs                                                                                          | `taplo check`, `npx markdownlint-cli2 '**/*.md'`, `just --fmt --unstable --check`                                                                                                                            |

---

## 4. Verification & Release Gates

Run all formatting, linting, hygiene, documentation, and offline test gates via
`just`:

```sh
just fmt          # cargo +nightly fmt, taplo fmt, prettier, black, just --fmt
just lint         # clippy (all-features, native, python), rustfmt, taplo, markdownlint, black --check, ruff check, mypy --strict, mypy, justfile, scripts/lint_hygiene.py
just test-offline # full offline test suite (native + python + default doctests + md-tmpl + pytest)
just test         # full test suite including live Gemini API integration tests
```
