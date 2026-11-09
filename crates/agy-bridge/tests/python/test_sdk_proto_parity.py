"""Tests for fail-loud SDK proto parity patches, run against the real installed SDK.

Run via: ``uv run pytest crates/agy-bridge/tests/python -q``
"""

import inspect
import logging
import os
import sys
import types

import pytest

sys.path.insert(
    0, os.path.join(os.path.dirname(__file__), "..", "..", "src", "runtime", "py")
)

import agent_init
import sdk_proto_parity

# ── SDK-proto parity features (exercised against the real installed SDK) ────

_PARITY_LOGGER = logging.getLogger("agy_bridge.test_parity")
_HARNESS_SIDE_TOOLS_PARAMS = ["self", "cfg", "is_subagent"]


def _patched_strategy_cls():
    """Real `LocalConnectionStrategy` subclass so patches never leak globally."""
    from google.antigravity.connections.local.local_connection import (
        LocalConnectionStrategy,
    )

    class _Strategy(LocalConnectionStrategy):
        pass

    assert sdk_proto_parity._patch_harness_side_tools_proto(_Strategy, _PARITY_LOGGER)
    return _Strategy


def test_harness_side_tools_patch_keeps_real_sdk_signature_and_call_path():
    from google.antigravity.types import CapabilitiesConfig, SubagentCapabilities

    strategy_cls = _patched_strategy_cls()
    params = inspect.signature(strategy_cls._to_harness_side_tools_proto).parameters
    assert list(params) == _HARNESS_SIDE_TOOLS_PARAMS
    strategy = strategy_cls(capabilities_config=CapabilitiesConfig())
    harness_config = strategy._build_harness_config()
    assert harness_config.HasField("harness_side_tools")
    sub_proto = strategy._to_harness_side_tools_proto(
        SubagentCapabilities(), is_subagent=True
    )
    assert sub_proto.HasField("subagents")


def test_harness_side_tools_patch_sets_flag_for_root_agent_only():
    real_params = inspect.signature(
        _patched_strategy_cls().__mro__[1]._to_harness_side_tools_proto
    ).parameters
    assert list(real_params) == _HARNESS_SIDE_TOOLS_PARAMS

    class _Base:
        def _to_harness_side_tools_proto(self, cfg, is_subagent=False):
            subagents = types.SimpleNamespace(disable_define_subagent_tool=False)
            return types.SimpleNamespace(subagents=subagents)

    class _Strategy(_Base):
        pass

    assert sdk_proto_parity._patch_harness_side_tools_proto(_Strategy, _PARITY_LOGGER)
    flagged = types.SimpleNamespace(
        **{sdk_proto_parity.ATTR_DISABLE_DEFINE_SUBAGENT_TOOL: True}
    )
    strategy = _Strategy()
    root = strategy._to_harness_side_tools_proto(flagged)
    assert root.subagents.disable_define_subagent_tool is True
    sub = strategy._to_harness_side_tools_proto(flagged, is_subagent=True)
    assert sub.subagents.disable_define_subagent_tool is False
    plain = strategy._to_harness_side_tools_proto(types.SimpleNamespace())
    assert plain.subagents.disable_define_subagent_tool is False


def test_disable_define_subagent_tool_reaches_harness_or_fails_loudly():
    local_cfg = {"capabilities": {"disable_define_subagent_tool": True}}
    supported = sdk_proto_parity._sdk_proto_supports(
        sdk_proto_parity.PROTO_SUBAGENTS_CONFIG,
        sdk_proto_parity.FIELD_DISABLE_DEFINE_SUBAGENT_TOOL,
    )
    if supported:
        agent_init._wire_capabilities(local_cfg)
        strategy = _patched_strategy_cls()(
            capabilities_config=local_cfg["capabilities"]
        )
        harness_config = strategy._build_harness_config()
        assert harness_config.harness_side_tools.subagents.disable_define_subagent_tool
    else:
        with pytest.raises(
            sdk_proto_parity.SdkFeatureUnsupportedError,
            match="disable_define_subagent_tool",
        ):
            agent_init._wire_capabilities(local_cfg)


def test_titled_custom_system_instructions_reach_harness_or_fail_loudly():
    from google.antigravity.connections.local import local_connection

    assert sdk_proto_parity._patch_compaction_config_proto(
        local_connection, _PARITY_LOGGER
    )
    local_cfg = {"system_instructions": {"title": "Persona", "text": "Audit."}}
    supported = sdk_proto_parity._sdk_proto_supports(
        sdk_proto_parity.PROTO_CUSTOM_SI_PART, sdk_proto_parity.FIELD_TITLE
    )
    if supported:
        agent_init._wire_system_instructions(local_cfg)
        proto = local_connection.to_system_instructions_proto(
            local_cfg["system_instructions"]
        )
        assert proto.custom.part[0].title == "Persona"
        assert proto.custom.part[0].text == "Audit."
    else:
        with pytest.raises(
            sdk_proto_parity.SdkFeatureUnsupportedError, match="TitledCustom"
        ):
            agent_init._wire_system_instructions(local_cfg)


def test_untitled_custom_system_instructions_need_no_sdk_support():
    from google.antigravity.connections.local import local_connection

    local_cfg = {"system_instructions": {"title": None, "text": "Audit."}}
    agent_init._wire_system_instructions(local_cfg)
    proto = local_connection.to_system_instructions_proto(
        local_cfg["system_instructions"]
    )
    assert proto.custom.part[0].text == "Audit."


def test_compaction_extras_reach_harness_and_include_last_user_message_fails_loudly():
    from google.antigravity.connections.local import local_connection
    from google.antigravity.types import CapabilitiesConfig

    assert sdk_proto_parity._patch_compaction_config_proto(
        local_connection, _PARITY_LOGGER
    )
    base = {
        "token_threshold": 50000,
        "checkpoint_interval_tokens": 10000,
        "max_context_tokens": 64000,
        "summary_prompt_override": "Keep CVEs",
        "use_model_reported_token_count": True,
    }
    local_cfg = {"compaction_config": dict(base)}
    agent_init._wire_compaction_config(local_cfg)
    proto, _ = local_connection.to_proto_compaction_config(
        local_cfg["compaction_config"], CapabilitiesConfig()
    )
    assert proto.checkpoint_interval_tokens == 10000
    assert proto.max_context_tokens == 64000
    assert proto.summary_prompt_override == "Keep CVEs"
    assert proto.use_model_reported_token_count is True

    with_flag = {"compaction_config": {**base, "include_last_user_message": True}}
    supported = sdk_proto_parity._sdk_proto_supports(
        sdk_proto_parity.PROTO_COMPACTION_CONFIG,
        sdk_proto_parity.FIELD_INCLUDE_LAST_USER_MESSAGE,
    )
    if supported:
        agent_init._wire_compaction_config(with_flag)
        proto, _ = local_connection.to_proto_compaction_config(
            with_flag["compaction_config"], CapabilitiesConfig()
        )
        assert proto.include_last_user_message is True
    else:
        with pytest.raises(
            sdk_proto_parity.SdkFeatureUnsupportedError,
            match="include_last_user_message",
        ):
            agent_init._wire_compaction_config(with_flag)


def test_mcp_server_env_unwrapping_and_per_server_fields_reach_harness_proto():
    from google.antigravity.connections.local import local_connection

    local_cfg = {
        "mcp_servers": [
            {
                "type": "stdio",
                "name": "ghidra_mcp",
                "command": "env",
                "args": [
                    "API_TOKEN=secret123",
                    "ghidra-mcp-server",
                    "--port",
                    "8080",
                ],
                "enabled_tools": ["decompile_function", "list_functions"],
                "timeout_seconds": 60,
                "force_all_tools_eager": True,
            },
            {
                "type": "http",
                "name": "remote_mcp",
                "url": "http://127.0.0.1:9090/mcp",
                "disabled_tools": ["drop_database"],
                "timeout_seconds": 25,
                "force_all_tools_eager": True,
            },
        ]
    }
    agent_init._wire_mcp_servers(local_cfg)
    assert len(local_cfg["mcp_servers"]) == 2

    stdio_proto = local_connection._to_mcp_server_proto(local_cfg["mcp_servers"][0])
    assert stdio_proto.name == "ghidra_mcp"
    assert stdio_proto.stdio.command == "ghidra-mcp-server"
    assert list(stdio_proto.stdio.args) == ["--port", "8080"]
    assert dict(stdio_proto.stdio.env) == {"API_TOKEN": "secret123"}
    assert list(stdio_proto.enabled_tools) == ["decompile_function", "list_functions"]
    assert list(stdio_proto.disabled_tools) == []
    assert stdio_proto.timeout_seconds == 60
    assert stdio_proto.force_all_tools_eager is True

    http_proto = local_connection._to_mcp_server_proto(local_cfg["mcp_servers"][1])
    assert http_proto.name == "remote_mcp"
    assert http_proto.http.url == "http://127.0.0.1:9090/mcp"
    assert list(http_proto.enabled_tools) == []
    assert list(http_proto.disabled_tools) == ["drop_database"]
    assert http_proto.timeout_seconds == 25
    assert http_proto.force_all_tools_eager is True
