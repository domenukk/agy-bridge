"""Adversarial Gate R tests for `scripts/lint_hygiene.py`.

Verifies that `lint_hygiene.py` unconditionally rejects Rust outer/inner lint
suppression attributes, `# noqa`, and inline lint-bypass comments across
`build.rs`, production Rust modules, `#[cfg(test)]` blocks, integration tests,
and Python files — with zero bypasses — and passes on the real workspace.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[4]
LINT_SCRIPT = REPO_ROOT / "scripts" / "lint_hygiene.py"

OUTER_ALLOW = "#" + "[allow"
INNER_ALLOW = "#!" + "[allow"
OUTER_EXPECT = "#" + "[expect"
BYPASS_MARKER = "NOL" + "INT"
JSON_MACRO = "serde_json::" + "json!"


def _load_lint_module():
    spec = importlib.util.spec_from_file_location("agy_lint_hygiene", LINT_SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Unable to load lint module from {LINT_SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["agy_lint_hygiene"] = module
    spec.loader.exec_module(module)
    return module


LINT = _load_lint_module()


@pytest.mark.parametrize(
    ("rel_path", "content"),
    [
        (
            "crates/demo/build.rs",
            f"// {BYPASS_MARKER}: build script wrapper\n{OUTER_ALLOW}(clippy::unnecessary_wraps)]\nfn main() {{}}\n",
        ),
        (
            "crates/demo/src/lib.rs",
            f"{INNER_ALLOW}(dead_code)]\npub fn f() {{}}\n",
        ),
        (
            "crates/demo/src/triggers.rs",
            f"pub fn ok() {{}}\n\n#[cfg(test)]\n{OUTER_ALLOW}(deprecated)]\nmod tests {{\n    #[test]\n    fn t() {{}}\n}}\n",
        ),
        (
            "crates/demo/src/caps.rs",
            f"// {BYPASS_MARKER}: legacy alias\npub const X: u32 = 1;\n",
        ),
        (
            "crates/demo/src/exp.rs",
            f"{OUTER_EXPECT}(clippy::unused_async)]\npub async fn f() {{}}\n",
        ),
        (
            "crates/demo/src/helper.py",
            "def f() -> int:\n    return 1  # noqa: F401\n",
        ),
        (
            "crates/demo/src/wire.rs",
            f'pub fn bad() -> serde_json::Value {{\n    {JSON_MACRO}({{"ok": true}})\n}}\n',
        ),
        (
            "crates/agy-bridge-test-support/src/lib.rs",
            f'pub fn bad_support() -> String {{\n    {JSON_MACRO}({{"models": []}}).to_string()\n}}\n',
        ),
    ],
)
def test_gate_r_rejects_suppressions_and_nolint_everywhere(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], rel_path: str, content: str
) -> None:
    target = tmp_path / rel_path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")

    failed = LINT.check_workspace(tmp_path)
    captured = capsys.readouterr().out
    assert failed is True, f"Expected violation in {rel_path}, got output:\n{captured}"
    assert rel_path in captured


def test_gate_r_rejects_oversized_files(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    big = tmp_path / "crates" / "demo" / "src" / "big.rs"
    big.parent.mkdir(parents=True, exist_ok=True)
    big.write_text("pub const X: usize = 1;\n" * 1200, encoding="utf-8")

    failed = LINT.check_workspace(tmp_path)
    captured = capsys.readouterr().out
    assert failed is True
    assert "crates/demo/src/big.rs" in captured


def test_real_workspace_passes_zero_bypass_hygiene_gate() -> None:
    assert LINT.check_workspace(REPO_ROOT) is False
