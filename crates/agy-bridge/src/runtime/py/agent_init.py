# DRY helpers shared across hook context serialization.
#
# These module-level helpers are pure and importable without the live
# antigravity SDK (all SDK imports happen lazily inside the section-wiring
# functions and inside `init_agent`), so they can be unit-tested with plain
# pytest.

from __future__ import annotations

import os
from typing import Any

if os.name == "nt" and not os.environ.get("USERPROFILE"):
    import tempfile

    os.environ["USERPROFILE"] = os.environ.get("HOME") or tempfile.gettempdir()

# Hook points whose callbacks must return a `HookResult` (allow/deny gate).
RESULT_HOOK_POINTS: tuple[str, ...] = (
    "pre_turn",
    "pre_tool_call_decide",
    "on_interaction",
)

# Placeholder API key used when a custom base_url (proxy/gateway) handles auth
# itself, so no real key is required. It only needs to satisfy the SDK's
# non-empty API-key validation; it is cleared in `_build_harness_config` before
# the actual RPC, so its literal value is arbitrary and never sent anywhere.
_PROXY_AUTH_SENTINEL: str = "__agy_proxy_auth__"


def _to_dict(obj: Any) -> Any:
    """Best-effort conversion of a pydantic-like object to a plain dict.

    Falls back to returning the object unchanged when it exposes neither
    `model_dump` (pydantic v2) nor `dict` (pydantic v1).
    """
    if hasattr(obj, "model_dump"):
        return obj.model_dump()
    elif hasattr(obj, "dict"):
        return obj.dict()
    return obj


def _normalize_tool_name(name: Any) -> str:
    """Normalize a tool name to a plain string.

    Handles enum-like values (`.value`) and non-string names (`str()`).
    """
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
    # Best-effort: surface structured metadata
    # attached to the error context so errors
    # raised on the Python side still deliver
    # metadata to on_tool_error. Rust-side
    # ToolError metadata is merged
    # authoritatively in `handle_on_tool_error`;
    # this only helps errors that never reach
    # the Rust dispatch path.
    try:
        err_metadata = getattr(ctx, "metadata", None)
        if err_metadata is not None:
            err_metadata = _to_dict(err_metadata)
            payload["metadata"] = err_metadata
    except Exception:
        hook_logger.warning(
            "Failed to extract metadata from " "on_tool_error context",
            exc_info=True,
        )
    return json.dumps(payload)


def _serialize_session_ctx(local_config: dict[str, Any], agent_id: int) -> str:
    """Serialize an on_session_start/on_session_end payload to a JSON string.

    Falls back to a workspace-derived or `default_session` conversation id when
    the config does not carry one.
    """
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
    """Serialize an arbitrary hook context to a JSON string.

    Handles plain strings, pydantic models (`model_dump_json`), dicts, and an
    opaque `str()` fallback.
    """
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
    """Convert a serialized Rust `ModelEntry` dict or model name string into an SDK `ModelTarget`.

    Falls back to returning the plain model string when the SDK types module is
    not installed or when `entry` is already a string without extra options.
    """
    if isinstance(entry, str):
        if not is_image and not base_url:
            return entry
        entry = {"name": entry}

    if not isinstance(entry, dict):
        return entry

    name = entry.get("name") or "gemini-3-flash-preview"
    entry_api_key = entry.get("api_key") or default_api_key
    gen = entry.get("generation") or {}

    try:
        from google.antigravity.types import (
            GeminiAPIEndpoint,
            GeminiModelOptions,
            ModelTarget,
            ModelType,
            ServiceTier,
            ThinkingLevel,
        )

        options = None
        if isinstance(gen, dict) and (
            gen.get("thinking_level") or gen.get("service_tier")
        ):
            tl_raw = gen.get("thinking_level")
            st_raw = gen.get("service_tier")
            tl = ThinkingLevel(str(tl_raw).lower()) if tl_raw else None
            st = ServiceTier(str(st_raw).lower()) if st_raw else None
            options = GeminiModelOptions(thinking_level=tl, service_tier=st)

        endpoint = None
        if base_url or entry_api_key or options is not None:
            endpoint = GeminiAPIEndpoint(
                base_url=base_url, api_key=entry_api_key, options=options
            )

        model_types = [ModelType.IMAGE] if is_image else [ModelType.TEXT]
        return ModelTarget(name=name, types=model_types, endpoint=endpoint)
    except Exception:
        return name


def _munge_config_model(
    local_config: dict[str, Any], custom_base_url: str | None = None
) -> None:
    """Normalize gemini_config into LocalAgentConfig top-level fields (model, models, api_key).

    In SDK 0.1.17+, `LocalAgentConfig` accepts `model: str | ModelTarget | None`
    (single shorthand model) and `models: list[ModelTarget] | None` (explicit
    multi-model list). Map `gemini_config.api_key` and `gemini_config.models`
    (`default` and optional `image_generation`) to strongly-typed `ModelTarget`
    objects and remove the obsolete `gemini_config` sub-dict.
    """
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
    """Pop and return `initial_history` from the config.

    Extract initial_history before passing config to the SDK.
    The SDK does not know about this field — we inject it into the
    conversation's internal _history list after agent creation.
    """
    return local_config.pop("initial_history", [])


# The exact Antigravity SDK version whose LocalConnection WebSocket client calls
# ``websockets.connect()`` WITHOUT an explicit ``max_size``, so inbound frames
# are capped at the library default of 1 MiB. The local harness echoes
# conversation state back to the client at roughly 2x the input size, so any turn
# whose input exceeds ~512 KiB produces a response frame larger than 1 MiB. The
# client then aborts the socket with a 1009 (message too big) close, the harness
# exits, and the SDK surfaces it as ``WS close code 1006`` -- silently breaking
# every large-context turn.
#
# This patch is pinned to the EXACT version we verified. Newer SDK releases are
# expected to carry the upstream fix (pass ``max_size`` themselves), so we
# deliberately leave any other version untouched to avoid masking a real change.
_WS_MAXSIZE_PATCH_SDK_VERSION = "0.1.10"

# Generous but BOUNDED inbound frame cap (128 MiB). This comfortably covers even
# ~1M-token contexts (harness response ~2x input) while still guarding against a
# runaway/corrupt frame exhausting memory. Override via the
# ``AGY_WS_MAX_MESSAGE_BYTES`` env var; a value <= 0 selects unbounded (None).
_WS_MAXSIZE_DEFAULT_CAP = 128 * 1024 * 1024


def _resolve_installed_sdk_version():
    """Return the installed ``google-antigravity`` version, or None if unknown."""
    try:
        import importlib.metadata as _meta

        return _meta.version("google-antigravity")
    except Exception:
        # PackageNotFoundError (e.g. in pure unit tests) or any metadata error:
        # treat as "unknown version" so the version-gated patch is skipped.
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
    import sys, threading

    lock = getattr(sys, "_agy_bridge_monkeypatch_lock", None)
    if lock is None:
        lock = threading.Lock()
        sys._agy_bridge_monkeypatch_lock = lock
    return lock


def _patch_websockets_max_size(logger, sdk_version=None):
    """Raise the default websockets frame limit from 1 MiB to 128 MiB for SDK 0.1.10.

    Scoped strictly to the one SDK release that suffers from the issue,
    ``_WS_MAXSIZE_PATCH_SDK_VERSION``; any other version is left untouched on the
    assumption that newer releases carry the upstream fix. Idempotent across
    repeated ``init_agent`` calls. Returns True iff the patch is now in effect.
    """
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
            # Only inject the cap when the SDK did not specify one, so an explicit
            # future SDK setting always wins.
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


def _apply_sdk_monkeypatches(logger):
    """Apply the LocalConnection monkeypatches (turn-context/idle-event race,
    dynamic cascade_id sync, tool-result normalization).

    Before patching, we verify that the target class has the expected internal
    structure.  If the SDK has been refactored and the expected attributes are
    missing, the patches are skipped with a warning rather than silently
    corrupting class internals.
    """
    with _get_monkeypatch_lock():
        try:
            import sys
            import asyncio
            from google.antigravity.connections.local.local_connection import (
                LocalConnection,
            )

            if getattr(LocalConnection, "_is_monkeypatched", False):
                return
            LocalConnection._is_monkeypatched = True

            # ── Structural guard ──
            # Verify the class still has the internals we patch.
            # If the SDK refactors these away, patching would silently break.
            expected_attrs = ["__init__", "_tool_result_to_dict"]
            missing = [a for a in expected_attrs if not hasattr(LocalConnection, a)]
            if missing:
                logger.warning(
                    "[MONKEYPATCH] SDK structural drift detected! "
                    "LocalConnection is missing expected attributes: %s. "
                    "Skipping monkeypatches — the SDK may have been updated "
                    "past the version these patches were written for.",
                    missing,
                )
                return

            # Check SDK version if available, for diagnostic logging and min version enforcement.
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
                        f"[SDK-VERSION] Installed google-antigravity version {sdk_version} is older than "
                        f"minimum supported {MIN_SUPPORTED_SDK_VERSION}. Please upgrade: pip install --upgrade google-antigravity>={MIN_SUPPORTED_SDK_VERSION}."
                    )
            except RuntimeError:
                raise
            except Exception:
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
                # When the turn context is cleared, fire any deferred idle signal.
                if value is None and getattr(self, "_idle_deferred", False):
                    self._idle_deferred = False
                    logger.info(
                        "[MONKEYPATCH] Firing deferred is_idle.set() now that _current_turn_context is None"
                    )
                    # Access the real asyncio.Event inside PatchedEvent to bypass the guard.
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
                            # Store the harness-assigned conversation id into the
                            # agent's shared bridge state so
                            # `AgentHandle::conversation_id` and custom-tool
                            # `ToolContext` observe the *real* SDK trajectory id.
                            # This is deliberately separate from the observe-only
                            # `on_session_start` hook below, whose init-time
                            # dispatch may carry a fabricated fallback id.
                            if hasattr(globals_mod, "set_agent_conversation_id"):
                                try:
                                    globals_mod.set_agent_conversation_id(
                                        int(agent_id), str(value)
                                    )
                                except Exception as e:
                                    logger.error(
                                        "[MONKEYPATCH] Failed to store SDK conversation id: %s",
                                        e,
                                        exc_info=True,
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
                                except Exception as e:
                                    logger.error(
                                        "[MONKEYPATCH] Failed to sync conversation ID: %s",
                                        e,
                                        exc_info=True,
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
                                        except Exception as e:
                                            logger.error(
                                                "[MONKEYPATCH] Failed to store SDK conversation id: %s",
                                                e,
                                                exc_info=True,
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
                                        except Exception as e:
                                            logger.error(
                                                "[MONKEYPATCH] Failed to sync conversation ID: %s",
                                                e,
                                                exc_info=True,
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

                    orig_handle_tool_call = LocalHarnessEventProcessor.handle_tool_call

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
                                except Exception as e:
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
                                    except Exception as hook_err:
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
                        except Exception as e:
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
            except Exception as e:
                logger.warning("Failed to patch LocalHarnessEventProcessor: %s", e)

            LocalConnection._is_monkeypatched = True
        except ImportError:
            logger.warning(
                "[MONKEYPATCH] google.antigravity.connections.local.local_connection "
                "not importable — LocalConnection patches skipped"
            )
        except RuntimeError:
            raise
        except Exception as e:
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
            import sys, json, asyncio, inspect

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

            # PyO3 future_into_py returns an awaitable, but iscoroutinefunction is False.
            # We must call it on the main event loop thread, not in a thread pool.
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

        alias_map = {
            "read_url_content": BuiltinTools.READ_URL_CONTENT,
            "search_web": BuiltinTools.SEARCH_WEB,
        }
        if item in alias_map:
            return alias_map[item]
        return BuiltinTools(item)
    except Exception:
        return item


def _normalize_capabilities(local_config: dict[str, Any]) -> None:
    """Normalize capabilities fields (agent_behavior, allowed_subagents, run_command_config, tool_output_truncation_config, builtin tools)."""
    if "capabilities" in local_config and local_config["capabilities"]:
        caps = local_config["capabilities"]
        if isinstance(caps, dict):
            if "compaction_threshold" in caps and caps["compaction_threshold"] is None:
                caps.pop("compaction_threshold", None)
            if "agent_behavior" in caps and isinstance(caps["agent_behavior"], str):
                try:
                    from google.antigravity.types import AgentBehavior

                    caps["agent_behavior"] = AgentBehavior(
                        caps["agent_behavior"].lower()
                    )
                except Exception:
                    pass
            if "run_command_config" in caps and isinstance(
                caps["run_command_config"], dict
            ):
                try:
                    from google.antigravity.types import RunCommandConfig

                    caps["run_command_config"] = RunCommandConfig(
                        **caps["run_command_config"]
                    )
                except Exception:
                    pass
            if (
                "tool_output_truncation" in caps
                and "tool_output_truncation_config" not in caps
            ):
                caps["tool_output_truncation_config"] = caps.pop(
                    "tool_output_truncation"
                )
            if "tool_output_truncation_config" in caps and isinstance(
                caps["tool_output_truncation_config"], dict
            ):
                try:
                    from google.antigravity.types import ToolOutputTruncationConfig

                    caps["tool_output_truncation_config"] = ToolOutputTruncationConfig(
                        **caps["tool_output_truncation_config"]
                    )
                except Exception:
                    pass
            if "enabled_tools" in caps and isinstance(caps["enabled_tools"], list):
                caps["enabled_tools"] = [
                    _coerce_builtin_tool(t) for t in caps["enabled_tools"]
                ]
            if "disabled_tools" in caps and isinstance(caps["disabled_tools"], list):
                caps["disabled_tools"] = [
                    _coerce_builtin_tool(t) for t in caps["disabled_tools"]
                ]


def _wire_capabilities(local_config: dict[str, Any]) -> None:
    """Convert normalized capabilities dict into a strongly-typed SDK `CapabilitiesConfig`."""
    _normalize_capabilities(local_config)
    if "capabilities" in local_config and isinstance(
        local_config["capabilities"], dict
    ):
        try:
            from google.antigravity.types import CapabilitiesConfig, RunCommandConfig

            if hasattr(CapabilitiesConfig, "model_rebuild"):
                CapabilitiesConfig.model_rebuild()
            caps_dict = dict(local_config["capabilities"])
            # Translate Rust-only `command_timeout_ms` if `run_command_config` is unset
            cmd_timeout_ms = caps_dict.pop("command_timeout_ms", None)
            if (
                cmd_timeout_ms is not None
                and caps_dict.get("run_command_config") is None
            ):
                caps_dict["run_command_config"] = RunCommandConfig(
                    timeout_seconds=float(cmd_timeout_ms) / 1000.0
                )
            # Strip Rust-only `image_model` field so CapabilitiesConfig validation succeeds
            caps_dict.pop("image_model", None)
            if hasattr(CapabilitiesConfig, "model_fields"):
                allowed_keys = set(CapabilitiesConfig.model_fields.keys())
                caps_dict = {k: v for k, v in caps_dict.items() if k in allowed_keys}
            local_config["capabilities"] = CapabilitiesConfig(**caps_dict)
        except Exception:
            pass


def _wire_subagents(local_config: dict[str, Any]) -> None:
    """Translate serialized subagent specs into SDK SubagentConfig objects."""
    if "subagents" in local_config and local_config["subagents"]:
        try:
            from google.antigravity.types import (
                AgentBehavior,
                RunCommandConfig,
                SubagentCapabilities,
                SubagentConfig,
            )

            if hasattr(SubagentCapabilities, "model_rebuild"):
                SubagentCapabilities.model_rebuild()
            if hasattr(SubagentConfig, "model_rebuild"):
                SubagentConfig.model_rebuild()
        except Exception:
            return

        parsed_subagents = []
        for s in local_config["subagents"]:
            if isinstance(s, dict):
                caps = None
                if "capabilities" in s and s["capabilities"]:
                    c_dict = s["capabilities"]
                    beh = c_dict.get("agent_behavior", "autonomous")
                    if isinstance(beh, str):
                        try:
                            beh = AgentBehavior(beh.lower())
                        except (ValueError, KeyError):
                            beh = AgentBehavior.AUTONOMOUS
                    rc_cfg = None
                    if "run_command_config" in c_dict and isinstance(
                        c_dict["run_command_config"], dict
                    ):
                        try:
                            rc_cfg = RunCommandConfig(**c_dict["run_command_config"])
                        except Exception:
                            pass
                    enabled = c_dict.get("enabled_tools")
                    if isinstance(enabled, list):
                        enabled = [_coerce_builtin_tool(t) for t in enabled]
                    disabled = c_dict.get("disabled_tools")
                    if isinstance(disabled, list):
                        disabled = [_coerce_builtin_tool(t) for t in disabled]
                    caps = SubagentCapabilities(
                        agent_behavior=beh,
                        allowed_subagents=c_dict.get("allowed_subagents"),
                        enabled_tools=enabled,
                        disabled_tools=disabled,
                        run_command_config=rc_cfg,
                    )
                parsed_subagents.append(
                    SubagentConfig(
                        name=s["name"],
                        description=s.get("description", ""),
                        system_instructions=s.get("system_instructions"),
                        capabilities=caps,
                        tools=s.get("tools", []),
                    )
                )
            else:
                parsed_subagents.append(s)
        local_config["subagents"] = parsed_subagents


def _wire_budget_config(local_config: dict[str, Any]) -> None:
    """Translate serialized budget config into SDK BudgetConfig object."""
    if "budget_config" not in local_config:
        return
    bc = local_config["budget_config"]
    if bc is None:
        local_config.pop("budget_config", None)
        return
    if isinstance(bc, dict):
        try:
            from google.antigravity.types import BudgetConfig, BudgetScope

            bc_copy = dict(bc)
            if "scope" in bc_copy and isinstance(bc_copy["scope"], str):
                try:
                    bc_copy["scope"] = BudgetScope(bc_copy["scope"].upper())
                except ValueError:
                    pass
            local_config["budget_config"] = BudgetConfig(**bc_copy)
        except ImportError:
            return


def _wire_compaction_config(local_config: dict[str, Any]) -> None:
    """Translate serialized compaction config into SDK CompactionConfig object."""
    if "compaction_config" not in local_config:
        return
    cc = local_config["compaction_config"]
    if cc is None:
        local_config.pop("compaction_config", None)
        return
    if isinstance(cc, dict):
        try:
            from google.antigravity.types import CompactionConfig

            token_threshold = cc.get("token_threshold")
            if token_threshold is not None and int(token_threshold) > 0:
                local_config["compaction_config"] = CompactionConfig(
                    token_threshold=int(token_threshold)
                )
            else:
                local_config.pop("compaction_config", None)
        except ImportError:
            return


def _wire_retry_config(local_config: dict[str, Any]) -> None:
    """Translate serialized retry config into SDK RetryConfig object."""
    if "retry_config" not in local_config:
        return
    rc = local_config["retry_config"]
    if rc is None:
        local_config.pop("retry_config", None)
        return
    if isinstance(rc, dict):
        try:
            from google.antigravity.types import (
                ModelAPIRetryConfig,
                ModelOutputRetryConfig,
                RetryConfig,
            )

            api_retry = rc.get("api_retry")
            if isinstance(api_retry, dict):
                api_retry = ModelAPIRetryConfig(**api_retry)
            out_retry = rc.get("model_output_retry")
            if isinstance(out_retry, dict):
                out_retry = ModelOutputRetryConfig(**out_retry)
            kwargs: dict[str, Any] = {}
            if api_retry is not None:
                kwargs["api_retry"] = api_retry
            if out_retry is not None:
                kwargs["model_output_retry"] = out_retry
            local_config["retry_config"] = RetryConfig(**kwargs)
        except ImportError:
            return


def _wire_system_instructions(local_config: dict[str, Any]) -> None:
    """Translate serialized system_instructions into SDK Custom/TemplatedSystemInstructions."""
    if "system_instructions" not in local_config:
        return
    si = local_config["system_instructions"]
    if si is None:
        local_config.pop("system_instructions", None)
        return
    try:
        from google.antigravity.types import (
            CustomSystemInstructions,
            SystemInstructionSection,
            TemplatedSystemInstructions,
        )

        if isinstance(si, str):
            local_config["system_instructions"] = CustomSystemInstructions(text=si)
        elif isinstance(si, dict):
            if ("text" in si or "custom_instructions" in si) and "sections" not in si:
                text_val = si.get("text") or si.get("custom_instructions") or ""
                local_config["system_instructions"] = CustomSystemInstructions(
                    text=text_val
                )
            elif "sections" in si or "identity" in si:
                sections_raw = si.get("sections") or []
                sections = [
                    SystemInstructionSection(**sec) if isinstance(sec, dict) else sec
                    for sec in sections_raw
                ]
                identity_val = si.get("identity")
                if identity_val is None and "base_instructions" in si:
                    identity_val = si.get("base_instructions")
                local_config["system_instructions"] = TemplatedSystemInstructions(
                    identity=identity_val,
                    sections=sections,
                )
    except ImportError:
        return


def _wire_policies(local_config, agent_id_u64):
    """Translate serialized policy specs into SDK policy objects."""
    import logging

    from google.antigravity.hooks import policy

    if "policies" in local_config:
        parsed_policies = []
        policy_logger = logging.getLogger("agy_bridge.policies")
        for p in local_config["policies"]:
            if isinstance(p, str):
                if p == "AllowAll":
                    parsed_policies.append(policy.allow_all())
                elif p == "DenyAll":
                    parsed_policies.append(policy.deny_all())
                else:
                    policy_logger.warning("Unknown string policy %r, skipping", p)
            elif isinstance(p, dict) and "Allow" in p:
                parsed_policies.append(policy.allow(p["Allow"]))
            elif isinstance(p, dict) and "Deny" in p:
                parsed_policies.append(policy.deny(p["Deny"]))
            elif isinstance(p, dict) and "AskUser" in p:

                async def _rust_confirm_handler(tc):
                    import sys, json, inspect, asyncio

                    globals_mod = sys.modules.get("_agy_bridge_globals")
                    if not globals_mod or not hasattr(
                        globals_mod, "dispatch_rust_policy_confirm"
                    ):
                        policy_logger.warning(
                            "dispatch_rust_policy_confirm not found in globals module, falling back to console input"
                        )
                        print(
                            f"\nAgent requested to run tool '{tc.name}' with args {dict(tc.args)}"
                        )
                        try:
                            ans = await asyncio.to_thread(input, "Allow? [Y/n]: ")
                            return ans.strip().lower() in ("", "y", "yes")
                        except (EOFError, OSError):
                            return False

                    tc_args_json = json.dumps(dict(tc.args))
                    res = globals_mod.dispatch_rust_policy_confirm(
                        int(agent_id_u64), tc.name, tc_args_json
                    )
                    if inspect.isawaitable(res):
                        return await res
                    return res

                parsed_policies.append(
                    policy.ask_user(p["AskUser"]["tool"], handler=_rust_confirm_handler)
                )
            elif isinstance(p, dict) and "WorkspaceOnly" in p:
                pass  # Handled by SDK via LocalAgentConfig.workspaces field
            else:
                policy_logger.warning("Unknown policy type %r, skipping", p)
        local_config["policies"] = parsed_policies


def _wire_hooks(local_config, agent_id_u64):
    """Register Rust-backed hook callbacks with the SDK."""
    import logging
    import sys

    # --- Wire hooks ---
    hooks_entries = local_config.pop("hooks", [])

    if hooks_entries:
        try:
            from google.antigravity.hooks import hooks as hooks_module

            hook_logger = logging.getLogger("agy_bridge.hooks")

            import re

            registered_hooks = []
            for entry in hooks_entries:
                try:
                    hook_name = entry.get("name", "unnamed")
                    hook_point = entry.get("point", "")
                    # Translate CamelCase (e.g. PreTurn) to snake_case (e.g. pre_turn)
                    sdk_point = re.sub(r"(?<!^)(?=[A-Z])", "_", hook_point).lower()

                    decorator = getattr(hooks_module, sdk_point, None)
                    if decorator is None:
                        hook_logger.warning(
                            "Unknown or unsupported hook point %r (translated to %r) for hook %r, skipping",
                            hook_point,
                            sdk_point,
                            hook_name,
                        )
                        continue

                    def _make_hook_cb(name, point_label):
                        """Factory to capture name/point_label per hook."""

                        async def _hook_callback(context=None, data=None):
                            import sys, json, inspect

                            ctx = data if data is not None else context
                            globals_mod = sys.modules.get("_agy_bridge_globals")
                            if not globals_mod or not hasattr(
                                globals_mod, "dispatch_rust_hook"
                            ):
                                hook_logger.warning(
                                    "dispatch_rust_hook not found in _agy_bridge_globals, skipping hook %r",
                                    name,
                                )
                                if point_label in RESULT_HOOK_POINTS:
                                    return hooks_module.HookResult(
                                        allow=False,
                                        message="dispatch_rust_hook not found in _agy_bridge_globals",
                                    )
                                return

                            if point_label == "pre_tool_call_decide":
                                if globals_mod:
                                    if not hasattr(globals_mod, "CURRENT_TOOL_CALLS"):
                                        globals_mod.CURRENT_TOOL_CALLS = {}
                                    globals_mod.CURRENT_TOOL_CALLS[agent_id_u64] = ctx

                            # Map SDK context types to JSON for the Rust hook handler.
                            # The SDK passes known pydantic types: ToolCall, ToolResult,
                            # Content (str or BaseModel), or None (session hooks).
                            try:
                                if point_label in (
                                    "on_session_start",
                                    "on_session_end",
                                ):
                                    ctx_json = _serialize_session_ctx(
                                        local_config, agent_id_u64
                                    )
                                elif point_label == "post_tool_call":
                                    current_tool_call = (
                                        globals_mod.CURRENT_TOOL_CALLS.get(agent_id_u64)
                                        if globals_mod
                                        and hasattr(globals_mod, "CURRENT_TOOL_CALLS")
                                        else None
                                    )
                                    ctx_json = _serialize_post_tool_call_ctx(
                                        ctx, current_tool_call, agent_id_u64
                                    )
                                elif point_label == "on_tool_error":
                                    current_tool_call = (
                                        globals_mod.CURRENT_TOOL_CALLS.get(agent_id_u64)
                                        if globals_mod
                                        and hasattr(globals_mod, "CURRENT_TOOL_CALLS")
                                        else None
                                    )
                                    ctx_json = _serialize_on_tool_error_ctx(
                                        ctx, current_tool_call, hook_logger
                                    )
                                elif point_label == "stop":
                                    ctx_json = _serialize_stop_args(ctx)
                                elif ctx is None:
                                    ctx_json = "{}"
                                elif point_label == "post_turn":
                                    ctx_json = _serialize_post_turn_ctx(ctx)
                                elif point_label == "pre_turn":
                                    ctx_json = _serialize_pre_turn_ctx(ctx)
                                else:
                                    ctx_json = _serialize_generic_ctx(ctx)
                            except Exception as e:
                                hook_logger.error(
                                    "Failed to serialize hook context for %r: %s",
                                    name,
                                    e,
                                )
                                if point_label == "stop":
                                    from google.antigravity import types as sdk_types

                                    return sdk_types.StopHookResult()
                                if point_label in RESULT_HOOK_POINTS:
                                    return hooks_module.HookResult(
                                        allow=False,
                                        message=f"Failed to serialize hook context: {e}",
                                    )
                                ctx_json = "{}"

                            try:
                                res = globals_mod.dispatch_rust_hook(
                                    int(agent_id_u64), point_label, ctx_json
                                )
                                if inspect.isawaitable(res):
                                    result_json = await res
                                else:
                                    result_json = res
                            except Exception as e:
                                hook_logger.error(
                                    "dispatch_rust_hook failed for %r: %s", name, e
                                )
                                if point_label == "stop":
                                    from google.antigravity import types as sdk_types

                                    return sdk_types.StopHookResult()
                                if point_label in RESULT_HOOK_POINTS:
                                    return hooks_module.HookResult(
                                        allow=False, message=str(e)
                                    )
                                return

                            # stop is a TransformHook returning types.StopHookResult.
                            if point_label == "stop":
                                from google.antigravity import types as sdk_types

                                if not result_json:
                                    return sdk_types.StopHookResult()
                                try:
                                    res_dict = json.loads(result_json)
                                    dec_str = res_dict.get("decision", "ALLOW_STOP")
                                    decision = (
                                        sdk_types.StopDecision.CONTINUE
                                        if dec_str == "CONTINUE"
                                        else sdk_types.StopDecision.ALLOW_STOP
                                    )
                                    return sdk_types.StopHookResult(
                                        decision=decision,
                                        reason=res_dict.get("reason", "") or "",
                                    )
                                except Exception as e:
                                    hook_logger.error(
                                        "Failed to decode stop result JSON %r: %s",
                                        result_json,
                                        e,
                                    )
                                    return sdk_types.StopHookResult()

                            # on_tool_error is a TransformHook: the Rust side
                            # returns the model-facing error representation as a
                            # JSON value (a JSON string, or `null` to defer to
                            # the harness's default error formatting).
                            if point_label == "on_tool_error":
                                if not result_json:
                                    return None
                                try:
                                    return json.loads(result_json)
                                except json.JSONDecodeError:
                                    hook_logger.error(
                                        "Failed to decode on_tool_error result JSON %r",
                                        result_json,
                                    )
                                    return None

                            if result_json:
                                try:
                                    res_dict = json.loads(result_json)
                                    if "allow" in res_dict:
                                        return hooks_module.HookResult(
                                            allow=res_dict.get("allow", True),
                                            message=res_dict.get("message", ""),
                                        )
                                except json.JSONDecodeError as e:
                                    hook_logger.error(
                                        "Failed to decode hook result JSON %r: %s",
                                        result_json,
                                        e,
                                    )
                                    if point_label in RESULT_HOOK_POINTS:
                                        return hooks_module.HookResult(
                                            allow=False,
                                            message=f"Invalid hook result JSON: {e}",
                                        )

                            if point_label in RESULT_HOOK_POINTS:
                                return hooks_module.HookResult(allow=True, message="")

                        return _hook_callback

                    callback = _make_hook_cb(hook_name, sdk_point)

                    decorator = getattr(hooks_module, sdk_point, None)
                    if decorator is None:
                        hook_logger.warning(
                            "SDK does not support hook decorator for %r, skipping hook %r",
                            sdk_point,
                            hook_name,
                        )
                        continue

                    registered_hooks.append(decorator(callback))
                    hook_logger.info(
                        "Registered hook %r at point %s", hook_name, sdk_point
                    )
                except Exception as exc:
                    hook_logger.error(
                        "Failed to register hook %r: %s",
                        entry.get("name", "unnamed"),
                        exc,
                    )

            if registered_hooks:
                existing = local_config.get("hooks", [])
                if isinstance(existing, list):
                    local_config["hooks"] = existing + registered_hooks
                else:
                    local_config["hooks"] = registered_hooks
        except ImportError:
            hook_logger = logging.getLogger("agy_bridge.hooks")
            hook_logger.warning(
                "google.antigravity.hooks.hooks module not available, skipping hook registration"
            )


def _wire_triggers(local_config):
    """Register SDK triggers from serialized trigger specs; returns the list."""
    import logging

    # --- Wire triggers using SDK primitives ---
    trigger_entries = local_config.pop("triggers", [])
    sdk_triggers = []
    if trigger_entries:
        try:
            from google.antigravity.triggers import every, on_file_change

            trigger_logger = logging.getLogger("agy_bridge.triggers")

            def _make_trigger_cb(name, msg_tpl):
                """Factory to capture name/msg_tpl per trigger."""

                async def _trigger_callback(ctx, *_args):
                    trigger_logger.info("Trigger %r fired, sending: %s", name, msg_tpl)
                    try:
                        await ctx.send(msg_tpl)
                    except Exception as notify_exc:
                        trigger_logger.error(
                            "Failed to send notification for trigger %r: %s",
                            name,
                            notify_exc,
                        )

                return _trigger_callback

            for entry in trigger_entries:
                try:
                    trigger_name = entry.get("name", "unnamed")
                    config = entry.get("config", {})
                    message_template = entry.get("message_template", "")

                    callback = _make_trigger_cb(trigger_name, message_template)

                    if "Every" in config:
                        interval_secs = config["Every"].get("interval", 0)
                        sdk_triggers.append(every(interval_secs, callback))
                        trigger_logger.info(
                            "Registered every(%ds) trigger %r",
                            interval_secs,
                            trigger_name,
                        )
                    elif "OnFileChange" in config:
                        path = config["OnFileChange"].get("path", "")
                        sdk_triggers.append(on_file_change(path, callback))
                        trigger_logger.info(
                            "Registered on_file_change(%r) trigger %r",
                            path,
                            trigger_name,
                        )
                    else:
                        trigger_logger.warning(
                            "Unknown trigger config for %r: %s, skipping",
                            trigger_name,
                            config,
                        )
                except Exception as exc:
                    trigger_logger.error(
                        "Failed to register trigger %r: %s",
                        entry.get("name", "unnamed"),
                        exc,
                    )
        except ImportError:
            trigger_logger = logging.getLogger("agy_bridge.triggers")
            trigger_logger.error("Failed to import trigger modules: %s", exc)
    return sdk_triggers


def _wire_mcp_servers(local_config):
    """Convert serialized MCP server dicts into SDK pydantic types."""
    import logging

    # --- Wire MCP Servers ---
    # Rust serializes MCP servers as JSON dicts with a "type" discriminator.
    # Convert them to the correct SDK pydantic types.
    if "mcp_servers" in local_config and local_config["mcp_servers"]:
        try:
            import google.antigravity.types as agy_types

            parsed_mcp = []
            for mcp in local_config.pop("mcp_servers"):
                typ = mcp.pop("type", None)
                if "name" not in mcp or not mcp["name"]:
                    mcp["name"] = "mcp_server"
                if typ == "stdio":
                    parsed_mcp.append(agy_types.McpStdioServer(**mcp))
                elif typ in ("sse", "http"):
                    parsed_mcp.append(agy_types.McpStreamableHttpServer(**mcp))
                else:
                    logging.getLogger("agy_bridge.mcp").warning(
                        "Unknown MCP type %r", typ
                    )
            local_config["mcp_servers"] = parsed_mcp
        except Exception as exc:
            logging.getLogger("agy_bridge.mcp").error(
                "Failed to parse MCP configs: %s", exc
            )
    else:
        local_config.pop("mcp_servers", None)


def _setup_base_url_routing(local_config):
    """Wire per-instance base_url routing into LocalConnectionStrategy.

    Returns the custom base_url (or None) so the caller can bind it onto the
    agent lifecycle at the right time.
    """
    import logging

    logger = logging.getLogger("agy_bridge.init")

    # --- Handle custom base_url routing ---
    # When a custom base_url is set (e.g., local proxy, alternative gateway),
    # inject it into the harness config proto at connection time.
    #
    # IMPORTANT: The monkey-patch must be per-instance, not per-class.
    # Multiple agents in the same process may use different base_urls
    # (or no base_url at all). We use a registry keyed by strategy instance
    # id to look up the correct URL.
    custom_base_url = None
    if "gemini_config" in local_config and local_config["gemini_config"]:
        custom_base_url = local_config["gemini_config"].pop("base_url", None)
    if not custom_base_url:
        custom_base_url = os.environ.get("GEMINI_API_BASE_URL")

    if custom_base_url:
        # When routing through a proxy/gateway that handles auth (e.g. via
        # mTLS or bearer tokens), no API key is needed. Set a sentinel so the
        # SDK's API key validation passes.
        if "gemini_config" not in local_config or not local_config["gemini_config"]:
            local_config["gemini_config"] = {}
        if not local_config["gemini_config"].get("api_key"):
            # FRAGILE: this sentinel bypasses the SDK's API key validation
            # when routing through a proxy/gateway that handles auth. It is
            # cleared in _build_harness_config before the actual RPC. If the
            # SDK changes its API key validation, this will need updating.
            local_config["gemini_config"]["api_key"] = _PROXY_AUTH_SENTINEL

        try:
            from google.antigravity.connections.local.local_connection import (
                LocalConnectionStrategy,
            )
        except ImportError:
            logger.warning(
                "[BASE_URL] LocalConnectionStrategy not importable — "
                "base_url routing unavailable (SDK may have been restructured)"
            )
            return custom_base_url

        # ── Structural guard ──
        if not hasattr(LocalConnectionStrategy, "_build_harness_config"):
            logger.warning(
                "[BASE_URL] SDK structural drift: LocalConnectionStrategy "
                "no longer has _build_harness_config. base_url routing "
                "cannot be applied — the SDK may have been updated."
            )
            return custom_base_url

        import contextvars

        # Route the base_url to the connection strategy WITHOUT any global
        # handoff slot. A `contextvars.ContextVar` is isolated per asyncio task
        # and per thread, so concurrent agent creations — on the same event
        # loop or across separate bridges — never clobber each other's
        # base_url. The URL is copied onto the strategy instance in `__init__`
        # (which runs inside the creating agent's task during `__aenter__`) and
        # consumed in `_build_harness_config`. Because it lives on the instance,
        # it is freed with the strategy — there is no ever-growing registry.
        if not hasattr(LocalConnectionStrategy, "_agy_base_url_var"):
            LocalConnectionStrategy._agy_base_url_var = contextvars.ContextVar(
                "agy_base_url", default=None
            )
            _original_build = LocalConnectionStrategy._build_harness_config

            def _patched_build(self):
                config = _original_build(self)
                url = (
                    getattr(self, "_agy_base_url", None)
                    or LocalConnectionStrategy._agy_base_url_var.get()
                    or os.environ.get("GEMINI_API_BASE_URL")
                )
                if url:
                    try:
                        if config.HasField("gemini_config"):
                            config.gemini_config.base_url = url
                            logger.info("Injected base_url=%s into harness config", url)
                    except (AttributeError, ValueError):
                        pass
                    if hasattr(config, "models"):
                        for m in config.models:
                            if m.HasField("gemini_api_endpoint"):
                                m.gemini_api_endpoint.base_url = url
                                if (
                                    not m.gemini_api_endpoint.api_key
                                    or m.gemini_api_endpoint.api_key
                                    == _PROXY_AUTH_SENTINEL
                                ):
                                    m.gemini_api_endpoint.api_key = _PROXY_AUTH_SENTINEL
                                logger.info(
                                    "Injected base_url=%s into model %s",
                                    url,
                                    m.name,
                                )
                return config

            LocalConnectionStrategy._build_harness_config = _patched_build

            # Patch __init__ to bind the base_url from the current task/thread
            # context onto the new strategy instance.
            _original_strategy_init = LocalConnectionStrategy.__init__

            def _patched_strategy_init(self, *args, **kwargs):
                _original_strategy_init(self, *args, **kwargs)
                # ContextVar reads are task- and thread-local, so this is
                # race-free across concurrent agent creations.
                url = LocalConnectionStrategy._agy_base_url_var.get()
                self._agy_base_url = url
                if url and getattr(self, "_models", None):
                    for m in self._models:
                        if getattr(m, "endpoint", None) is not None:
                            m.endpoint.base_url = url

            LocalConnectionStrategy.__init__ = _patched_strategy_init

    return custom_base_url


def _build_agent_lifecycle(
    agent, agent_id_u64, initial_history, custom_base_url, passed_event_loop
):
    """Schedule the agent's async lifecycle and return `(controller, awaitable)`."""
    import asyncio
    import logging
    import sys

    # Store the base_url on the agent object so _agent_lifecycle can
    # set _pending_base_url at the right time (on the event loop thread,
    # right before __aenter__).
    if custom_base_url:
        agent._agy_pending_base_url = custom_base_url

    enter_event = asyncio.Event()
    exit_event = asyncio.Event()
    result_holder = {}

    async def _agent_lifecycle():
        try:
            # Bind this agent's base_url into the current task's context, on the
            # event loop thread, right before __aenter__. `_agent_lifecycle`
            # runs as its own asyncio task (scheduled via
            # run_coroutine_threadsafe), so the ContextVar value is isolated
            # from every other concurrent agent creation — there is no shared
            # slot that another init could overwrite, on this loop or any other.
            pending_url = getattr(agent, "_agy_pending_base_url", None)
            if pending_url is not None:
                from google.antigravity.connections.local.local_connection import (
                    LocalConnectionStrategy,
                )

                LocalConnectionStrategy._agy_base_url_var.set(pending_url)
                delattr(agent, "_agy_pending_base_url")

            _entered_agent = False
            for _attempt in range(1, 6):
                try:
                    await agent.__aenter__()
                    _entered_agent = True
                    break
                except BaseException as _enter_err:
                    _err_str = str(_enter_err)
                    _is_transient_win_err = os.name == "nt" and (
                        isinstance(_enter_err, PermissionError)
                        or "WinError 32" in _err_str
                        or "WinError 26" in _err_str
                        or "WinError 5" in _err_str
                        or "WinError 10055" in _err_str
                        or "WinError 10048" in _err_str
                    )
                    if _is_transient_win_err and _attempt < 5:
                        await asyncio.sleep(0.05 * _attempt)
                        continue
                    raise
            try:
                if hasattr(agent, "conversation") and hasattr(
                    agent.conversation, "connection"
                ):
                    conn = agent.conversation.connection
                    conn._agent_id = agent_id_u64
                    if hasattr(conn, "_processor"):
                        conn._processor._agent_id = agent_id_u64

                # Inject initial_history into the conversation's internal
                # _history list, enabling warm-start with prior context.
                if (
                    initial_history
                    and hasattr(agent, "conversation")
                    and agent.conversation is not None
                ):
                    conv = agent.conversation
                    if hasattr(conv, "_history"):
                        from google.genai import types as genai_types

                        history_logger = logging.getLogger("agy_bridge.initial_history")
                        for msg in initial_history:
                            role = msg.get("role", "user")
                            content_text = msg.get("content", "")
                            try:
                                entry = genai_types.Content(
                                    role=role,
                                    parts=[genai_types.Part(text=content_text)],
                                )
                                conv._history.append(entry)
                            except Exception as e:
                                history_logger.error(
                                    "Failed to inject initial_history entry (role=%s): %s",
                                    role,
                                    e,
                                )
                        history_logger.info(
                            "Injected %d initial_history entries into conversation",
                            len(initial_history),
                        )

                result_holder["instance"] = agent
                enter_event.set()
                await exit_event.wait()
            finally:
                if _entered_agent:
                    await agent.__aexit__(*sys.exc_info())
        except BaseException as e:
            result_holder["error"] = e
            if not enter_event.is_set():
                enter_event.set()

    loop = passed_event_loop
    if loop is None:
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            try:
                loop = asyncio.get_event_loop()
            except Exception:
                logging.getLogger("agy_bridge.init").debug(
                    "asyncio.get_event_loop() failed, falling back to _agy_bridge_globals.EVENT_LOOP",
                    exc_info=True,
                )
                globals_mod = sys.modules.get("_agy_bridge_globals")
                # Prefer the per-runtime map keyed by this thread's identity so
                # that, with multiple bridges in one process, we never resolve
                # another runtime's event loop. Fall back to the legacy single
                # EVENT_LOOP attribute for backward compatibility.
                import threading

                loop = None
                loops = (
                    getattr(globals_mod, "EVENT_LOOPS", None) if globals_mod else None
                )
                if loops is not None:
                    loop = loops.get(threading.get_ident())
                if (
                    loop is None
                    and globals_mod is not None
                    and hasattr(globals_mod, "EVENT_LOOP")
                ):
                    loop = globals_mod.EVENT_LOOP
                if loop is None:
                    raise RuntimeError("EVENT_LOOP not found in _agy_bridge_globals")
    future = asyncio.run_coroutine_threadsafe(_agent_lifecycle(), loop)

    class AgentLifecycleController:
        def __init__(self, future, exit_event):
            self.future = future
            self.exit_event = exit_event

        async def __aexit__(self, exc_type, exc_val, exc_tb):
            self.exit_event.set()
            try:
                await asyncio.wait_for(asyncio.wrap_future(self.future), timeout=3.0)
            except (asyncio.CancelledError, asyncio.TimeoutError):
                if not self.future.done():
                    self.future.cancel()

    async def _wait_for_enter():
        await enter_event.wait()
        if "error" in result_holder:
            raise result_holder["error"]
        return result_holder["instance"]

    controller = AgentLifecycleController(future, exit_event)
    return (controller, _wait_for_enter())


def init_agent(
    config_json: str, agent_id_u64: int, agent_cls: Any, passed_event_loop: Any
) -> tuple[Any, Any]:
    import logging, sys, json

    # Extract and consume the backend log level injected by Rust.
    # Default ("warn") matches upstream SDK behavior — only warnings and above.
    _pre_parsed = json.loads(config_json)
    _backend_log_level = _pre_parsed.pop("_backend_log_level", "warn")
    config_json = json.dumps(_pre_parsed)

    _LEVEL_MAP = {
        "error": logging.ERROR,
        "warn": logging.WARNING,
        "info": logging.INFO,
        "debug": logging.DEBUG,
    }
    py_level = _LEVEL_MAP.get(_backend_log_level, logging.WARNING)
    logging.basicConfig(level=py_level, stream=sys.stderr)
    logger = logging.getLogger("agy_bridge.init")

    _apply_sdk_monkeypatches(logger)
    _patch_websockets_max_size(logger)

    if "_agy_bridge_globals" not in sys.modules:
        import types

        sys.modules["_agy_bridge_globals"] = types.ModuleType("_agy_bridge_globals")

    local_config: dict[str, Any] = json.loads(config_json)

    _wire_tool_proxies(local_config, agent_id_u64)
    _wire_capabilities(local_config)
    _wire_subagents(local_config)
    _wire_budget_config(local_config)
    _wire_compaction_config(local_config)
    _wire_retry_config(local_config)
    _wire_system_instructions(local_config)
    _wire_policies(local_config, agent_id_u64)
    _wire_hooks(local_config, agent_id_u64)
    sdk_triggers = _wire_triggers(local_config)
    _wire_mcp_servers(local_config)

    # Rust serializes `skills` as `"skills_paths"` and `gemini` as
    # `"gemini_config"` via serde(rename), so no Python-side renaming needed.
    # Strip null gemini_config so the SDK uses its defaults.
    if "gemini_config" in local_config and local_config["gemini_config"] is None:
        local_config.pop("gemini_config")

    if "session_continuation_mode" in local_config:
        scm = local_config["session_continuation_mode"]
        if scm is None:
            local_config.pop("session_continuation_mode")
        elif isinstance(scm, str):
            from google.antigravity.types import SessionContinuationMode

            try:
                local_config["session_continuation_mode"] = SessionContinuationMode(scm)
            except ValueError:
                pass

    custom_base_url = _setup_base_url_routing(local_config)

    _munge_config_model(local_config, custom_base_url=custom_base_url)

    initial_history = _extract_initial_history(local_config)

    from google.antigravity.connections.local.local_connection_config import (
        LocalAgentConfig,
    )

    # `conversation_id` is forwarded to the SDK as the harness `cascade_id`
    # (resume key): a non-empty value tells the local harness to reload that
    # trajectory from `save_dir`. It MUST reference a conversation the harness
    # previously persisted; an unknown id fails startup ("conversation not
    # found"). Leave it unset to start a fresh conversation — the harness then
    # assigns an id, which the bridge captures (see the `_cascade_id`
    # monkeypatch) and exposes via `AgentHandle::conversation_id`.
    config = LocalAgentConfig(triggers=sdk_triggers, **local_config)
    agent = agent_cls(config)

    return _build_agent_lifecycle(
        agent, agent_id_u64, initial_history, custom_base_url, passed_event_loop
    )
