"""SDK 0.1.24 Python backend & protobuf wire conformance tests."""

from __future__ import annotations

import asyncio
import json
import logging
import sys
import types
from pathlib import Path
from typing import Any

import pytest

_PY_SRC = Path(__file__).resolve().parents[2] / "src" / "runtime" / "py"
if str(_PY_SRC) not in sys.path:
    sys.path.insert(0, str(_PY_SRC))

import agent_init  # type: ignore[import-not-found]
import agent_init_helpers  # type: ignore[import-not-found]
import sdk_proto_parity  # type: ignore[import-not-found]


def test_vertex_openai_litert_and_sse_mcp_lowering(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from google.antigravity import types as sdk_types
    from google.antigravity.connections.local import local_connection as lc_mod
    from google.antigravity.connections.local.litert_connection_config import (
        LiteRTAgentConfig,
        LiteRTBackend,
    )
    from google.antigravity.connections.local.local_connection import (
        LocalConnectionStrategy,
    )
    from google.antigravity.connections.local.local_openai_connection_config import (
        LocalOpenAIAgentConfig,
    )

    logger = logging.getLogger("test_0124_conformance")
    sdk_proto_parity._patch_harness_side_tools_proto(LocalConnectionStrategy, logger)
    sdk_proto_parity._patch_compaction_config_proto(lc_mod, logger)

    # 1. VertexConfig + http_headers + ambient env fallback when base_url is unset
    monkeypatch.setenv("GOOGLE_CLOUD_PROJECT", "ambient-proj")
    monkeypatch.setenv("GOOGLE_CLOUD_LOCATION", "europe-west4")
    v_cfg: dict[str, Any] = {
        "model": "gemini-4-hydrogen",
        "env": {"GOOGLE_CLOUD_PROJECT": "env-override-proj"},
        "vertex_config": {
            "http_headers": {"X-Custom-Tenant": "acme"},
            "models": {
                "default": {
                    "name": "gemini-4-hydrogen",
                    "generation": {
                        "thinking_level": "xhigh",
                        "service_tier": "priority",
                    },
                },
                "image_generation": {"name": "gemini-3.1-flash-lite-image"},
            },
        },
        "mcp_servers": [
            {
                "type": "sse",
                "name": "remote_sse",
                "url": "https://mcp.example.com/sse",
                "headers": {"Authorization": "Bearer tok"},
                "timeout_seconds": 25,
                "force_all_tools_eager": True,
            }
        ],
    }
    agent_init._wire_mcp_servers(v_cfg)
    assert isinstance(v_cfg["mcp_servers"][0], sdk_types.McpStreamableHttpServer)
    local_agent_cfg = sdk_proto_parity._build_python_agent_config(v_cfg, [])
    strat = local_agent_cfg.create_strategy(tool_runner=None, hook_runner=None)
    hc = strat._build_harness_config()
    assert len(hc.models) == 2
    assert hc.models[0].HasField("vertex_endpoint")
    assert hc.models[0].vertex_endpoint.project == "env-override-proj"
    assert hc.models[0].vertex_endpoint.location == "europe-west4"
    assert dict(hc.models[0].vertex_endpoint.http_headers) == {
        "X-Custom-Tenant": "acme"
    }
    assert hc.models[0].vertex_endpoint.options.thinking_level == "xhigh"
    assert len(hc.mcp_servers) == 1
    assert hc.mcp_servers[0].HasField("http")
    assert hc.mcp_servers[0].http.url == "https://mcp.example.com/sse"

    # 2. LocalOpenAIConfig -> LocalOpenAIAgentConfig -> GemmaEndpoint
    oai_raw: dict[str, Any] = {
        "model": "gemma-3-27b-it",
        "local_openai_config": {
            "model": "qwen3-coder:30b",
            "base_url": "http://127.0.0.1:11434/v1",
        },
    }
    oai_agent_cfg = sdk_proto_parity._build_python_agent_config(oai_raw, [])
    assert isinstance(oai_agent_cfg, LocalOpenAIAgentConfig)
    oai_strat = oai_agent_cfg.create_strategy(tool_runner=None, hook_runner=None)
    oai_hc = oai_strat._build_harness_config()
    gemma_models = [m for m in oai_hc.models if m.HasField("gemma_endpoint")]
    assert len(gemma_models) == 1
    assert gemma_models[0].name == "qwen3-coder:30b"
    assert gemma_models[0].gemma_endpoint.base_url == "http://127.0.0.1:11434/v1"

    # 3. LiteRTConfig -> LiteRTAgentConfig
    model_file = tmp_path / "demo.litertlm"
    model_file.write_bytes(b"litert")
    lrt_raw: dict[str, Any] = {
        "litert_config": {
            "model_path": str(model_file),
            "backend": "npu",
            "audio_backend": "cpu",
            "vision_backend": "gpu",
            "enable_speculative_decoding": True,
            "cache_dir": str(tmp_path / "cache"),
            "port": 0,
            "download_if_missing": False,
        }
    }
    lrt_agent_cfg = sdk_proto_parity._build_python_agent_config(lrt_raw, [])
    assert isinstance(lrt_agent_cfg, LiteRTAgentConfig)
    assert lrt_agent_cfg.backend == LiteRTBackend.NPU
    assert lrt_agent_cfg.audio_backend == LiteRTBackend.CPU
    assert lrt_agent_cfg.vision_backend == LiteRTBackend.GPU
    assert lrt_agent_cfg.enable_speculative_decoding is True


def test_base_url_precedence_and_subagent_disable_flags(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from google.antigravity.connections.local.local_connection import (
        LocalConnectionStrategy,
    )

    logger = logging.getLogger("test_0124_prec")
    sdk_proto_parity._patch_harness_side_tools_proto(LocalConnectionStrategy, logger)

    monkeypatch.setenv("GOOGLE_GEMINI_BASE_URL", "http://127.0.0.1:11111")
    monkeypatch.delenv("GEMINI_API_BASE_URL", raising=False)

    # config.env["GOOGLE_GEMINI_BASE_URL"] beats process env
    env_cfg: dict[str, Any] = {
        "env": {"GOOGLE_GEMINI_BASE_URL": "http://127.0.0.1:22222"},
    }
    assert agent_init._setup_base_url_routing(env_cfg) == "http://127.0.0.1:22222"

    # explicit gemini_config.base_url beats config.env and process env
    explicit_cfg: dict[str, Any] = {
        "env": {"GOOGLE_GEMINI_BASE_URL": "http://127.0.0.1:22222"},
        "gemini_config": {
            "base_url": "http://127.0.0.1:33333",
            "http_headers": {"X-Auth": "1"},
        },
        "capabilities": {
            "disable_define_subagent_tool": True,
            "disable_manage_subagents_tool": True,
            "image_model": "imagen-custom-v4",
        },
        "subagents": [
            {
                "name": "worker",
                "description": "Worker subagent",
                "capabilities": {
                    "agent_behavior": "autonomous",
                    "enabled_tools": ["view_file"],
                    "disable_define_subagent_tool": True,
                    "disable_manage_subagents_tool": True,
                },
            }
        ],
    }
    agent_init._wire_capabilities(explicit_cfg)
    sdk_proto_parity._wire_subagents(explicit_cfg)
    resolved_url = agent_init._setup_base_url_routing(explicit_cfg)
    assert resolved_url == "http://127.0.0.1:33333"
    agent_init._munge_config_model(explicit_cfg, custom_base_url=resolved_url)
    lac = sdk_proto_parity._build_python_agent_config(explicit_cfg, [])
    strat = lac.create_strategy(tool_runner=None, hook_runner=None)
    hc = strat._build_harness_config()
    assert hc.harness_side_tools.subagents.disable_define_subagent_tool is True
    assert hc.harness_side_tools.subagents.disable_manage_subagents_tool is True
    assert (
        hc.custom_subagents[0].harness_side_tools.subagents.disable_define_subagent_tool
        is True
    )
    assert (
        hc.custom_subagents[
            0
        ].harness_side_tools.subagents.disable_manage_subagents_tool
        is True
    )
    assert hc.models[1].name == "imagen-custom-v4"
    assert dict(hc.models[0].gemini_api_endpoint.http_headers) == {"X-Auth": "1"}


def test_on_interaction_returns_sdk_question_hook_result_and_tool_metadata_never_leaks(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from google.antigravity import types as sdk_types
    from google.antigravity.hooks import hook_runner as hr_mod
    from google.antigravity.hooks import hooks as hooks_mod

    globals_mod = types.ModuleType("_agy_bridge_globals")

    async def _fake_hook(agent_id: int, point: str, ctx_json: str) -> str:
        assert agent_id == 77
        assert point == "on_interaction"
        parsed = json.loads(ctx_json)
        assert parsed["questions"][0]["question"] == "Deploy?"
        return json.dumps(
            {
                "responses": [
                    {
                        "selected_option_ids": ["2"],
                        "freeform_response": "",
                        "skipped": False,
                    }
                ],
                "cancelled": False,
            }
        )

    async def _fake_tool(agent_id: int, name: str, args_json: str) -> dict[str, Any]:
        assert agent_id == 77
        assert name == "probe_tool"
        assert json.loads(args_json) == {"q": "ok"}
        return {
            "content": '{"status":"clean"}',
            "metadata": {"host_secret": "NEVER_SEND_TO_LLM"},
        }

    globals_mod.dispatch_rust_hook = _fake_hook  # type: ignore[attr-defined]
    globals_mod.dispatch_rust_tool = _fake_tool  # type: ignore[attr-defined]
    globals_mod.get_current_client_tool_call_id = (  # type: ignore[attr-defined]
        agent_init_helpers.get_current_client_tool_call_id
    )
    globals_mod._store_tracked_tool_call = (  # type: ignore[attr-defined]
        agent_init_helpers._store_tracked_tool_call
    )
    globals_mod._store_tool_metadata = (  # type: ignore[attr-defined]
        agent_init_helpers._store_tool_metadata
    )
    globals_mod._pop_tool_metadata = (  # type: ignore[attr-defined]
        agent_init_helpers._pop_tool_metadata
    )
    monkeypatch.setitem(sys.modules, "_agy_bridge_globals", globals_mod)

    cfg: dict[str, Any] = {
        "hooks": [{"name": "qa", "point": "OnInteraction", "callback_id": "qa_1"}],
        "tools": [
            {"name": "probe_tool", "description": "Probe", "parameter_schema": {}}
        ],
    }
    sdk_proto_parity._wire_tool_proxies(cfg, 77)
    agent_init._wire_hooks(cfg, 77)
    runner = hr_mod.HookRunner(hooks=cfg["hooks"])

    async def _run() -> None:
        spec = sdk_types.AskQuestionInteractionSpec(
            questions=[
                sdk_types.AskQuestionEntry(
                    question="Deploy?",
                    options=[
                        sdk_types.AskQuestionOption(id="1", text="No"),
                        sdk_types.AskQuestionOption(id="2", text="Yes"),
                    ],
                )
            ]
        )
        turn_ctx = hooks_mod.TurnContext(runner.session_context)
        hr_res, q_res, _ = await runner.dispatch_interaction(turn_ctx, spec)
        assert hr_res.allow is True
        assert isinstance(q_res, sdk_types.QuestionHookResult)
        assert q_res.cancelled is False
        assert q_res.responses[0].selected_option_ids == ["2"]

        proxy = cfg["tools"][0]
        tok = agent_init_helpers._CURRENT_CLIENT_TOOL_CALL_ID.set("call-probe-1")
        try:
            llm_visible = await proxy(q="ok")
        finally:
            agent_init_helpers._CURRENT_CLIENT_TOOL_CALL_ID.reset(tok)
        assert llm_visible == '{"status":"clean"}'
        assert "NEVER_SEND_TO_LLM" not in str(llm_visible)

        # Builtin/other tool with a different call_id MUST NOT inherit probe_tool's metadata!
        builtin_res = sdk_types.ToolResult(
            name="view_file", id="call-builtin-2", result="contents"
        )
        builtin_payload = json.loads(
            agent_init_helpers._serialize_post_tool_call_ctx(builtin_res, None, 77)
        )
        assert builtin_payload["metadata"] == {}

        # Matching call_id pops probe_tool's exact metadata once!
        probe_res = sdk_types.ToolResult(
            name="probe_tool", id="call-probe-1", result=llm_visible
        )
        probe_payload = json.loads(
            agent_init_helpers._serialize_post_tool_call_ctx(probe_res, None, 77)
        )
        assert probe_payload["metadata"] == {"host_secret": "NEVER_SEND_TO_LLM"}
        second_pop = json.loads(
            agent_init_helpers._serialize_post_tool_call_ctx(probe_res, None, 77)
        )
        assert second_pop["metadata"] == {}

    asyncio.run(_run())


def test_env_validation_vertex_standard_vs_express_and_base_url_precedence(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from google.antigravity import types as sdk_types

    # 1. Dangerous & malformed env entries fail loudly
    for bad_env in [
        {"": "x"},
        {"BAD=KEY": "x"},
        {"BAD\nKEY": "x"},
        {"GOOD": "val\0nul"},
        {"ld_preload": "/tmp/evil.so"},
        {"DYLD_INSERT_LIBRARIES": "/tmp/evil.dylib"},
        {"ANTIGRAVITY_HARNESS_PATH": "/tmp/fake"},
        {"PYTHONPATH": "/tmp/hijack"},
        {"prompt_command": "evil"},
    ]:
        with pytest.raises(ValueError):
            sdk_proto_parity._validate_agent_env(bad_env)

    sdk_proto_parity._validate_agent_env(
        {"GOOGLE_GEMINI_BASE_URL": "http://127.0.0.1:9"}
    )

    # 2. VertexConfig Standard mode ignores top-level api_key; Express mode uses it
    monkeypatch.delenv("GOOGLE_CLOUD_PROJECT", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_LOCATION", raising=False)
    std_cfg: dict[str, Any] = {
        "api_key": "ambient-gemini-key",
        "vertex_config": {"project": "p1", "location": "us-central1"},
    }
    built_std = sdk_proto_parity._build_python_agent_config(std_cfg, [])
    ep_std = built_std.models[0].endpoint
    assert isinstance(ep_std, sdk_types.VertexEndpoint)
    assert ep_std.project == "p1"
    assert ep_std.location == "us-central1"
    assert ep_std.api_key is None
    ep_std.validate_endpoint()

    exp_cfg: dict[str, Any] = {
        "api_key": "express-key-123",
        "vertex_config": {},
    }
    built_exp = sdk_proto_parity._build_python_agent_config(exp_cfg, [])
    ep_exp = built_exp.models[0].endpoint
    assert isinstance(ep_exp, sdk_types.VertexEndpoint)
    assert ep_exp.api_key == "express-key-123"
    ep_exp.validate_endpoint()

    # 3. GOOGLE_GEMINI_BASE_URL beats GEMINI_API_BASE_URL in os.environ
    monkeypatch.setenv("GOOGLE_GEMINI_BASE_URL", "http://127.0.0.1:18001")
    monkeypatch.setenv("GEMINI_API_BASE_URL", "http://127.0.0.1:18002")
    local_cfg: dict[str, Any] = {}
    chosen = agent_init._setup_base_url_routing(local_cfg)
    assert chosen == "http://127.0.0.1:18001"
