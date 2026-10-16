#!/usr/bin/env python3
"""Hygiene linter — flags suppression patterns and structural issues.

Run via `just lint-hygiene` or directly: `python3 scripts/lint_hygiene.py`

Exit code 0 = clean, 1 = violations found.

Escape hatch: add `// NOLINT: <reason>` on the line ABOVE a flagged
pattern to suppress it.  The reason is mandatory.
Bare `// NOLINT` without a reason is itself flagged as a violation.
Stale NOLINTs (where the next line doesn't trigger any check) are also flagged.
"""

import re
import sys
from dataclasses import dataclass, field
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

# Suppression markers.
NOLINT_WITH_REASON = re.compile(r"//\s*NOLINT:\s*\S")
NOLINT_ANY = re.compile(r"//\s*NOLINT")
NOLINT_BARE = re.compile(r"//\s*NOLINT\s*$")

# Test-code classification (Rust).
TEST_PATH = re.compile(
    r"(^|/)tests/|(^|/)tests\.rs$|(^|/)tests_[^/]*\.rs$|_tests?\.rs$|(^|/)crates/agy-bridge-test-support/"
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


@dataclass
class Check:
    """A single hygiene check definition."""

    name: str
    pattern: re.Pattern[str]
    dirs: list[str]
    exts: set[str]
    message: str
    exclude: re.Pattern[str] | None = None  # lines matching this are skipped
    exclude_path: re.Pattern[str] | None = None  # file paths matching this are skipped
    suppressible: bool = True  # if False, NOLINT cannot suppress this check
    scope: Scope = Scope.ALL  # production-only / test-only restriction
    hits: list[str] = field(default_factory=list)
    suppressed: list[str] = field(default_factory=list)


CHECKS: list[Check] = [
    # ── Rust: hard errors (cannot be NOLINTed) ────────────────────────────
    Check(
        name="Rust: complexity lint suppression",
        pattern=re.compile(
            r"too_many_lines|too_many_arguments|type_complexity|cognitive_complexity"
        ),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="NEVER suppress complexity lints — refactor the function or type instead.",
        suppressible=False,
    ),
    Check(
        name="Rust: #[path = ...] module override",
        pattern=re.compile(r"#\[path\s*="),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message='NEVER use #[path = "..."] — use standard <module>/mod.rs directory hierarchy instead.',
        suppressible=False,
    ),
    Check(
        name="Rust: include!(...) outside OUT_DIR",
        pattern=re.compile(r"\binclude!\("),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="NEVER use include!() to stitch source files — use normal modules.",
        exclude=re.compile(r"OUT_DIR"),
        exclude_path=re.compile(r"crates/agy-bridge/src/proto/mod\.rs$"),
        suppressible=False,
    ),
    Check(
        name="Rust: untyped schemars::schema_for! conversion",
        pattern=re.compile(r"schemars::schema_for!"),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Use JsonSchema::from_schema::<T>() or #[llm_tool] instead of raw schemars::schema_for!.",
        exclude_path=re.compile(r"crates/agy-bridge/src/config/agent/mod\.rs$"),
        suppressible=False,
    ),
    Check(
        name="Rust: banned fastrand crate",
        pattern=re.compile(r"\bfastrand::"),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Use fast_rands instead of fastrand.",
        suppressible=False,
    ),
    # ── Rust: lint suppression ────────────────────────────────────────────
    Check(
        name="Rust: #[allow(...)]",
        pattern=re.compile(r"#\[allow\("),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Fix the underlying issue instead of suppressing the lint.",
    ),
    Check(
        name="Rust: #[expect(...)]",
        pattern=re.compile(r"#\[expect\("),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Fix the underlying issue instead of suppressing the lint.",
    ),
    # ── Rust: silently discarded values ───────────────────────────────────
    Check(
        name="Rust: let _ = (ignored Result/value)",
        pattern=re.compile(r"\blet _\s*="),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Handle the Result/value properly — don't silently discard it.",
    ),
    Check(
        name="Rust: if let Ok(...) (silent Err drop)",
        pattern=re.compile(r"\bif let Ok\("),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="The Err branch is silently ignored. Use match, map_err, or ? to handle errors.",
    ),
    Check(
        name="Rust: .unwrap_or_default() (hidden errors)",
        pattern=re.compile(
            r"\.unwrap_or_default\(\)|\.(unwrap_or_else|map_or_else)\(\s*(String::new|Vec::new|HashMap::new|Default::default)\b"
        ),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Silently replaces errors/None with defaults. Use an explicit match and log/propagate the error.",
    ),
    Check(
        name="Rust: .ok() (Result→Option, error discarded)",
        pattern=re.compile(r"\.ok\(\)"),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Converts Result to Option, silently discarding the error.",
    ),
    Check(
        name="Rust: .is_ok() / .is_err() (value discarded)",
        pattern=re.compile(r"\.(is_ok|is_err)\(\)"),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Checks the Result but discards the inner value. Use match or ? instead.",
        # Legitimate in assertions, conditions, and boolean expressions.
        exclude=re.compile(r"assert|if |while |\|\||&&|return .*\.is_"),
    ),
    Check(
        name="Rust: discarded closure arg |_|",
        pattern=re.compile(r"\|_\|"),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Closure discards its argument. Name it and use it (log, propagate, etc.).",
        # Skip doc-comments and string literals that mention closures.
        exclude=re.compile(r"^\s*///|^\s*//[^/].*\|_\||" + r'".*\|_\|'),
    ),
    Check(
        name="Rust: Err(_) (error value discarded in match)",
        pattern=re.compile(r"\bErr\(_\)"),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Error value is discarded in pattern match. Capture and log/propagate it.",
    ),
    Check(
        name="Rust: .unwrap_or(()) / .unwrap_or(0) / .unwrap_or(T::MAX) / .unwrap_or(Value::Null) (silent swallow)",
        pattern=re.compile(
            r"\.unwrap_or\(\s*(\(\)|\b0\b|false|true|\b[A-Z0-9_]*(ZERO|UNSET)[A-Z0-9_]*\b"
            r"|\b[iu](8|16|32|64|128|size)::(MAX|MIN)\b|\bf(32|64)::(MAX|MIN)\b"
            r"|(serde_json::)?Value::Null)\s*\)"
        ),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Silently swallows errors with a trivial/sentinel default. Handle the error explicitly.",
    ),
    Check(
        name="Rust: serde_json::json! in production code",
        pattern=re.compile(r"\bjson!\s*\("),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Untyped JSON grab-bag in production code. Use #[derive(Serialize)] structs (json! is allowed only in tests).",
        scope=Scope.PRODUCTION_ONLY,
    ),
    Check(
        name="Rust: test skip-guards (.ok()? / else { return; })",
        pattern=re.compile(r"\.ok\(\)\?|else\s*\{\s*return\s*;\s*\}"),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Tests must fail loudly, never silently skip. Use expect()/assert! instead.",
        scope=Scope.TEST_ONLY,
    ),
    # ── Rust: panic-at-runtime markers ────────────────────────────────────
    Check(
        name="Rust: todo!() / unimplemented!()",
        pattern=re.compile(r"\b(todo|unimplemented)!\("),
        dirs=RUST_DIRS,
        exts=RUST_EXTS,
        message="Will panic at runtime. Implement or return an error.",
    ),
    # ── Python: error & test hygiene ──────────────────────────────────────
    Check(
        name="Python: bare except or silent pass",
        pattern=re.compile(r"^\s*except\s*:|^\s*except\b[^:]*:\s*pass\b"),
        dirs=PYTHON_DIRS,
        exts=PYTHON_EXTS,
        message="Catch specific exceptions and handle/log them instead of bare except or pass.",
        exclude_path=re.compile(r"scripts/lint_hygiene\.py$"),
    ),
    Check(
        name="Python: inspect.getsource() in tests",
        pattern=re.compile(r"inspect\.getsource\("),
        dirs=PYTHON_DIRS,
        exts=PYTHON_EXTS,
        message="Test behavior, never source text via inspect.getsource().",
        exclude_path=re.compile(r"scripts/lint_hygiene\.py$"),
        suppressible=False,
    ),
]


# ── Helpers ──────────────────────────────────────────────────────────────────


def source_files(dirs: list[str], exts: set[str]) -> list[Path]:
    """Collect all source files under `dirs` matching `exts`."""
    files: list[Path] = []
    for d in dirs:
        root = Path(d)
        if not root.is_dir():
            continue
        for path in root.rglob("*"):
            if path.is_file() and path.suffix in exts:
                files.append(path)
    return sorted(files)


def is_nolinted(lines: list[str], lineno: int) -> bool:
    """Check if the line above `lineno` (1-indexed) has a valid NOLINT: reason."""
    if lineno < 2:
        return False
    prev_line = lines[lineno - 2]  # lineno 1-indexed, list 0-indexed
    return bool(NOLINT_WITH_REASON.search(prev_line))


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


def is_test_file(path: Path, gated_modules: list[Path]) -> bool:
    """Whether every line of `path` is test-only code."""
    if TEST_PATH.search(path.as_posix()):
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


def run_pattern_checks() -> bool:
    """Run all pattern checks. Returns True if any failed.

    Tracks which NOLINT comments are consumed (suppress a real hit) so we can
    detect stale ones afterwards.
    """
    failed = False
    # Collect all NOLINT locations and whether they were consumed.
    # Key: (path, lineno of NOLINT line), value: consumed?
    nolint_locations: dict[tuple[Path, int], bool] = {}

    # First pass: find all NOLINT lines.
    all_dirs_exts: set[tuple[str, str]] = set()
    for check in CHECKS:
        for d in check.dirs:
            for ext in check.exts:
                all_dirs_exts.add((d, ext))
    all_dirs_set = {d for d, _ in all_dirs_exts}
    all_exts_set = {e for _, e in all_dirs_exts}
    for path in source_files(list(all_dirs_set), all_exts_set):
        if path.as_posix().endswith("scripts/lint_hygiene.py"):
            continue
        try:
            lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        for lineno, line in enumerate(lines, start=1):
            if NOLINT_WITH_REASON.search(line):
                nolint_locations[(path, lineno)] = False  # not yet consumed

    # Second pass: run checks, marking consumed NOLINTs.
    gated_modules = cfg_test_module_paths(source_files(RUST_DIRS, RUST_EXTS))
    for check in CHECKS:
        print(f"=== {check.name} ===")
        files = source_files(check.dirs, check.exts)
        for path in files:
            if check.exclude_path and check.exclude_path.search(path.as_posix()):
                continue
            try:
                lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
            except OSError:
                continue
            whole_file_is_test = is_test_file(path, gated_modules)
            inline_test_start = first_inline_test_line(lines)
            for lineno, line in enumerate(lines, start=1):
                if not check.pattern.search(line):
                    continue
                is_test_line = whole_file_is_test or lineno >= inline_test_start
                if not line_in_scope(check.scope, is_test_line):
                    continue
                # Skip lines that are NOLINT comments — pattern matches in
                # the reason text are not actual code violations.
                if NOLINT_ANY.search(line):
                    continue
                if check.exclude and check.exclude.search(line):
                    continue
                if check.suppressible and is_nolinted(lines, lineno):
                    # Mark the NOLINT as consumed.
                    nolint_key = (path, lineno - 1)
                    if nolint_key in nolint_locations:
                        nolint_locations[nolint_key] = True
                    check.suppressed.append(f"  {path}:{lineno}: {line.strip()}")
                    continue
                check.hits.append(f"  {path}:{lineno}: {line.strip()}")
        if check.hits:
            for hit in check.hits:
                print(hit)
            print(f"^^^ {check.message}")
            failed = True
        else:
            print("  ✓ clean")
        if check.suppressed:
            for sup in check.suppressed:
                print(f"  ℹ  (suppressed) {sup.strip()}")

    # Check for stale NOLINTs (not consumed by any check).
    print("=== Stale NOLINT comments ===")
    stale: list[str] = []
    for (path, lineno), consumed in sorted(nolint_locations.items()):
        if not consumed:
            try:
                lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
                line_text = lines[lineno - 1].strip() if lineno <= len(lines) else "???"
            except OSError:
                line_text = "???"
            stale.append(f"  {path}:{lineno}: {line_text}")
    if stale:
        for s in stale:
            print(s)
        print("^^^ NOLINT comment doesn't suppress anything — remove it.")
        failed = True
    else:
        print("  ✓ clean")

    return failed


def run_bare_nolint_check() -> bool:
    """Flag NOLINT comments that don't include a reason."""
    print("=== Bare NOLINT (missing reason) ===")
    hits: list[str] = []
    for path in source_files(ALL_DIRS, ALL_EXTS):
        try:
            content = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for lineno, line in enumerate(content.splitlines(), start=1):
            if NOLINT_BARE.search(line):
                hits.append(f"  {path}:{lineno}: {line.strip()}")
    if hits:
        for hit in hits:
            print(hit)
        print("^^^ NOLINT must include a reason: // NOLINT: <why this is acceptable>")
        return True
    print("  ✓ clean")
    return False


def run_long_file_check() -> bool:
    """Flag files reaching or exceeding MAX_FILE_LINES."""
    print(f"=== Long files (>={MAX_FILE_LINES} lines) ===")
    long_files: list[tuple[Path, int]] = []

    for path in source_files(ALL_DIRS, ALL_EXTS):
        try:
            line_count = sum(1 for _ in path.open(encoding="utf-8", errors="replace"))
        except OSError:
            continue
        if line_count >= MAX_FILE_LINES:
            long_files.append((path, line_count))

    if long_files:
        long_files.sort(key=lambda x: -x[1])
        for path, count in long_files:
            print(f"  ⚠  {path} ({count} lines)")
        print(
            "^^^ Long file detected. Should be refactored to be modular "
            "and testable (with good testing) and a good folder structure."
        )
        return True

    print("  ✓ clean")
    return False


# ── Main ─────────────────────────────────────────────────────────────────────


def main() -> int:
    import os

    os.chdir(Path(__file__).resolve().parent.parent)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")

    failed = run_pattern_checks()
    failed = run_bare_nolint_check() or failed
    failed = run_long_file_check() or failed

    print()
    if failed:
        print("❌ Hygiene check failed — see above")
        return 1
    print("✅ All hygiene checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
