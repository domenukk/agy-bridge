"""Regression tests for v0.16.8 security hardening and google-antigravity 0.1.24 parity."""

from __future__ import annotations

import asyncio
import json
import os
import sys
import types
from typing import Any

import pytest

sys.path.insert(
    0, os.path.join(os.path.dirname(__file__), "..", "..", "src", "runtime", "py")
)

import agent_init
import sdk_proto_parity


def test_sdk_0124_default_model_and_thinking_level_xhigh() -> None:
    from google.antigravity import models as sdk_models
    from google.antigravity import types as sdk_types

    assert sdk_models.DEFAULT_MODEL == "gemini-4-hydrogen"
    assert sdk_types.ThinkingLevel.EXTRA_HIGH.value == "xhigh"

    target_xhigh = agent_init._build_model_target(
        {
            "name": "gemini-4-hydrogen",
            "generation": {"thinking_level": "xhigh"},
        }
    )
    assert (
        target_xhigh.endpoint.options.thinking_level
        == sdk_types.ThinkingLevel.EXTRA_HIGH
    )

    target_legacy = agent_init._build_model_target(
        {
            "name": "gemini-4-hydrogen",
            "generation": {"thinking_level": "extra_high"},
        }
    )
    assert (
        target_legacy.endpoint.options.thinking_level
        == sdk_types.ThinkingLevel.EXTRA_HIGH
    )


def test_ask_user_fails_closed_without_blocking_console_input(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from google.antigravity import types as sdk_types

    def _forbid_input(_prompt: str = "") -> str:
        raise AssertionError("console input() must never be called")

    monkeypatch.setattr("builtins.input", _forbid_input)
    monkeypatch.delitem(sys.modules, "_agy_bridge_globals", raising=False)

    cfg: dict[str, Any] = {
        "policies": [{"AskUser": {"tool": "run_command", "handler_id": "h1"}}]
    }
    agent_init._wire_policies(cfg, 77)
    ask_policy = cfg["policies"][0]
    assert ask_policy.ask_user is not None

    tc = sdk_types.ToolCall(name="run_command", args={"command_line": "id"})
    approved = asyncio.run(ask_policy.ask_user(tc))
    assert approved is False

    # Also fail closed when dispatch_rust_policy_confirm raises RuntimeError (no handler registered).
    globals_mod = types.ModuleType("_agy_bridge_globals")

    def _raise_missing(_aid: int, _name: str, _args: str) -> bool:
        raise RuntimeError("No active AskUserHandler")

    globals_mod.dispatch_rust_policy_confirm = _raise_missing  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "_agy_bridge_globals", globals_mod)
    approved_err = asyncio.run(ask_policy.ask_user(tc))
    assert approved_err is False


def test_workspace_only_policy_wired_into_sdk_and_harness_proto() -> None:
    from google.antigravity.connections.local.local_connection_config import (
        LocalAgentConfig,
    )

    cfg: dict[str, Any] = {
        "policies": [
            {"WorkspaceOnly": ["/tmp/ws_alpha"]},
            "AllowAll",
        ]
    }
    agent_init._wire_policies(cfg, 88)
    assert "/tmp/ws_alpha" in cfg["workspaces"]
    assert any(getattr(p, "name", "") == "workspace_only" for p in cfg["policies"])

    lac = LocalAgentConfig(**cfg)
    strategy = lac.create_strategy(tool_runner=None, hook_runner=None)
    hc = strategy._build_harness_config()
    rule_names = [r.name for r in hc.policy_config.rules]
    assert "workspace_only" in rule_names


def test_hooks_deduplicate_same_point_forward_modified_args_and_key_by_call_id(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from google.antigravity import types as sdk_types
    from google.antigravity.hooks import hook_runner as hr_mod

    globals_mod = types.ModuleType("_agy_bridge_globals")
    dispatch_calls: list[tuple[str, dict[str, Any]]] = []

    async def _fake_dispatch(agent_id: int, point: str, ctx_json: str) -> str:
        assert agent_id == 99
        parsed = json.loads(ctx_json)
        dispatch_calls.append((point, parsed))
        if point == "pre_tool_call_decide":
            if parsed.get("name") == "tool_a":
                return json.dumps(
                    {
                        "allow": True,
                        "message": "",
                        "transformed_args": {"x": 100, "injected": True},
                    }
                )
            return json.dumps({"allow": True, "message": "", "transformed_args": None})
        return ""

    globals_mod.dispatch_rust_hook = _fake_dispatch  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "_agy_bridge_globals", globals_mod)

    # Simulate both TransformToolInput and PreToolCallDecide + PostToolCall registered.
    cfg: dict[str, Any] = {
        "hooks": [
            {"name": "transform_1", "point": "PreToolCallDecide", "callback_id": "t1"},
            {"name": "decide_1", "point": "PreToolCallDecide", "callback_id": "d1"},
            {"name": "post_1", "point": "PostToolCall", "callback_id": "p1"},
        ]
    }
    agent_init._wire_hooks(cfg, 99)
    # Must register exactly 1 pre_tool_call_decide and 1 post_tool_call Python callback!
    assert len(cfg["hooks"]) == 2

    runner = hr_mod.HookRunner(hooks=cfg["hooks"])

    async def _exercise() -> None:
        from google.antigravity.hooks import hooks as hooks_mod

        session_ctx = hooks_mod.SessionContext()
        turn_ctx = hooks_mod.TurnContext(session_ctx)
        tc_a = sdk_types.ToolCall(name="tool_a", args={"a": 1}, id="call-A")
        tc_b = sdk_types.ToolCall(name="tool_b", args={"b": 2}, id="call-B")

        res_a, mod_tc_a, op_ctx_a = await runner.dispatch_pre_tool_call(
            turn_context=turn_ctx, tool_call=tc_a
        )
        res_b, mod_tc_b, op_ctx_b = await runner.dispatch_pre_tool_call(
            turn_context=turn_ctx, tool_call=tc_b
        )
        assert res_a.allow is True
        assert res_a.modified_args == {"x": 100, "injected": True}
        assert mod_tc_a.args == {"x": 100, "injected": True}
        assert res_b.modified_args is None
        assert mod_tc_b.args == {"b": 2}

        # Complete call-A AFTER call-B started: call-A's args must NOT be overwritten by call-B!
        tr_a = sdk_types.ToolResult(name="tool_a", id="call-A", result="ok-a")
        tr_b = sdk_types.ToolResult(name="tool_b", id="call-B", result="ok-b")
        await runner.dispatch_post_tool_call(op_ctx_a, tr_a)
        await runner.dispatch_post_tool_call(op_ctx_b, tr_b)

    asyncio.run(_exercise())

    pre_calls = [p for pt, p in dispatch_calls if pt == "pre_tool_call_decide"]
    post_calls = [p for pt, p in dispatch_calls if pt == "post_tool_call"]
    assert len(pre_calls) == 2
    assert len(post_calls) == 2
    assert post_calls[0]["args"] == {"x": 100, "injected": True}
    assert post_calls[1]["args"] == {"b": 2}


def test_disable_manage_subagents_tool_and_google_gemini_base_url(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import logging

    from google.antigravity.connections.local.local_connection import (
        LocalConnectionStrategy,
    )

    class _IsolatedStrategy(LocalConnectionStrategy):
        pass

    assert sdk_proto_parity._patch_harness_side_tools_proto(
        _IsolatedStrategy, logging.getLogger("test_0124")
    )
    local_cfg: dict[str, Any] = {
        "capabilities": {"disable_manage_subagents_tool": True}
    }
    agent_init._wire_capabilities(local_cfg)
    strat = _IsolatedStrategy(capabilities_config=local_cfg["capabilities"])
    hc = strat._build_harness_config()
    assert hc.harness_side_tools.subagents.disable_manage_subagents_tool is True

    monkeypatch.delenv("GEMINI_API_BASE_URL", raising=False)
    monkeypatch.setenv("GOOGLE_GEMINI_BASE_URL", "http://127.0.0.1:19876")
    routing_cfg: dict[str, Any] = {}
    resolved = agent_init._setup_base_url_routing(routing_cfg)
    assert resolved == "http://127.0.0.1:19876"


def test_standalone_workspace_only_appends_allow_all_and_preserves_containment(
    tmp_path: Any,
) -> None:
    from google.antigravity.hooks import policy as sdk_policy
    from google.antigravity.proto import localharness_pb2

    ws_dir = str(tmp_path / "ws")
    cfg: dict[str, Any] = {
        "policies": [{"WorkspaceOnly": [ws_dir]}],
    }
    agent_init._wire_policies(cfg, 42)
    assert cfg["workspaces"] == [ws_dir]
    rule_names = [getattr(p, "name", "") for p in cfg["policies"]]
    assert "workspace_only" in rule_names
    assert "allow_all" in rule_names

    proto_cfg, _ = sdk_policy._to_policy_config_proto(cfg["policies"])
    assert (
        proto_cfg.workspace_containment
        != localharness_pb2.PolicyConfig.WORKSPACE_CONTAINMENT_DISABLED
    )
    assert any(
        r.tool == "*" and r.decision == localharness_pb2.POLICY_DECISION_ALLOW
        for r in proto_cfg.rules
    )
