# Pure module-level helpers for hook context serialization and SDK monkeypatches.

from __future__ import annotations

import logging
import os
from typing import Any

if os.name == "nt" and not os.environ.get("USERPROFILE"):
    import tempfile

    os.environ["USERPROFILE"] = os.environ.get("HOME") or tempfile.gettempdir()

RESULT_HOOK_POINTS: tuple[str, ...] = (
    "pre_turn",
    "pre_tool_call_decide",
    "on_interaction",
)
_PROXY_AUTH_SENTINEL: str = "__agy_proxy_auth__"


def _to_dict(obj: Any) -> Any:
    """Convert a pydantic-like object to a plain dict when possible."""
    if hasattr(obj, "model_dump"):
        return obj.model_dump()
    elif hasattr(obj, "dict"):
        return obj.dict()
    return obj


def _normalize_tool_name(name: Any) -> str:
    """Normalize a tool name to a plain string."""
    if hasattr(name, "value"):
        return str(name.value)
    elif not isinstance(name, str):
        return str(name)
    return name


def _serialize_post_tool_call_ctx(
    ctx: Any, current_tool_call: Any, agent_id_u64: int | None = None
) -> str:
    """Serialize a `post_tool_call` hook context to a JSON string."""
    import json

    tool_args = getattr(current_tool_call, "args", {}) if current_tool_call else {}
    tool_args = _to_dict(tool_args)

    result_val = getattr(ctx, "result", None)
    if result_val is None:
        result_val = getattr(ctx, "tool_result", None)
    result_str = ""
    metadata = {}
    if result_val is None:
        result_str = ""
    elif isinstance(result_val, str):
        try:
            parsed = json.loads(result_val)
            if isinstance(parsed, dict) and (
                "content" in parsed or "metadata" in parsed
            ):
                result_str = parsed.get("content", result_val)
                metadata = parsed.get("metadata", {})
            else:
                result_str = result_val
        except (ValueError, TypeError):
            result_str = result_val
    elif isinstance(result_val, dict) and (
        "content" in result_val or "metadata" in result_val
    ):
        result_str = result_val.get("content", "")
        metadata = result_val.get("metadata", {})
    else:
        tool_output = getattr(result_val, "result", None)
        if tool_output is None:
            tool_output = getattr(result_val, "output", None)
        if isinstance(tool_output, str):
            try:
                parsed = json.loads(tool_output)
                if isinstance(parsed, dict) and (
                    "content" in parsed or "metadata" in parsed
                ):
                    result_str = parsed.get("content", tool_output)
                    metadata = parsed.get("metadata", {})
                else:
                    result_str = tool_output
            except (ValueError, TypeError):
                result_str = tool_output
        elif isinstance(tool_output, dict) and (
            "content" in tool_output or "metadata" in tool_output
        ):
            result_str = tool_output.get("content", "")
            metadata = tool_output.get("metadata", {})
        else:
            try:
                if hasattr(result_val, "model_dump_json"):
                    result_str = result_val.model_dump_json()
                elif hasattr(result_val, "model_dump"):
                    result_str = json.dumps(result_val.model_dump())
                else:
                    result_str = json.dumps(result_val)
            except Exception:
                import logging

                logging.getLogger("agy_bridge.tool_dispatch").warning(
                    "Failed to JSON-serialize tool result, falling back to str()",
                    exc_info=True,
                )
                result_str = str(result_val)

    if not metadata and hasattr(ctx, "metadata") and ctx.metadata is not None:
        metadata = _to_dict(ctx.metadata)

    if not metadata and agent_id_u64 is not None:
        import sys

        globals_mod = sys.modules.get("_agy_bridge_globals")
        if globals_mod and hasattr(globals_mod, "LAST_TOOL_METADATA"):
            cached_meta = globals_mod.LAST_TOOL_METADATA.get(int(agent_id_u64))
            if cached_meta:
                metadata = _to_dict(cached_meta)

    tool_name = getattr(ctx, "name", "")
    if not tool_name and hasattr(ctx, "tool_name"):
        tool_name = ctx.tool_name
    if not tool_name and current_tool_call:
        tool_name = getattr(current_tool_call, "name", "")
    tool_name = _normalize_tool_name(tool_name)

    payload = {
        "name": tool_name,
        "args": tool_args,
        "result": result_str,
        "metadata": metadata,
    }
    return json.dumps(payload)


def _serialize_on_tool_error_ctx(
    ctx: Any, current_tool_call: Any, hook_logger: Any
) -> str:
    """Serialize an `on_tool_error` hook context to a JSON string."""
    import json

    tool_name = getattr(current_tool_call, "name", "") if current_tool_call else ""
    tool_args = getattr(current_tool_call, "args", {}) if current_tool_call else {}
    tool_args = _to_dict(tool_args)

    tool_name = _normalize_tool_name(tool_name)

    payload = {
        "tool_name": tool_name,
        "tool_args": tool_args,
        "error": str(ctx),
    }
    try:
        err_metadata = getattr(ctx, "metadata", None)
        if err_metadata is not None:
            err_metadata = _to_dict(err_metadata)
            payload["metadata"] = err_metadata
    except Exception:
        hook_logger.warning(
            "Failed to extract metadata from on_tool_error context",
            exc_info=True,
        )
    return json.dumps(payload)


def _serialize_session_ctx(local_config: dict[str, Any], agent_id: int) -> str:
    """Serialize an on_session_start/on_session_end payload to a JSON string."""
    import json

    conversation_id = local_config.get("conversation_id")
    if not conversation_id:
        workspaces = local_config.get("workspaces")
        if (
            workspaces
            and isinstance(workspaces, list)
            and len(workspaces) > 0
            and workspaces[0]
        ):
            ws_str = str(workspaces[0]).rstrip("/\\").replace("\\", "/")
            conversation_id = ws_str.rsplit("/", 1)[-1]
    if not conversation_id:
        conversation_id = "default_session"
    payload = {
        "session": {
            "session_id": str(conversation_id),
            "agent_id": int(agent_id),
        }
    }
    return json.dumps(payload)


def _serialize_post_turn_ctx(ctx: Any) -> str:
    """Serialize a `post_turn` hook context to a JSON string."""
    import json

    text_val = getattr(ctx, "text", str(ctx))
    return json.dumps(
        {
            "response_text": text_val,
            "turn_number": getattr(ctx, "turn_number", 0),
        }
    )


def _serialize_pre_turn_ctx(ctx: Any) -> str:
    """Serialize a `pre_turn` hook context to a JSON string."""
    import json

    text_val = ctx if isinstance(ctx, str) else str(ctx)
    return json.dumps(
        {
            "prompt": text_val,
            "turn_number": getattr(ctx, "turn_number", 0),
        }
    )


def _serialize_stop_args(ctx: Any) -> str:
    """Serialize a `stop` hook StopArgs context to a JSON string."""
    import json

    if ctx is None:
        return "{}"
    stop_reason_val = getattr(ctx, "stop_reason", None)
    if stop_reason_val is not None and hasattr(stop_reason_val, "value"):
        stop_reason_str = str(stop_reason_val.value)
    elif stop_reason_val is not None:
        stop_reason_str = str(stop_reason_val)
    else:
        stop_reason_str = "UNSPECIFIED"

    payload = {
        "response_text": getattr(ctx, "response_text", "") or "",
        "trajectory_id": getattr(ctx, "trajectory_id", "") or "",
        "continuation_count": int(getattr(ctx, "continuation_count", 0) or 0),
        "stop_reason": stop_reason_str,
        "error_message": getattr(ctx, "error_message", "") or "",
    }
    return json.dumps(payload)


def _serialize_generic_ctx(ctx: Any) -> str:
    """Serialize an arbitrary hook context to a JSON string."""
    import json

    if isinstance(ctx, str):
        return json.dumps({"value": ctx})
    elif hasattr(ctx, "model_dump_json"):
        return ctx.model_dump_json()
    elif isinstance(ctx, dict):
        return json.dumps(ctx)
    else:
        return json.dumps(str(ctx))


def _build_model_target(
    entry: str | dict[str, Any],
    default_api_key: str | None = None,
    base_url: str | None = None,
    is_image: bool = False,
) -> Any:
    """Convert a serialized Rust `ModelEntry` dict or model name string into an SDK `ModelTarget`."""
    if isinstance(entry, str):
        if not is_image and not base_url:
            return entry
        entry = {"name": entry}

    if not isinstance(entry, dict):
        return entry

    name = entry.get("name")
    entry_api_key = entry.get("api_key") or default_api_key
    gen = entry.get("generation") or {}

    try:
        from google.antigravity import models as _sdk_models
        from google.antigravity.types import (
            GeminiAPIEndpoint,
            GeminiModelOptions,
            ModelTarget,
            ModelType,
            ServiceTier,
            ThinkingLevel,
        )

        if not name:
            name = (
                _sdk_models.DEFAULT_IMAGE_GENERATION_MODEL
                if is_image
                else _sdk_models.DEFAULT_MODEL
            )

        options = None
        if isinstance(gen, dict) and (
            gen.get("thinking_level")
            or gen.get("service_tier")
            or gen.get("max_output_tokens") is not None
        ):
            tl_raw = gen.get("thinking_level")
            st_raw = gen.get("service_tier")
            mot_raw = gen.get("max_output_tokens")
            tl = ThinkingLevel(str(tl_raw).lower()) if tl_raw else None
            st = ServiceTier(str(st_raw).lower()) if st_raw else None
            mot = int(mot_raw) if mot_raw is not None and int(mot_raw) > 0 else None
            opt_kwargs: dict[str, Any] = {"thinking_level": tl, "service_tier": st}
            if mot is not None:
                opt_kwargs["max_output_tokens"] = mot
            options = GeminiModelOptions(**opt_kwargs)

        endpoint = None
        if base_url or entry_api_key or options is not None:
            endpoint = GeminiAPIEndpoint(
                base_url=base_url, api_key=entry_api_key, options=options
            )

        model_types = [ModelType.IMAGE] if is_image else [ModelType.TEXT]
        return ModelTarget(name=name, types=model_types, endpoint=endpoint)
    except (ImportError, RuntimeError, TypeError, ValueError, KeyError, AttributeError):
        return name


def _munge_config_model(
    local_config: dict[str, Any], custom_base_url: str | None = None
) -> None:
    """Normalize gemini_config into LocalAgentConfig top-level fields (model, models, api_key)."""
    if "gemini_config" in local_config:
        gemini_cfg = local_config.pop("gemini_config")
        if gemini_cfg:
            top_api_key = gemini_cfg.get("api_key")
            if top_api_key:
                local_config["api_key"] = top_api_key
            models = gemini_cfg.get("models")
            if isinstance(models, dict) and models:
                default_entry = models.get("default")
                image_entry = models.get("image_generation")
                if default_entry and image_entry:
                    t_default = _build_model_target(
                        default_entry,
                        default_api_key=top_api_key,
                        base_url=custom_base_url,
                        is_image=False,
                    )
                    t_image = _build_model_target(
                        image_entry,
                        default_api_key=top_api_key,
                        base_url=custom_base_url,
                        is_image=True,
                    )
                    if not isinstance(t_default, str) and not isinstance(t_image, str):
                        local_config["models"] = [t_default, t_image]
                        local_config.pop("model", None)
                    else:
                        local_config["model"] = t_default
                elif default_entry:
                    if isinstance(default_entry, dict):
                        local_config["model"] = _build_model_target(
                            default_entry,
                            default_api_key=top_api_key,
                            base_url=custom_base_url,
                            is_image=False,
                        )
                    else:
                        local_config["model"] = default_entry
                elif image_entry:
                    t_image = _build_model_target(
                        image_entry,
                        default_api_key=top_api_key,
                        base_url=custom_base_url,
                        is_image=True,
                    )
                    if not isinstance(t_image, str):
                        local_config["models"] = [t_image]
                        local_config.pop("model", None)


def _extract_initial_history(local_config: dict[str, Any]) -> list[dict[str, Any]]:
    """Pop and return `initial_history` from the config."""
    return local_config.pop("initial_history", [])


_WS_MAXSIZE_PATCH_SDK_VERSION = "0.1.10"
_WS_MAXSIZE_DEFAULT_CAP = 128 * 1024 * 1024


def _resolve_installed_sdk_version():
    """Return the installed ``google-antigravity`` version, or None if unknown."""
    try:
        import importlib.metadata as _meta

        return _meta.version("google-antigravity")
    except (ImportError, ValueError, KeyError, OSError):
        return None


def _resolve_ws_max_size(logger):
    """Resolve the inbound WS frame cap from env, falling back to the default.

    Returns an int byte cap, or None for unbounded (env value <= 0).
    """
    import os

    raw = os.environ.get("AGY_WS_MAX_MESSAGE_BYTES")
    if not raw:
        return _WS_MAXSIZE_DEFAULT_CAP
    try:
        cap = int(raw)
    except ValueError:
        logger.warning(
            "[MONKEYPATCH] Invalid AGY_WS_MAX_MESSAGE_BYTES=%r; using default %d",
            raw,
            _WS_MAXSIZE_DEFAULT_CAP,
        )
        return _WS_MAXSIZE_DEFAULT_CAP
    return None if cap <= 0 else cap


def _get_monkeypatch_lock():
    import sys
    import threading

    lock = getattr(sys, "_agy_bridge_monkeypatch_lock", None)
    if lock is None:
        lock = threading.Lock()
        sys._agy_bridge_monkeypatch_lock = lock
    return lock


def _patch_websockets_max_size(logger, sdk_version=None):
    """Raise the default websockets frame limit from 1 MiB to 128 MiB for SDK 0.1.10."""
    if sdk_version is None:
        sdk_version = _resolve_installed_sdk_version()

    if sdk_version != _WS_MAXSIZE_PATCH_SDK_VERSION:
        logger.info(
            "[MONKEYPATCH] Skipping WS max_size patch: SDK version %r != pinned "
            "%r (newer SDKs are expected to carry the upstream fix)",
            sdk_version,
            _WS_MAXSIZE_PATCH_SDK_VERSION,
        )
        return False

    with _get_monkeypatch_lock():
        try:
            import websockets
        except ImportError:
            logger.warning(
                "[MONKEYPATCH] websockets not importable -- WS max_size patch skipped"
            )
            return False

        if getattr(websockets, "_agy_max_size_patched", False):
            return True

        websockets._agy_max_size_patched = True
        cap = _resolve_ws_max_size(logger)
        original_connect = websockets.connect

        def _connect_with_max_size(*args, **kwargs):
            kwargs.setdefault("max_size", cap)
            return original_connect(*args, **kwargs)

        websockets.connect = _connect_with_max_size
        websockets._agy_original_connect = original_connect
        logger.info(
            "[MONKEYPATCH] Patched websockets.connect max_size=%s for SDK %s "
            "(default was 1 MiB; harness echoes ~2x input, breaking large contexts)",
            cap,
            sdk_version,
        )
        return True


_DEPENDENT_TOOLS_METADATA_KEY = "dependent_tools"
_DEPENDENT_SKILLS_METADATA_KEY = "dependent_skills"
_HISTORY_ROLE_USER = "user"
_HISTORY_ROLE_MODEL = "model"
_HISTORY_ROLE_SYSTEM = "system"
_HISTORY_LOGGER = logging.getLogger("agy_bridge.initial_history")


def _history_message_to_step(message: dict[str, Any], types_mod: Any) -> Any:
    """Convert a serialized Rust `ConversationMessage` into an SDK `Step`."""
    role = str(message["role"]).lower()
    content = str(message["content"])
    step_type = types_mod.StepType.TEXT_RESPONSE
    if role == _HISTORY_ROLE_USER:
        source, target = types_mod.StepSource.USER, types_mod.StepTarget.ENVIRONMENT
    elif role == _HISTORY_ROLE_MODEL:
        source, target = types_mod.StepSource.MODEL, types_mod.StepTarget.USER
    elif role == _HISTORY_ROLE_SYSTEM:
        source, target = types_mod.StepSource.SYSTEM, types_mod.StepTarget.ENVIRONMENT
        step_type = types_mod.StepType.SYSTEM_MESSAGE
    else:
        raise ValueError(f"unsupported initial_history role: {role!r}")
    return types_mod.Step(
        type=step_type,
        source=source,
        target=target,
        status=types_mod.StepStatus.DONE,
        content=content,
        is_complete_response=role == _HISTORY_ROLE_MODEL,
    )


def _seed_conversation_history(
    conversation: Any, initial_history: list[dict[str, Any]], types_mod: Any = None
) -> None:
    """Seed a fresh SDK `Conversation` with prior turns (native-backend parity)."""
    if types_mod is None:
        from google.antigravity import types as sdk_types

        types_mod = sdk_types
    steps = conversation._steps
    if steps:
        _HISTORY_LOGGER.info(
            "Skipping initial_history: conversation already has %d steps", len(steps)
        )
        return
    for message in initial_history:
        step = _history_message_to_step(message, types_mod)
        if step.source == types_mod.StepSource.USER:
            conversation._turn_start_indices.append(len(steps))
        steps.append(step)
    _HISTORY_LOGGER.info(
        "Injected %d initial_history entries into conversation", len(initial_history)
    )


def _fill_skill_proto(skill_proto: Any, skill: Any) -> None:
    """Populate a `genai.skills.Skill` proto from an SDK `InlineSkill`."""
    import json

    skill_proto.name = skill.name
    skill_proto.description = skill.description
    skill_proto.content = skill.content
    if skill.allowed_tools:
        skill_proto.allowed_tools.extend(skill.allowed_tools)
    metadata = dict(skill.metadata)
    if skill.dependent_tools:
        metadata.setdefault(
            _DEPENDENT_TOOLS_METADATA_KEY, json.dumps(list(skill.dependent_tools))
        )
    if skill.dependent_skills:
        metadata.setdefault(
            _DEPENDENT_SKILLS_METADATA_KEY, json.dumps(list(skill.dependent_skills))
        )
    if metadata:
        skill_proto.metadata.update(metadata)


def _subagent_skills_proto_with_inline(
    original: Any, strategy: Any, skills_config: Any
) -> Any:
    """Build a subagent `SubagentSkillsConfig` proto, encoding override inline skills."""
    override = getattr(skills_config, "override_config", None)
    if override is None or not override.inline_skills:
        return original(strategy, skills_config)
    stripped_override = override.model_copy(update={"inline_skills": []})
    stripped = skills_config.model_copy(update={"override_config": stripped_override})
    proto = original(strategy, stripped)
    for skill in override.inline_skills:
        _fill_skill_proto(proto.override_config.skill_sources.add().skill, skill)
    return proto


def _patch_subagent_inline_skills(strategy_cls: Any, logger: logging.Logger) -> bool:
    """Teach `LocalConnectionStrategy` to forward subagent override inline skills."""
    with _get_monkeypatch_lock():
        if getattr(strategy_cls, "_agy_subagent_inline_skills_patched", False):
            return True
        original = getattr(strategy_cls, "_to_subagent_skills_config_proto", None)
        if original is None:
            logger.warning(
                "[MONKEYPATCH] SDK structural drift: LocalConnectionStrategy has no "
                "_to_subagent_skills_config_proto; subagent inline skills unsupported"
            )
            return False

        def _patched(self: Any, skills_config: Any) -> Any:
            return _subagent_skills_proto_with_inline(original, self, skills_config)

        strategy_cls._to_subagent_skills_config_proto = _patched
        strategy_cls._agy_subagent_inline_skills_patched = True
        logger.info(
            "[MONKEYPATCH] Enabled subagent override inline_skills for localharness"
        )
        return True


_SDK_PATCH_ERRORS = (
    RuntimeError,
    TypeError,
    ValueError,
    KeyError,
    AttributeError,
    OSError,
)


def _apply_sdk_monkeypatches(logger):
    """Apply LocalConnection monkeypatches (turn-context/idle race, cascade_id sync, tool normalization)."""
    with _get_monkeypatch_lock():
        try:
            import asyncio
            import sys

            from google.antigravity.connections.local.local_connection import (
                LocalConnection,
            )

            if getattr(LocalConnection, "_is_monkeypatched", False):
                return
            LocalConnection._is_monkeypatched = True

            expected_attrs = ["__init__", "_tool_result_to_dict"]
            missing = [a for a in expected_attrs if not hasattr(LocalConnection, a)]
            if missing:
                logger.warning(
                    "[MONKEYPATCH] SDK structural drift detected! Missing: %s", missing
                )
                return

            MIN_SUPPORTED_SDK_VERSION = "0.1.16"
            try:
                import importlib.metadata as _meta

                sdk_version = _meta.version("google-antigravity")
                logger.info("[MONKEYPATCH] SDK version: %s", sdk_version)
                min_parts = [
                    int(p) for p in MIN_SUPPORTED_SDK_VERSION.split(".") if p.isdigit()
                ]
                ver_parts = [int(p) for p in sdk_version.split(".") if p.isdigit()]
                if ver_parts < min_parts:
                    raise RuntimeError(
                        f"[SDK-VERSION] Installed google-antigravity {sdk_version} < {MIN_SUPPORTED_SDK_VERSION}."
                    )
            except RuntimeError:
                raise
            except (ImportError, *_SDK_PATCH_ERRORS):
                sdk_version = "unknown"

            logger.info(
                "[MONKEYPATCH] Applying LocalConnection fix for turn context and idle event race"
            )

            LocalConnection._real_current_turn_context = None
            LocalConnection._idle_deferred = False

            @property
            def current_turn_context(self):
                return getattr(self, "_real_current_turn_context", None)

            @current_turn_context.setter
            def current_turn_context(self, value):
                self._real_current_turn_context = value
                if value is None and getattr(self, "_idle_deferred", False):
                    self._idle_deferred = False
                    logger.info(
                        "[MONKEYPATCH] Firing deferred is_idle.set() now that _current_turn_context is None"
                    )
                    if hasattr(self, "_processor") and hasattr(
                        self._processor, "is_idle"
                    ):
                        event = getattr(
                            self._processor.is_idle, "_event", self._processor.is_idle
                        )
                        event.set()
                    elif hasattr(self, "_is_idle"):
                        event = getattr(self._is_idle, "_event", self._is_idle)
                        event.set()

            LocalConnection._current_turn_context = current_turn_context

            original_init = LocalConnection.__init__

            def patched_init(self, *args, **kwargs):
                original_init(self, *args, **kwargs)

                class PatchedEvent:
                    def __init__(self, event, conn):
                        self._event = event
                        self._conn = conn

                    def set(self):
                        if (
                            getattr(self._conn, "_current_turn_context", None)
                            is not None
                        ):
                            logger.info(
                                "[MONKEYPATCH] Deferring is_idle.set() because _current_turn_context is not None"
                            )
                            self._conn._idle_deferred = True
                            return
                        self._event.set()

                    def clear(self):
                        self._event.clear()

                    def is_set(self):
                        return self._event.is_set()

                    async def wait(self):
                        await self._event.wait()

                if hasattr(self, "_processor") and hasattr(self._processor, "is_idle"):
                    original_event = self._processor.is_idle
                    self._processor.is_idle = PatchedEvent(original_event, self)
                elif hasattr(self, "_is_idle"):
                    try:
                        original_event = self._is_idle
                        self._is_idle = PatchedEvent(original_event, self)
                    except AttributeError:
                        pass

                self._real_current_turn_context = None
                self._idle_deferred = False

            LocalConnection.__init__ = patched_init

            @property
            def _cascade_id(self):
                return getattr(self, "_real_cascade_id", None)

            @_cascade_id.setter
            def _cascade_id(self, value):
                old_val = getattr(self, "_real_cascade_id", None)
                self._real_cascade_id = value
                if value and value != old_val:
                    agent_id = getattr(self, "_agent_id", None)
                    logger.info(
                        "[MONKEYPATCH] Detected dynamic cascade_id change: %r -> %r for agent %r",
                        old_val,
                        value,
                        agent_id,
                    )
                    if agent_id is not None:
                        globals_mod = sys.modules.get("_agy_bridge_globals")
                        if globals_mod is not None:
                            if hasattr(globals_mod, "set_agent_conversation_id"):
                                try:
                                    globals_mod.set_agent_conversation_id(
                                        int(agent_id), str(value)
                                    )
                                except _SDK_PATCH_ERRORS:
                                    logger.exception(
                                        "[MONKEYPATCH] Failed to store SDK conversation id"
                                    )
                            if hasattr(globals_mod, "dispatch_rust_hook"):
                                payload = {
                                    "session": {
                                        "session_id": str(value),
                                        "agent_id": int(agent_id),
                                    }
                                }
                                import json

                                ctx_json = json.dumps(payload)
                                logger.info(
                                    "[MONKEYPATCH] Syncing dynamic conversation ID %s to Rust for agent %s",
                                    value,
                                    agent_id,
                                )
                                try:
                                    globals_mod.dispatch_rust_hook(
                                        int(agent_id), "on_session_start", ctx_json
                                    )
                                except _SDK_PATCH_ERRORS:
                                    logger.exception(
                                        "[MONKEYPATCH] Failed to sync conversation ID"
                                    )

            LocalConnection._cascade_id = _cascade_id

            original_tool_result_to_dict = LocalConnection._tool_result_to_dict

            def patched_tool_result_to_dict(self, result):
                if result.error is not None:
                    return {"error": result.error}

                output = result.result
                if isinstance(output, dict) and "content" in output:
                    return {"result": output["content"]}

                return original_tool_result_to_dict(self, result)

            LocalConnection._tool_result_to_dict = patched_tool_result_to_dict

            original_receive_steps = LocalConnection.receive_steps

            async def patched_receive_steps(self):
                lock = getattr(self, "_receive_steps_lock", None)
                if lock is None:
                    lock = asyncio.Lock()
                    self._receive_steps_lock = lock
                async with lock:
                    async for step in original_receive_steps(self):
                        yield step

            LocalConnection.receive_steps = patched_receive_steps

            try:
                from google.antigravity.connections.local import event_processor

                LocalHarnessEventProcessor = event_processor.LocalHarnessEventProcessor

                if not getattr(LocalHarnessEventProcessor, "_is_monkeypatched", False):

                    @property
                    def main_trajectory_id(self):
                        return getattr(self, "_real_main_trajectory_id", None)

                    @main_trajectory_id.setter
                    def main_trajectory_id(self, value):
                        old_val = getattr(self, "_real_main_trajectory_id", None)
                        self._real_main_trajectory_id = value
                        if value and value != old_val:
                            agent_id = getattr(self, "_agent_id", None)
                            logger.info(
                                "[MONKEYPATCH] Detected dynamic main_trajectory_id change: %r -> %r for agent %r",
                                old_val,
                                value,
                                agent_id,
                            )
                            if agent_id is not None:
                                globals_mod = sys.modules.get("_agy_bridge_globals")
                                if globals_mod is not None:
                                    if hasattr(
                                        globals_mod, "set_agent_conversation_id"
                                    ):
                                        try:
                                            globals_mod.set_agent_conversation_id(
                                                int(agent_id), str(value)
                                            )
                                        except _SDK_PATCH_ERRORS:
                                            logger.exception(
                                                "[MONKEYPATCH] Failed to store SDK conversation id"
                                            )
                                    if hasattr(globals_mod, "dispatch_rust_hook"):
                                        payload = {
                                            "session": {
                                                "session_id": str(value),
                                                "agent_id": int(agent_id),
                                            }
                                        }
                                        import json

                                        ctx_json = json.dumps(payload)
                                        logger.info(
                                            "[MONKEYPATCH] Syncing dynamic conversation ID %s to Rust for agent %s",
                                            value,
                                            agent_id,
                                        )
                                        try:
                                            globals_mod.dispatch_rust_hook(
                                                int(agent_id),
                                                "on_session_start",
                                                ctx_json,
                                            )
                                        except _SDK_PATCH_ERRORS:
                                            logger.exception(
                                                "[MONKEYPATCH] Failed to sync conversation ID"
                                            )

                    LocalHarnessEventProcessor.main_trajectory_id = main_trajectory_id

                    orig_ep_tool_result = LocalHarnessEventProcessor.tool_result_to_dict

                    def patched_ep_tool_result(self, result):
                        if result.error is not None:
                            return {"error": result.error}
                        output = result.result
                        if isinstance(output, dict) and "content" in output:
                            return {"result": output["content"]}
                        return orig_ep_tool_result(self, result)

                    LocalHarnessEventProcessor.tool_result_to_dict = (
                        patched_ep_tool_result
                    )

                    async def patched_handle_tool_call(self, tool_call):
                        import json

                        from google.antigravity import types
                        from google.antigravity.connections.local import event_processor

                        try:
                            args = json.loads(tool_call.arguments_json or "{}")
                            tc = types.ToolCall(
                                id=tool_call.id, name=tool_call.name, args=args
                            )
                            tool_call_step = event_processor.LocalConnectionStep(
                                id=tool_call.id,
                                step_index=1,
                                type=types.StepType.TOOL_CALL,
                                source=types.StepSource.MODEL,
                                target=types.StepTarget.ENVIRONMENT,
                                status=types.StepStatus.ACTIVE,
                                tool_calls=[tc],
                            )
                            await self.step_queue.put(tool_call_step)

                            if self._tool_runner:
                                try:
                                    results = (
                                        await self._tool_runner.process_tool_calls(
                                            [types.ToolCall(name=tc.name, args=tc.args)]
                                        )
                                    )
                                    result = results[0]
                                    result.id = tool_call.id
                                except _SDK_PATCH_ERRORS as e:
                                    result = types.ToolResult(
                                        id=tool_call.id,
                                        name=tool_call.name,
                                        error=str(e),
                                        exception=e,
                                    )

                                if self._hook_runner:
                                    try:
                                        from google.antigravity.hooks import hooks

                                        turn_ctx = (
                                            self._hook_router.current_turn_context
                                            if self._hook_router
                                            else None
                                        ) or hooks.TurnContext(
                                            self._hook_runner.session_context
                                        )
                                        op_ctx = hooks.OperationContext(turn_ctx)
                                        await self._hook_runner.dispatch_post_tool_call(
                                            op_ctx, result
                                        )
                                    except _SDK_PATCH_ERRORS as hook_err:
                                        logger.warning(
                                            "Error dispatching post_tool_call hook: %s",
                                            hook_err,
                                        )

                                await self._send_tool_results([result])
                            else:
                                logger.warning(
                                    "Received tool call %s but no tool runner is configured. Yielding to user.",
                                    tool_call.name,
                                )
                        except _SDK_PATCH_ERRORS as e:
                            logger.exception(
                                "_handle_tool_call failed; returning error to model"
                            )
                            await self._send_tool_results(
                                [
                                    types.ToolResult(
                                        id=tool_call.id,
                                        name=tool_call.name,
                                        error=f"Internal SDK error: {e!r}",
                                    )
                                ]
                            )

                    LocalHarnessEventProcessor.handle_tool_call = (
                        patched_handle_tool_call
                    )
                    LocalHarnessEventProcessor._is_monkeypatched = True
            except (
                ImportError,
                RuntimeError,
                TypeError,
                ValueError,
                AttributeError,
            ) as e:
                logger.warning("Failed to patch LocalHarnessEventProcessor: %s", e)

            LocalConnection._is_monkeypatched = True
        except ImportError:
            logger.warning(
                "[MONKEYPATCH] google.antigravity.connections.local.local_connection "
                "not importable — LocalConnection patches skipped"
            )
        except RuntimeError:
            raise
        except (TypeError, ValueError, KeyError, AttributeError, OSError) as e:
            logger.warning("Failed to apply LocalConnection monkeypatch: %s", e)


def _wire_tool_proxies(local_config, agent_id_u64):
    """Replace tool/capability dict specs with AsyncRustProxy instances."""
    from google.antigravity.tools import tool_runner

    class AsyncRustProxy(tool_runner.ToolWithSchema):
        def __init__(self, agent_id, name, description, schema_dict):
            self.agent_id = str(agent_id)
            self.__name__ = name
            self.__doc__ = description
            self.input_schema = schema_dict
            self.fn = self.__call__

        async def __call__(self, **kwargs):
            import inspect
            import json
            import sys

            globals_mod = sys.modules.get("_agy_bridge_globals")
            if not globals_mod or not hasattr(globals_mod, "dispatch_rust_tool"):
                raise RuntimeError(
                    "dispatch_rust_tool not found in _agy_bridge_globals"
                )
            if not hasattr(globals_mod, "CURRENT_TOOL_CALLS"):
                globals_mod.CURRENT_TOOL_CALLS = {}

            class DummyToolCall:
                def __init__(self, name, args):
                    self.name = name
                    self.args = args

            globals_mod.CURRENT_TOOL_CALLS[int(self.agent_id)] = DummyToolCall(
                self.__name__, kwargs
            )
            args_json = json.dumps(kwargs)

            res = globals_mod.dispatch_rust_tool(
                int(self.agent_id), self.__name__, args_json
            )
            if inspect.isawaitable(res):
                res = await res
            if isinstance(res, dict) and "metadata" in res:
                if not hasattr(globals_mod, "LAST_TOOL_METADATA"):
                    globals_mod.LAST_TOOL_METADATA = {}
                globals_mod.LAST_TOOL_METADATA[int(self.agent_id)] = res.get("metadata")
            return res

    def create_proxy(agent_id, name, desc, schema):
        return AsyncRustProxy(agent_id, name, desc, schema)

    if "tools" in local_config:
        proxies = []
        for t in local_config["tools"]:
            if isinstance(t, dict) and "name" in t:
                proxies.append(
                    create_proxy(
                        agent_id_u64,
                        t["name"],
                        t.get("description", ""),
                        t.get("parameter_schema", {}),
                    )
                )
            else:
                proxies.append(t)
        local_config["tools"] = proxies

    if (
        "capabilities" in local_config
        and local_config["capabilities"]
        and local_config["capabilities"].get("enabled_tools") is not None
    ):
        proxies = []
        for t in local_config["capabilities"].get("enabled_tools") or []:
            if isinstance(t, dict) and "name" in t:
                proxies.append(
                    create_proxy(
                        agent_id_u64,
                        t["name"],
                        t.get("description", ""),
                        t.get("parameter_schema", {}),
                    )
                )
            else:
                proxies.append(t)
        local_config["capabilities"]["enabled_tools"] = proxies

    if "capabilities" in local_config and local_config["capabilities"] is None:
        del local_config["capabilities"]


def _coerce_builtin_tool(item: Any) -> Any:
    """Convert a string tool name into `BuiltinTools` when it matches a known builtin."""
    if not isinstance(item, str):
        return item
    try:
        from google.antigravity.types import BuiltinTools

        invoke_tool = getattr(BuiltinTools, "INVOKE_SUBAGENT", None)
        if invoke_tool is None:
            invoke_tool = getattr(BuiltinTools, "START_SUBAGENT", item)
        alias_map = {
            "read_url_content": BuiltinTools.READ_URL_CONTENT,
            "search_web": BuiltinTools.SEARCH_WEB,
            "start_subagent": invoke_tool,
            "invoke_subagent": invoke_tool,
        }
        if item in alias_map:
            return alias_map[item]
        return BuiltinTools(item)
    except (ImportError, TypeError, ValueError, KeyError, AttributeError):
        return item
