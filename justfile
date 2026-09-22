# agy-bridge task runner.
#
# This file is the single source of truth for every lint/format/test command.
# CI (.github/workflows/ci.yml) invokes these recipes rather than re-spelling
# the commands, so the two cannot drift.
#
# ── Environment variables ─────────────────────────────────────────────────
#
#   AGY_BRIDGE_SKIP_LIVE_TESTS       Set to any value to skip every test that
#                                    talks to the real Gemini API. Required for
#                                    an offline/CI run — without it the live
#                                    tests call `common::api_key()`, which
#                                    *panics* when no key is configured.
#                                    `just test-offline` sets it for you.
#   GEMINI_API_KEY                   Key used by the live tests. Read from the
#                                    environment or from ./.env.
#   GEMINI_API_BASE_URL              Proxy mode; stands in for GEMINI_API_KEY.
#   AGY_BRIDGE_MAX_CONCURRENT_TESTS  How many live tests may hit the API at
#                                    once (default: see tests/common/mod.rs).
#   AGY_BRIDGE_TEST_TIMEOUT_SECS     Per-test wall-clock timeout for the
#                                    mock-server tests (default 120).
#   RUST_TEST_THREADS                libtest parallelism. Defaulted to 4 in
#                                    .cargo/config.toml so a test binary cannot
#                                    hold one bridge per CPU core.
#   RUST_LOG                         tracing filter for `test-live-logged`.
#
# NOTE: even `test-offline` needs network access on a cold build — the crate's
# build.rs downloads the `localharness` wheel from PyPI for the native backend.

set windows-shell := ["bash.exe", "--norc", "-cu"]

# Default recipe: format, lint, and test
default: fmt lint test

# ── Format ────────────────────────────────────────────────────────────

# Format all code (Rust, TOML, Markdown, Python, Justfile)
fmt: fmt-rust fmt-toml fmt-markdown fmt-python fmt-just

# Format Rust code (nightly required for latest style rules)
fmt-rust:
    if cargo +nightly fmt --version >/dev/null 2>&1; then cargo +nightly fmt; else cargo fmt; fi

# Format TOML files
fmt-toml:
    taplo fmt

# Format Markdown files with prettier
fmt-markdown:
    if command -v npx >/dev/null 2>&1; then npx -y prettier@latest --write '**/*.md'; fi

# Format Python files with black
fmt-python:
    uv run black .

# Format the justfile itself
fmt-just:
    just --fmt --unstable

# ── Lint ──────────────────────────────────────────────────────────────

# Lint all code (Rust clippy, Rust fmt, TOML, Markdown, Justfile, hygiene)
lint: lint-rust lint-rust-fmt lint-toml lint-markdown lint-just lint-hygiene

# Lint Rust with clippy across all feature combinations
lint-rust:
    cargo clippy --all-targets --all-features -- -D warnings
    cargo clippy --all-targets --no-default-features --features native -- -D warnings
    cargo clippy --all-targets --no-default-features --features python -- -D warnings

# Lint Rust formatting
lint-rust-fmt:
    if cargo +nightly fmt --version >/dev/null 2>&1; then cargo +nightly fmt --check; else cargo fmt --check; fi

# Lint TOML files
lint-toml:
    taplo check

# Lint Markdown files
lint-markdown:
    if command -v npx >/dev/null 2>&1; then npx -y markdownlint-cli2@latest '**/*.md'; fi

# Lint the justfile (check formatting)
lint-just:
    just --fmt --unstable --check

# Lint code hygiene (suppression patterns, structural issues)
lint-hygiene:
    uv run python scripts/lint_hygiene.py

# ── Test ──────────────────────────────────────────────────────────────

# Run all tests (Rust python backend + native backend + default features + md-tmpl + Python)
test: test-rust test-native test-default test-md-tmpl test-python

# Run Rust tests for python backend
test-rust:
    cargo test -p agy-bridge --no-default-features --features python

# Run Rust tests for native pure-Rust backend
test-native:
    cargo test -p agy-bridge --no-default-features --features native

# Run the whole workspace with the crates' DEFAULT feature set.
#
# This is the feature set downstream users get, and it is the only recipe that
# also builds/executes the doctests in README.md (which is included as crate
# rustdoc). `test-native` deliberately does not cover it: it pins `-p

# agy-bridge --no-default-features`.
test-default:
    cargo test --workspace

# Run tests and example tests for the md-tmpl feature
test-md-tmpl:
    cargo test -p agy-bridge --features md-tmpl --test md_tmpl_test
    cargo test -p agy-bridge --features md-tmpl --example getting_started_template_tools

# Run Python tests for the embedded agent_init helpers
test-python:
    uv run pytest crates/agy-bridge/tests/python -q

# Run the full suite offline: no API key, no calls to the real Gemini API.
#
# Every live test short-circuits on AGY_BRIDGE_SKIP_LIVE_TESTS; the mock-server

# tests run for real against local TCP listeners.
test-offline:
    AGY_BRIDGE_SKIP_LIVE_TESTS=1 "{{ just_executable() }}" test

# Run tests with the bridge's tracing logs enabled, teeing everything to a
# timestamped file under test-logs/ so failures can be diagnosed after the fact.

# Override verbosity with RUST_LOG, e.g. `RUST_LOG=agy_bridge=trace just test-live-logged`.
test-live-logged:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p test-logs
    log="test-logs/live-$(date +%Y%m%d-%H%M%S).log"
    echo "Logging to ${log}"
    # Runs MULTI-THREADED, but bounded: RUST_TEST_THREADS is defaulted in
    # .cargo/config.toml so concurrent multi-bridge/multi-agent use cannot
    # scale with the core count. Live *API* concurrency is bounded separately
    # by the semaphore in tests/common/mod.rs.
    # Override the API-concurrency limit with AGY_BRIDGE_MAX_CONCURRENT_TESTS.
    RUST_LOG="${RUST_LOG:-agy_bridge=debug}" cargo test --tests -- --nocapture 2>&1 | tee "${log}"

# ── Other ─────────────────────────────────────────────────────────────

# Run all checks (lint + test)
check: lint test
