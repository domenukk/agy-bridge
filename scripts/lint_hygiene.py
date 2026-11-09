#!/usr/bin/env python3
"""Hygiene linter — flags suppression patterns and structural issues.

Run via `just lint-hygiene` or directly: `python3 scripts/lint_hygiene.py`

Exit code 0 = clean, 1 = violations found.

Zero-bypass policy: `#[allow(...)]`, `#![allow(...)]`, `#[expect(...)]`,
`#![expect(...)]`, `# noqa`, and `NOLINT` comments are unconditionally banned
across all production, build-script (`build.rs`), and test (`#[cfg(test)]` /
`tests/`) files. Fix the underlying issue instead of suppressing lints.
"""

from __future__ import annotations

import os
import re
import sys
from dataclasses import dataclass
from enum import Enum
from itertools import pairwise
from pathlib import Path

# ── Configuration ────────────────────────────────────────────────────────────

MAX_FILE_LINES = 1200

RUST_DIRS = ["crates/"]
RUST_EXTS = {".rs"}
PYTHON_DIRS = ["crates/", "scripts/"]
PYTHON_EXTS = {".py"}
ALL_DIRS = ["crates/", "scripts/"]
ALL_EXTS = {".rs", ".py"}
EXCLUDED_DIRS = {
    ".git",
    ".venv",
    "target",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
}

SELF_AND_HYGIENE_TEST_PATH = re.compile(
    r"(^|/)scripts/lint_hygiene\.py$|(^|/)test_lint_hygiene\.py$"
)

# Test-code classification (Rust).
TEST_PATH = re.compile(
    r"(^|/)tests/|(^|/)tests\.rs$|(^|/)tests_[^/]*\.rs$|_tests?\.rs$"
)
CFG_TEST_ATTR = re.compile(r"^\s*#\[cfg\(test\)\]\s*$")
CFG_TEST_MOD_DECL = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;")
CFG_TEST_MOD_INLINE = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")
MODULE_ROOT_FILES = {"mod.rs", "lib.rs", "main.rs"}


class Scope(Enum):
    """Which code a check applies to."""

    ALL = "all"
    PRODUCTION_ONLY = "production"
    TEST_ONLY = "test"


# ── Check definitions ────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Check:
    """A single hygiene check definition (zero bypasses)."""

    name: str
    pattern: re.Pattern[str]
    dirs: tuple[str, ...]
    exts: frozenset[str]
    message: str
    exclude: re.Pattern[str] | None = None
    exclude_path: re.Pattern[str] | None = None
    scope: Scope = Scope.ALL


CHECKS: tuple[Check, ...] = (
    # ── Zero-bypass lint & suppression bans ───────────────────────────────
    Check(
        name="Banned NOLINT comment",
        pattern=re.compile(r"(//|#)\s*NOLINT\b"),
        dirs=tuple(ALL_DIRS),
        exts=frozenset(ALL_EXTS),
        message="NOLINT suppressions are banned — fix the underlying issue instead.",
        exclude_path=SELF_AND_HYGIENE_TEST_PATH,
    ),
    Check(
        name="Rust: #[allow(...)] / #![allow(...)]",
        pattern=re.compile(r"#!?\s*\[\s*allow\s*\("),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Fix the underlying issue instead of suppressing the lint with #[allow(...)].",
    ),
    Check(
        name="Rust: #[expect(...)] / #![expect(...)]",
        pattern=re.compile(r"#!?\s*\[\s*expect\s*\("),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Fix the underlying issue instead of suppressing the lint with #[expect(...)].",
    ),
    Check(
        name="Rust: complexity lint suppression",
        pattern=re.compile(
            r"too_many_lines|too_many_arguments|type_complexity|cognitive_complexity"
        ),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="NEVER suppress complexity lints — refactor the function or type instead.",
    ),
    Check(
        name="Python: # noqa suppression",
        pattern=re.compile(r"#\s*noqa\b", re.IGNORECASE),
        dirs=tuple(PYTHON_DIRS),
        exts=frozenset(PYTHON_EXTS),
        message="Fix the Python lint issue instead of suppressing it with # noqa.",
        exclude_path=SELF_AND_HYGIENE_TEST_PATH,
    ),
    # ── Rust: structural invariants ───────────────────────────────────────
    Check(
        name="Rust: #[path = ...] module override",
        pattern=re.compile(r"#!?\s*\[\s*path\s*="),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message='NEVER use #[path = "..."] — use standard <module>/mod.rs directory hierarchy instead.',
    ),
    Check(
        name="Rust: include!(...) outside OUT_DIR",
        pattern=re.compile(r"\binclude!\("),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="NEVER use include!() to stitch source files — use normal modules.",
        exclude=re.compile(r"OUT_DIR"),
    ),
    Check(
        name="Rust: untyped schemars::schema_for! conversion",
        pattern=re.compile(r"schemars::schema_for!"),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Use JsonSchema::from_schema::<T>() or #[llm_tool] instead of raw schemars::schema_for!.",
        exclude_path=re.compile(r"(^|/)crates/agy-bridge/src/config/agent/mod\.rs$"),
    ),
    Check(
        name="Rust: banned fastrand crate",
        pattern=re.compile(r"\bfastrand::"),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Use fast_rands instead of fastrand.",
    ),
    # ── Rust: silently discarded values ───────────────────────────────────
    Check(
        name="Rust: let _ = (ignored Result/value)",
        pattern=re.compile(r"\blet _\s*="),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Handle the Result/value properly — don't silently discard it.",
    ),
    Check(
        name="Rust: if let Ok(...) (silent Err drop)",
        pattern=re.compile(r"\bif let Ok\("),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="The Err branch is silently ignored. Use match, map_err, or ? to handle errors.",
    ),
    Check(
        name="Rust: .unwrap_or_default() (hidden errors)",
        pattern=re.compile(
            r"\.unwrap_or_default\(\)|\.(unwrap_or_else|map_or_else)\(\s*(String::new|Vec::new|HashMap::new|Default::default)\b"
        ),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Silently replaces errors/None with defaults. Use an explicit match and log/propagate the error.",
    ),
    Check(
        name="Rust: .ok() (Result→Option, error discarded)",
        pattern=re.compile(r"\.ok\(\)"),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Converts Result to Option, silently discarding the error.",
    ),
    Check(
        name="Rust: .is_ok() / .is_err() (value discarded)",
        pattern=re.compile(r"\.(is_ok|is_err)\(\)"),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Checks the Result but discards the inner value. Use match or ? instead.",
        exclude=re.compile(r"assert|if |while |\|\||&&|return .*\.is_"),
    ),
    Check(
        name="Rust: discarded closure arg |_|",
        pattern=re.compile(r"\|_\|"),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Closure discards its argument. Name it and use it (log, propagate, etc.).",
        exclude=re.compile(r"^\s*///|^\s*//[^/].*\|_\||" + r'".*\|_\|'),
    ),
    Check(
        name="Rust: Err(_) (error value discarded in match)",
        pattern=re.compile(r"\bErr\(_\)"),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Error value is discarded in pattern match. Capture and log/propagate it.",
    ),
    Check(
        name="Rust: .unwrap_or(()) / .unwrap_or(0) / .unwrap_or(T::MAX) / .unwrap_or(Value::Null) (silent swallow)",
        pattern=re.compile(
            r"\.unwrap_or\(\s*(\(\)|\b0\b|false|true|\b[A-Z0-9_]*(ZERO|UNSET)[A-Z0-9_]*\b"
            r"|\b[iu](8|16|32|64|128|size)::(MAX|MIN)\b|\bf(32|64)::(MAX|MIN)\b"
            r"|(serde_json::)?Value::Null)\s*\)"
        ),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Silently swallows errors with a trivial/sentinel default. Handle the error explicitly.",
    ),
    Check(
        name="Rust: serde_json::json! in production code",
        pattern=re.compile(r"\bjson!\s*[\(\[\{]"),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Untyped JSON grab-bag in production code. Use #[derive(Serialize)] structs (json! is allowed only in tests).",
        exclude_path=re.compile(r"(^|/)examples/|(^|/)benches/|(^|/)mock/"),
        scope=Scope.PRODUCTION_ONLY,
    ),
    Check(
        name="Rust: test skip-guards (.ok()? / else { return; })",
        pattern=re.compile(r"\.ok\(\)\?|else\s*\{\s*return\s*;\s*\}"),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Tests must fail loudly, never silently skip. Use expect()/assert! instead.",
        scope=Scope.TEST_ONLY,
    ),
    # ── Rust: panic-at-runtime markers ────────────────────────────────────
    Check(
        name="Rust: todo!() / unimplemented!()",
        pattern=re.compile(r"\b(todo|unimplemented)!\("),
        dirs=tuple(RUST_DIRS),
        exts=frozenset(RUST_EXTS),
        message="Will panic at runtime. Implement or return an error.",
    ),
    # ── Python: error & test hygiene ──────────────────────────────────────
    Check(
        name="Python: bare except or silent pass",
        pattern=re.compile(r"^\s*except\s*:|^\s*except\b[^:]*:\s*pass\b"),
        dirs=tuple(PYTHON_DIRS),
        exts=frozenset(PYTHON_EXTS),
        message="Catch specific exceptions and handle/log them instead of bare except or pass.",
        exclude_path=SELF_AND_HYGIENE_TEST_PATH,
    ),
    Check(
        name="Python: inspect.getsource() in tests",
        pattern=re.compile(r"inspect\.getsource\("),
        dirs=tuple(PYTHON_DIRS),
        exts=frozenset(PYTHON_EXTS),
        message="Test behavior, never source text via inspect.getsource().",
        exclude_path=SELF_AND_HYGIENE_TEST_PATH,
    ),
)


# ── Helpers ──────────────────────────────────────────────────────────────────


def source_files(
    root_dir: Path, dirs: tuple[str, ...] | list[str], exts: frozenset[str] | set[str]
) -> list[Path]:
    """Collect all source files under `root_dir / d` matching `exts`."""
    files: list[Path] = []
    for d in dirs:
        sub = root_dir / d
        if not sub.is_dir():
            continue
        for path in sub.rglob("*"):
            rel_parts = path.relative_to(root_dir).parts
            if any(part in EXCLUDED_DIRS for part in rel_parts):
                continue
            if path.is_file() and path.suffix in exts:
                files.append(path)
    return sorted(files)


def module_dir(path: Path) -> Path:
    """Directory holding the child modules declared in `path`."""
    if path.name in MODULE_ROOT_FILES:
        return path.parent
    return path.parent / path.stem


def cfg_test_module_paths(rust_files: list[Path]) -> list[Path]:
    """Files/dirs of out-of-line modules declared under `#[cfg(test)]`."""
    gated: list[Path] = []
    for path in rust_files:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for attr_line, decl_line in pairwise(lines):
            if not CFG_TEST_ATTR.match(attr_line):
                continue
            decl = CFG_TEST_MOD_DECL.match(decl_line)
            if decl is None:
                continue
            child = module_dir(path) / decl.group(1)
            gated.extend([child.with_suffix(".rs"), child])
    return gated


def is_test_file(rel_posix: str, path: Path, gated_modules: list[Path]) -> bool:
    """Whether every line of `path` is test-only code."""
    if TEST_PATH.search(rel_posix):
        return True
    return any(path == gated or gated in path.parents for gated in gated_modules)


def first_inline_test_line(lines: list[str]) -> int:
    """1-indexed line where a top-level inline `#[cfg(test)] mod x { ... }` starts."""
    for index, (attr_line, decl_line) in enumerate(pairwise(lines), start=1):
        if CFG_TEST_ATTR.match(attr_line) and CFG_TEST_MOD_INLINE.match(decl_line):
            return index
    return len(lines) + 1


def line_in_scope(scope: Scope, is_test_line: bool) -> bool:
    """Whether a check with `scope` applies to a (test / production) line."""
    match scope:
        case Scope.ALL:
            return True
        case Scope.PRODUCTION_ONLY:
            return not is_test_line
        case Scope.TEST_ONLY:
            return is_test_line


# ── Check runners ────────────────────────────────────────────────────────────


def run_pattern_checks(root_dir: Path | None = None) -> bool:
    """Run all pattern checks under `root_dir` (defaults to CWD). Returns True if any failed."""
    base = root_dir if root_dir is not None else Path()
    failed = False

    gated_modules = cfg_test_module_paths(source_files(base, RUST_DIRS, RUST_EXTS))
    for check in CHECKS:
        print(f"=== {check.name} ===")
        hits: list[str] = []
        files = source_files(base, check.dirs, check.exts)
        for path in files:
            rel_path = path.relative_to(base)
            rel_posix = rel_path.as_posix()
            if check.exclude_path and check.exclude_path.search(rel_posix):
                continue
            try:
                lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
            except OSError:
                continue
            whole_file_is_test = is_test_file(rel_posix, path, gated_modules)
            inline_test_start = first_inline_test_line(lines)
            for lineno, line in enumerate(lines, start=1):
                if not check.pattern.search(line):
                    continue
                is_test_line = whole_file_is_test or lineno >= inline_test_start
                if not line_in_scope(check.scope, is_test_line):
                    continue
                if check.exclude and check.exclude.search(line):
                    continue
                hits.append(f"  {rel_posix}:{lineno}: {line.strip()}")
        if hits:
            for hit in hits:
                print(hit)
            print(f"^^^ {check.message}")
            failed = True
        else:
            print("  ✓ clean")

    return failed


def run_long_file_check(
    root_dir: Path | None = None, max_lines: int = MAX_FILE_LINES
) -> bool:
    """Flag files reaching or exceeding `max_lines`."""
    base = root_dir if root_dir is not None else Path()
    print(f"=== Long files (>={max_lines} lines) ===")
    long_files: list[tuple[str, int]] = []

    for path in source_files(base, ALL_DIRS, ALL_EXTS):
        rel_posix = path.relative_to(base).as_posix()
        try:
            with path.open(encoding="utf-8", errors="replace") as handle:
                line_count = sum(1 for _ in handle)
        except OSError:
            continue
        if line_count >= max_lines:
            long_files.append((rel_posix, line_count))

    if long_files:
        long_files.sort(key=lambda x: -x[1])
        for rel_posix, count in long_files:
            print(f"  ⚠  {rel_posix} ({count} lines)")
        print(
            "^^^ Long file detected. Should be refactored to be modular "
            "and testable (with good testing) and a good folder structure."
        )
        return True

    print("  ✓ clean")
    return False


def check_workspace(root_dir: Path) -> bool:
    """Run all hygiene checks on `root_dir`. Returns True if any violation is found."""
    failed = run_pattern_checks(root_dir)
    failed = run_long_file_check(root_dir) or failed
    return failed


# ── Main ─────────────────────────────────────────────────────────────────────


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    os.chdir(repo_root)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")

    failed = check_workspace(repo_root)

    print()
    if failed:
        print("❌ Hygiene check failed — see above")
        return 1
    print("✅ All hygiene checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
