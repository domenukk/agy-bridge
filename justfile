# Default recipe: format, lint, and test
default: fmt lint test

# ── Format ────────────────────────────────────────────────────────────

# Format all code (Rust, TOML, Markdown, Python, Justfile)
fmt: fmt-rust fmt-toml fmt-markdown fmt-python fmt-just

# Format Rust code (nightly required for latest style rules)
fmt-rust:
    cargo +nightly fmt

# Format TOML files
fmt-toml:
    taplo fmt

# Format Markdown files with prettier
fmt-markdown:
    npx -y prettier@latest --write '**/*.md'

# Format Python files with black
fmt-python:
    black .

# Format the justfile itself
fmt-just:
    just --fmt --unstable

# ── Lint ──────────────────────────────────────────────────────────────

# Lint all code (Rust clippy, Rust fmt, TOML, Markdown, Justfile, hygiene)
lint: lint-rust lint-rust-fmt lint-toml lint-markdown lint-just lint-hygiene

# Lint Rust with clippy
lint-rust:
    cargo clippy --all-targets -- -D warnings

# Lint Rust formatting
lint-rust-fmt:
    cargo +nightly fmt --check

# Lint TOML files
lint-toml:
    taplo check

# Lint Markdown files
lint-markdown:
    npx -y markdownlint-cli2@latest '**/*.md'

# Lint the justfile (check formatting)
lint-just:
    just --fmt --unstable --check

# Lint code hygiene (suppression patterns, structural issues)
lint-hygiene:
    python3 scripts/lint_hygiene.py

# ── Test ──────────────────────────────────────────────────────────────

# Run tests
test:
    cargo test

# ── Other ─────────────────────────────────────────────────────────────

# Run all checks (lint + test)
check: lint test
