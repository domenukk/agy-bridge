# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] — 2026-06-21

### Added

- **Live hook integration tests** for `OnToolError`, `TransformToolInput`,
  `OnSessionStart`/`OnSessionEnd`, and `OnInteraction` hook points.
- **`SessionContext` serde roundtrip** — `started_at` now serializes and
  deserializes correctly (was silently lost before).
- **`#[non_exhaustive]`** on all public enums likely to grow: `Error`,
  `HookPoint`, `HookCallback`, `PolicyRule`, `PolicyDecision`, `TriggerConfig`,
  `McpServer`, `BuiltinTools`, `ContentPrimitive`, `Content`, `ResponseEvent`,
  `StreamChunk`, and `OnCompactionContext`.
- **`Content::text()` constructor**, `is_text()`, and `as_text()` accessors.
- **Media factory methods**: `Image::png()`, `Image::jpeg()`, `Document::pdf()`,
  `Audio::mp3()`, `Video::mp4()`, `from_file()`, `with_description()`.
- **47 new media tests** covering constructors, MIME types, file loading.
- **8 path canonicalization tests** in `policies/path.rs`.
- **`Content::Video` serde test** in `content/serialization.rs`.
- **Exhaustive hook runner tests** including panic recovery, duplicate
  replacement, and observer patterns.
- Interactive stdin loops for `human_in_the_loop` and `interactive_cli` examples.

### Changed

- **BREAKING**: `SessionContext::started_at` changed from `std::time::Instant` to
  `std::time::SystemTime`. `SystemTime` is serializable and survives serde
  roundtrips; `Instant` was silently zeroed on deserialization.
- **BREAKING**: `OnCompactionContext` is now `#[non_exhaustive]`.
- `async_ops.rs` refactored via `run_py_async_op()` generic helper — reduced
  from 878 to 581 lines with identical behavior.
- `hooks/runner.rs` refactored via `run_observer()` — 7 duplicate observer
  methods collapsed to one generic helper.
- Deep-dive examples now attach hooks via `.hooks()` (previously configured
  but never attached).
- `multimodal.rs` example uses `CARGO_MANIFEST_DIR` instead of relative paths.

### Fixed

- 2 broken intra-doc links to `ChatResponseHandle` in `streaming/types.rs`.
- `schemars` upgraded from v0.8 to v1; `SchemaGenerator` API calls updated.
- Unsafe environment variable manipulation removed from `agent.rs` tests.
- Redundant closures replaced with method references (`clippy::redundant_closure`).
- `let...else` patterns used where clippy expects them in examples.
- `String::new()` used instead of `"".to_owned()` in triggers tests.

### Removed

- Conflicting `TryFrom` trait implementations in `policies/rules.rs` and
  `triggers.rs`; replaced with `validated_from()` / `try_from_iter()`.

## [0.1.4] — 2026-06-20

### Added

- `PostToolCallContext` now exposes tool metadata.
- Integration test for tool metadata parsing.

## [0.1.1] — 2026-06-19

Initial release with core agent lifecycle, hooks, policies, streaming,
custom tools, and MCP server support.

[0.2.0]: https://github.com/domenukk/agy-bridge/compare/0.1.4...HEAD
[0.1.4]: https://github.com/domenukk/agy-bridge/compare/0.1.1...0.1.4
[0.1.1]: https://github.com/domenukk/agy-bridge/releases/tag/0.1.1
