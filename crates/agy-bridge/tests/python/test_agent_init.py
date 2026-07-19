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


def test_munge_config_model_drops_model_when_gemini_present():
    cfg = {"model": "gemini-pro", "gemini_config": {"models": {"default": "x"}}}
    agent_init._munge_config_model(cfg)
    assert "model" not in cfg
    assert "gemini_config" in cfg


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
