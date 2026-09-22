"""Unit tests for the pure, importable helpers in ``agent_init.py``.

These tests exercise the DRY helpers and context-serialization helpers
*without* the live antigravity SDK. All SDK imports in ``agent_init`` are lazy
(inside functions), so a bare ``import agent_init`` must succeed here.

Run via: ``python3 -m pytest crates/agy-bridge/tests/python -q``
"""

import json
import os
import sys
from collections import namedtuple

sys.path.insert(
    0, os.path.join(os.path.dirname(__file__), "..", "..", "src", "runtime", "py")
)

import agent_init  # noqa: E402

# ── Fakes ──────────────────────────────────────────────────────────────────


class ModelDumpObj:
    """Pydantic-v2-like object exposing ``model_dump``."""

    def __init__(self, data):
        self._data = data

    def model_dump(self):
        return dict(self._data)


class DictObj:
    """Pydantic-v1-like object exposing ``dict`` but not ``model_dump``."""

    def __init__(self, data):
        self._data = data

    def dict(self):
        return dict(self._data)


class ModelDumpJsonObj:
    """Object exposing ``model_dump_json`` (used by generic serializer)."""

    def __init__(self, data):
        self._data = data

    def model_dump_json(self):
        return json.dumps(self._data)


class EnumLike:
    """Object with a ``.value`` attribute, like an enum member."""

    def __init__(self, value):
        self.value = value


class NonSerializable:
    """Object that cannot be JSON serialized and has no dump methods."""

    def __repr__(self):
        return "<NonSerializable>"


ToolCall = namedtuple("ToolCall", ["name", "args"])


# ── _to_dict ────────────────────────────────────────────────────────────────


def test_to_dict_model_dump():
    obj = ModelDumpObj({"a": 1})
    assert agent_init._to_dict(obj) == {"a": 1}


def test_to_dict_dict_fallback():
    obj = DictObj({"b": 2})
    assert agent_init._to_dict(obj) == {"b": 2}


def test_to_dict_plain_dict_passthrough():
    d = {"c": 3}
    # A plain dict has no model_dump; it *does* have .dict? No — builtin dict
    # has no ``dict`` attribute, so it should pass through unchanged.
    assert agent_init._to_dict(d) is d


def test_to_dict_scalar_passthrough():
    assert agent_init._to_dict(42) == 42
    assert agent_init._to_dict("x") == "x"


def test_to_dict_prefers_model_dump_over_dict():
    class Both:
        def model_dump(self):
            return {"from": "model_dump"}

        def dict(self):
            return {"from": "dict"}

    assert agent_init._to_dict(Both()) == {"from": "model_dump"}


# ── _normalize_tool_name ─────────────────────────────────────────────────────


def test_normalize_tool_name_enum_like():
    assert agent_init._normalize_tool_name(EnumLike("read_file")) == "read_file"


def test_normalize_tool_name_plain_str():
    assert agent_init._normalize_tool_name("grep") == "grep"


def test_normalize_tool_name_int_to_str():
    assert agent_init._normalize_tool_name(7) == "7"


# ── RESULT_HOOK_POINTS contract ──────────────────────────────────────────────


def test_result_hook_points_exact_membership():
    assert set(agent_init.RESULT_HOOK_POINTS) == {
        "pre_turn",
        "pre_tool_call_decide",
        "on_interaction",
    }
    assert len(agent_init.RESULT_HOOK_POINTS) == 3


# ── _serialize_post_tool_call_ctx ────────────────────────────────────────────


def test_serialize_post_tool_call_string_result():
    ctx = namedtuple("Ctx", ["name", "result"])("do_thing", "hello")
    tc = ToolCall("do_thing", {"x": 1})
    out = json.loads(agent_init._serialize_post_tool_call_ctx(ctx, tc))
    assert out == {
        "name": "do_thing",
        "args": {"x": 1},
        "result": "hello",
        "metadata": {},
    }


def test_serialize_post_tool_call_dict_content_result():
    Ctx = namedtuple("Ctx", ["name", "result"])
    ctx = Ctx("do_thing", {"content": "body", "metadata": {"k": "v"}})
    tc = ToolCall("do_thing", {})
    out = json.loads(agent_init._serialize_post_tool_call_ctx(ctx, tc))
    assert out["result"] == "body"
    assert out["metadata"] == {"k": "v"}


def test_serialize_post_tool_call_none_result_and_no_tool_call():
    Ctx = namedtuple("Ctx", ["name", "result"])
    ctx = Ctx("t", None)
    out = json.loads(agent_init._serialize_post_tool_call_ctx(ctx, None))
    assert out == {"name": "t", "args": {}, "result": "", "metadata": {}}


def test_serialize_post_tool_call_enum_name_and_model_dump_args():
    Ctx = namedtuple("Ctx", ["name", "result"])
    ctx = Ctx(EnumLike("enum_tool"), "r")
    tc = ToolCall("enum_tool", ModelDumpObj({"p": 9}))
    out = json.loads(agent_init._serialize_post_tool_call_ctx(ctx, tc))
    assert out["name"] == "enum_tool"
    assert out["args"] == {"p": 9}


def test_serialize_post_tool_call_nested_result_object():
    Ctx = namedtuple("Ctx", ["name", "result"])
    Inner = namedtuple("Inner", ["result"])
    ctx = Ctx("t", Inner({"content": "deep", "metadata": {"m": 1}}))
    out = json.loads(agent_init._serialize_post_tool_call_ctx(ctx, None))
    assert out["result"] == "deep"
    assert out["metadata"] == {"m": 1}


def test_serialize_post_tool_call_model_dump_json_result():
    class ResultWithJson:
        result = None  # not a dict-with-content

        def model_dump_json(self):
            return json.dumps({"serialized": True})

    Ctx = namedtuple("Ctx", ["name", "result"])
    ctx = Ctx("t", ResultWithJson())
    out = json.loads(agent_init._serialize_post_tool_call_ctx(ctx, None))
    assert out["result"] == json.dumps({"serialized": True})


def test_serialize_post_tool_call_non_serializable_fallback():
    Ctx = namedtuple("Ctx", ["name", "result"])
    ctx = Ctx("t", NonSerializable())
    out = json.loads(agent_init._serialize_post_tool_call_ctx(ctx, None))
    # Falls back to str() of the object.
    assert out["result"] == "<NonSerializable>"


# ── _serialize_on_tool_error_ctx ─────────────────────────────────────────────


class _CapturingLogger:
    def __init__(self):
        self.warnings = []

    def warning(self, *args, **kwargs):
        self.warnings.append((args, kwargs))


def test_serialize_on_tool_error_basic():
    tc = ToolCall("boom", {"a": 1})
    logger = _CapturingLogger()
    out = json.loads(agent_init._serialize_on_tool_error_ctx("kaboom", tc, logger))
    assert out == {"tool_name": "boom", "tool_args": {"a": 1}, "error": "kaboom"}


def test_serialize_on_tool_error_no_tool_call():
    logger = _CapturingLogger()
    out = json.loads(agent_init._serialize_on_tool_error_ctx("err", None, logger))
    assert out == {"tool_name": "", "tool_args": {}, "error": "err"}


def test_serialize_on_tool_error_with_metadata():
    class ErrCtx:
        metadata = ModelDumpObj({"code": 500})

        def __str__(self):
            return "the error"

    tc = ToolCall(EnumLike("t"), {})
    logger = _CapturingLogger()
    out = json.loads(agent_init._serialize_on_tool_error_ctx(ErrCtx(), tc, logger))
    assert out["tool_name"] == "t"
    assert out["error"] == "the error"
    assert out["metadata"] == {"code": 500}


# ── _serialize_session_ctx ───────────────────────────────────────────────────


def test_serialize_session_ctx_with_conversation_id():
    out = json.loads(
        agent_init._serialize_session_ctx({"conversation_id": "conv-1"}, 5)
    )
    assert out == {"session": {"session_id": "conv-1", "agent_id": 5}}


def test_serialize_session_ctx_workspace_fallback():
    cfg = {"workspaces": ["/home/user/my-workspace/"]}
    out = json.loads(agent_init._serialize_session_ctx(cfg, 3))
    assert out["session"]["session_id"] == "my-workspace"
    assert out["session"]["agent_id"] == 3


def test_serialize_session_ctx_windows_workspace_fallback():
    cfg = {"workspaces": ["C:\\Users\\user\\my-workspace\\"]}
    out = json.loads(agent_init._serialize_session_ctx(cfg, 3))
    assert out["session"]["session_id"] == "my-workspace"
    assert out["session"]["agent_id"] == 3


def test_serialize_session_ctx_default_fallback():
    out = json.loads(agent_init._serialize_session_ctx({}, 1))
    assert out["session"]["session_id"] == "default_session"


# ── _serialize_post_turn_ctx / _serialize_pre_turn_ctx ───────────────────────


def test_serialize_post_turn_with_text_and_turn_number():
    Ctx = namedtuple("Ctx", ["text", "turn_number"])
    out = json.loads(agent_init._serialize_post_turn_ctx(Ctx("resp", 4)))
    assert out == {"response_text": "resp", "turn_number": 4}


def test_serialize_post_turn_missing_fields_uses_str_and_zero():
    class Ctx:
        def __str__(self):
            return "stringified"

    out = json.loads(agent_init._serialize_post_turn_ctx(Ctx()))
    assert out == {"response_text": "stringified", "turn_number": 0}


def test_serialize_pre_turn_plain_string():
    out = json.loads(agent_init._serialize_pre_turn_ctx("my prompt"))
    assert out == {"prompt": "my prompt", "turn_number": 0}


def test_serialize_pre_turn_object_with_turn_number():
    class Ctx:
        turn_number = 2

        def __str__(self):
            return "obj-prompt"

    out = json.loads(agent_init._serialize_pre_turn_ctx(Ctx()))
    assert out == {"prompt": "obj-prompt", "turn_number": 2}


# ── _serialize_generic_ctx ───────────────────────────────────────────────────


def test_serialize_generic_string():
    assert json.loads(agent_init._serialize_generic_ctx("hi")) == {"value": "hi"}


def test_serialize_generic_model_dump_json():
    out = json.loads(agent_init._serialize_generic_ctx(ModelDumpJsonObj({"z": 1})))
    assert out == {"z": 1}


def test_serialize_generic_dict():
    assert json.loads(agent_init._serialize_generic_ctx({"a": 2})) == {"a": 2}


def test_serialize_generic_fallback_str():
    out = json.loads(agent_init._serialize_generic_ctx(NonSerializable()))
    assert out == "<NonSerializable>"


# ── _munge_config_model ──────────────────────────────────────────────────────


def test_munge_config_model_promotes_gemini_config_to_toplevel():
    cfg = {
        "model": "gemini-pro",
        "gemini_config": {"models": {"default": "x"}, "api_key": "secret"},
    }
    agent_init._munge_config_model(cfg)
    assert cfg["model"] == "x"
    assert cfg["api_key"] == "secret"
    assert "gemini_config" not in cfg


def test_munge_config_model_keeps_model_when_no_gemini():
    cfg = {"model": "gemini-pro"}
    agent_init._munge_config_model(cfg)
    assert cfg["model"] == "gemini-pro"


def test_munge_config_model_keeps_model_when_gemini_falsy():
    cfg = {"model": "gemini-pro", "gemini_config": None}
    agent_init._munge_config_model(cfg)
    assert cfg["model"] == "gemini-pro"

    cfg2 = {"model": "gemini-pro", "gemini_config": {}}
    agent_init._munge_config_model(cfg2)
    assert cfg2["model"] == "gemini-pro"


# ── _extract_initial_history ─────────────────────────────────────────────────


def test_extract_initial_history_present():
    cfg = {"initial_history": [{"role": "user", "content": "hi"}], "other": 1}
    hist = agent_init._extract_initial_history(cfg)
    assert hist == [{"role": "user", "content": "hi"}]
    assert "initial_history" not in cfg
    assert cfg == {"other": 1}


def test_extract_initial_history_missing_defaults_empty():
    cfg = {"other": 1}
    assert agent_init._extract_initial_history(cfg) == []
    assert cfg == {"other": 1}


# ── Import contract ──────────────────────────────────────────────────────────


def test_import_does_not_require_live_sdk():
    # Re-import in a clean namespace to prove no top-level SDK import runs.
    import importlib

    module = importlib.reload(agent_init)
    assert hasattr(module, "init_agent")
    assert hasattr(module, "RESULT_HOOK_POINTS")


# ── websockets max_size patch (version-gated) ────────────────────────────────


class _FakeLogger:
    """Minimal logger capturing info/warning calls for assertions."""

    def __init__(self):
        self.infos = []
        self.warnings = []

    def info(self, *args, **kwargs):
        self.infos.append((args, kwargs))

    def warning(self, *args, **kwargs):
        self.warnings.append((args, kwargs))


def _install_fake_websockets(monkeypatch):
    """Register a fake ``websockets`` module and return it plus a call recorder."""
    import types as _types

    calls = []
    fake_ws = _types.ModuleType("websockets")

    def _connect(*args, **kwargs):
        calls.append((args, kwargs))
        return "CONNECTION"

    fake_ws.connect = _connect
    monkeypatch.setitem(sys.modules, "websockets", fake_ws)
    return fake_ws, calls


def test_ws_patch_skipped_for_non_pinned_version():
    # A version that is not the pinned one must be left untouched (returns before
    # even importing websockets).
    logger = _FakeLogger()
    applied = agent_init._patch_websockets_max_size(
        logger, sdk_version="99.99.99-not-real"
    )
    assert applied is False


def test_ws_patch_skipped_for_unknown_version():
    # An unknown/empty version (e.g. metadata lookup failed) must skip the patch
    # deterministically, without importing or mutating the real websockets module.
    logger = _FakeLogger()
    assert agent_init._patch_websockets_max_size(logger, sdk_version="") is False


def test_ws_patch_applied_for_pinned_version(monkeypatch):
    fake_ws, calls = _install_fake_websockets(monkeypatch)
    logger = _FakeLogger()

    applied = agent_init._patch_websockets_max_size(
        logger, sdk_version=agent_init._WS_MAXSIZE_PATCH_SDK_VERSION
    )

    assert applied is True
    assert getattr(fake_ws, "_agy_max_size_patched", False) is True

    # Calling the wrapped connect without max_size injects the default cap.
    assert fake_ws.connect("ws://harness") == "CONNECTION"
    _, kwargs = calls[-1]
    assert kwargs["max_size"] == agent_init._WS_MAXSIZE_DEFAULT_CAP


def test_ws_patch_respects_explicit_caller_max_size(monkeypatch):
    fake_ws, calls = _install_fake_websockets(monkeypatch)
    agent_init._patch_websockets_max_size(
        _FakeLogger(), sdk_version=agent_init._WS_MAXSIZE_PATCH_SDK_VERSION
    )

    # An explicit max_size from a (future) SDK caller must win over our default.
    fake_ws.connect("ws://harness", max_size=4096)
    _, kwargs = calls[-1]
    assert kwargs["max_size"] == 4096


def test_ws_patch_is_idempotent(monkeypatch):
    fake_ws, _ = _install_fake_websockets(monkeypatch)

    assert agent_init._patch_websockets_max_size(
        _FakeLogger(), sdk_version=agent_init._WS_MAXSIZE_PATCH_SDK_VERSION
    )
    wrapped = fake_ws.connect

    # Second call is a no-op: it must not re-wrap the already-patched connect.
    assert agent_init._patch_websockets_max_size(
        _FakeLogger(), sdk_version=agent_init._WS_MAXSIZE_PATCH_SDK_VERSION
    )
    assert fake_ws.connect is wrapped


def test_resolve_ws_max_size_default(monkeypatch):
    monkeypatch.delenv("AGY_WS_MAX_MESSAGE_BYTES", raising=False)
    assert (
        agent_init._resolve_ws_max_size(_FakeLogger())
        == agent_init._WS_MAXSIZE_DEFAULT_CAP
    )


def test_resolve_ws_max_size_env_override(monkeypatch):
    monkeypatch.setenv("AGY_WS_MAX_MESSAGE_BYTES", "5000")
    assert agent_init._resolve_ws_max_size(_FakeLogger()) == 5000


def test_resolve_ws_max_size_env_unbounded(monkeypatch):
    monkeypatch.setenv("AGY_WS_MAX_MESSAGE_BYTES", "0")
    assert agent_init._resolve_ws_max_size(_FakeLogger()) is None


def test_resolve_ws_max_size_env_invalid_warns_and_defaults(monkeypatch):
    monkeypatch.setenv("AGY_WS_MAX_MESSAGE_BYTES", "not-an-int")
    logger = _FakeLogger()
    assert agent_init._resolve_ws_max_size(logger) == agent_init._WS_MAXSIZE_DEFAULT_CAP
    assert logger.warnings, "invalid env value should have logged a warning"


# ── v0.16.0 feature tests ──────────────────────────────────────────────────


def test_serialize_stop_args_none():
    assert agent_init._serialize_stop_args(None) == "{}"


def test_serialize_stop_args_populated():
    from collections import namedtuple

    StopArgsFake = namedtuple(
        "StopArgsFake",
        [
            "response_text",
            "trajectory_id",
            "continuation_count",
            "stop_reason",
            "error_message",
        ],
    )
    fake_args = StopArgsFake(
        response_text="Task finished successfully",
        trajectory_id="traj-12345",
        continuation_count=2,
        stop_reason=EnumLike("MAX_MODEL_CALLS_EXCEEDED"),
        error_message="",
    )
    serialized = agent_init._serialize_stop_args(fake_args)
    data = json.loads(serialized)
    assert data["response_text"] == "Task finished successfully"
    assert data["trajectory_id"] == "traj-12345"
    assert data["continuation_count"] == 2
    assert data["stop_reason"] == "MAX_MODEL_CALLS_EXCEEDED"
    assert data["error_message"] == ""


def _ensure_antigravity_types_module():
    try:
        from google.antigravity import types as _types

        return _types
    except ImportError:
        import types as py_types

        google_mod = sys.modules.setdefault("google", py_types.ModuleType("google"))
        agy_mod = sys.modules.setdefault(
            "google.antigravity", py_types.ModuleType("google.antigravity")
        )
        types_mod = sys.modules.setdefault(
            "google.antigravity.types", py_types.ModuleType("google.antigravity.types")
        )
        google_mod.antigravity = agy_mod
        agy_mod.types = types_mod

        class AgentBehavior:
            AUTONOMOUS = "autonomous"

            def __init__(self, val):
                self.value = val

        class RunCommandConfig:
            def __init__(
                self,
                enable_daemons=False,
                timeout_seconds=None,
                enable_sandbox=False,
            ):
                self.enable_daemons = enable_daemons
                self.timeout_seconds = timeout_seconds
                self.enable_sandbox = enable_sandbox

        class SubagentCapabilities:
            def __init__(
                self,
                agent_behavior=None,
                allowed_subagents=None,
                enabled_tools=None,
                disabled_tools=None,
                run_command_config=None,
            ):
                self.agent_behavior = agent_behavior
                self.allowed_subagents = allowed_subagents
                self.enabled_tools = enabled_tools
                self.disabled_tools = disabled_tools
                self.run_command_config = run_command_config

        class SubagentConfig:
            def __init__(
                self,
                name,
                description="",
                system_instructions=None,
                capabilities=None,
                tools=None,
            ):
                self.name = name
                self.description = description
                self.system_instructions = system_instructions
                self.capabilities = capabilities
                self.tools = tools or []

        types_mod.AgentBehavior = AgentBehavior
        types_mod.RunCommandConfig = RunCommandConfig
        types_mod.SubagentCapabilities = SubagentCapabilities
        types_mod.SubagentConfig = SubagentConfig
        return types_mod


def test_normalize_capabilities_with_run_command_config():
    types = _ensure_antigravity_types_module()
    config = {
        "capabilities": {
            "run_command_config": {
                "enable_daemons": True,
                "timeout_seconds": 45.5,
                "enable_sandbox": True,
            }
        }
    }
    agent_init._normalize_capabilities(config)
    cap = config["capabilities"]

    assert isinstance(cap["run_command_config"], types.RunCommandConfig)
    assert cap["run_command_config"].enable_daemons is True
    assert cap["run_command_config"].timeout_seconds == 45.5
    assert cap["run_command_config"].enable_sandbox is True


def test_wire_subagents_with_run_command_config():
    types = _ensure_antigravity_types_module()
    config = {
        "subagents": [
            {
                "name": "worker",
                "description": "Worker subagent",
                "system_instructions": "Do work",
                "capabilities": {
                    "run_command_config": {
                        "enable_daemons": False,
                        "timeout_seconds": 15.0,
                        "enable_sandbox": True,
                    }
                },
            }
        ]
    }
    agent_init._wire_subagents(config)
    sub = config["subagents"][0]

    assert isinstance(sub, types.SubagentConfig)
    assert sub.capabilities is not None
    assert sub.capabilities.run_command_config is not None
    assert sub.capabilities.run_command_config.enable_daemons is False
    assert sub.capabilities.run_command_config.timeout_seconds == 15.0
    assert sub.capabilities.run_command_config.enable_sandbox is True


def test_munge_config_model_strongly_typed_model_target():
    types = _ensure_antigravity_types_module()
    if not hasattr(types, "ModelTarget"):
        return
    cfg = {
        "model": "gemini-3-flash-preview",
        "gemini_config": {
            "api_key": "top-key",
            "models": {
                "default": {
                    "name": "gemini-3-flash-preview",
                    "api_key": "override-key",
                    "generation": {
                        "thinking_level": "HIGH",
                        "service_tier": "FLEX",
                    },
                },
                "image_generation": {
                    "name": "imagen-3",
                },
            },
        },
    }
    agent_init._munge_config_model(cfg, custom_base_url="http://127.0.0.1:8642")
    assert cfg["api_key"] == "top-key"
    assert "model" not in cfg
    assert isinstance(cfg["models"], list)
    assert len(cfg["models"]) == 2
    default_target, img_target = cfg["models"]
    assert isinstance(default_target, types.ModelTarget)
    assert default_target.name == "gemini-3-flash-preview"
    assert default_target.types == [types.ModelType.TEXT]
    assert default_target.endpoint.api_key == "override-key"
    assert default_target.endpoint.base_url == "http://127.0.0.1:8642"
    assert default_target.endpoint.options.thinking_level == types.ThinkingLevel.HIGH
    assert default_target.endpoint.options.service_tier == types.ServiceTier.FLEX
    assert isinstance(img_target, types.ModelTarget)
    assert img_target.name == "imagen-3"
    assert img_target.types == [types.ModelType.IMAGE]
    assert img_target.endpoint.api_key == "top-key"
    assert img_target.endpoint.base_url == "http://127.0.0.1:8642"

    from google.antigravity.connections.local.local_connection_config import (
        LocalAgentConfig,
    )

    lac = LocalAgentConfig(**cfg)
    assert len(lac.models) == 2
    assert lac.models[0].name == "gemini-3-flash-preview"
    assert lac.models[1].name == "imagen-3"


def test_wire_capabilities_minimal_and_truncation():
    types = _ensure_antigravity_types_module()
    if not hasattr(types, "CapabilitiesConfig"):
        return
    cfg = {
        "capabilities": {
            "agent_behavior": "minimal",
            "enabled_tools": ["view_file", "read_url_content"],
            "image_model": "imagen-3",
            "tool_output_truncation_config": {
                "max_tokens": 8192,
            },
        }
    }
    agent_init._wire_capabilities(cfg)
    cap = cfg["capabilities"]
    assert isinstance(cap, types.CapabilitiesConfig)
    assert cap.agent_behavior == types.AgentBehavior.MINIMAL
    assert cap.enabled_tools == [
        types.BuiltinTools.VIEW_FILE,
        types.BuiltinTools.READ_URL_CONTENT,
    ]
    assert isinstance(
        cap.tool_output_truncation_config, types.ToolOutputTruncationConfig
    )
    assert cap.tool_output_truncation_config.max_tokens == 8192


def test_wire_compaction_budget_retry_and_instructions():
    types = _ensure_antigravity_types_module()
    if not hasattr(types, "CompactionConfig"):
        return
    cfg = {
        "compaction_config": {"token_threshold": 50000},
        "budget_config": {
            "max_tokens": 100000,
            "max_model_calls": 10,
            "scope": "FORWARD_LOOKING",
        },
        "retry_config": {
            "api_retry": {
                "max_retries": 15,
                "initial_sleep_duration_ms": 1000,
                "exponential_multiplier": 2.0,
                "jitter_range": 0.2,
            },
            "model_output_retry": {"max_retries": 5},
        },
        "system_instructions": {
            "text": "Be concise and accurate.",
        },
    }
    agent_init._wire_compaction_config(cfg)
    agent_init._wire_budget_config(cfg)
    agent_init._wire_retry_config(cfg)
    agent_init._wire_system_instructions(cfg)

    assert isinstance(cfg["compaction_config"], types.CompactionConfig)
    assert cfg["compaction_config"].token_threshold == 50000
    assert isinstance(cfg["budget_config"], types.BudgetConfig)
    assert cfg["budget_config"].scope == types.BudgetScope.FORWARD_LOOKING
    assert isinstance(cfg["retry_config"], types.RetryConfig)
    assert cfg["retry_config"].api_retry.max_retries == 15
    assert cfg["retry_config"].model_output_retry.max_retries == 5
    assert isinstance(cfg["system_instructions"], types.CustomSystemInstructions)
    assert cfg["system_instructions"].text == "Be concise and accurate."
