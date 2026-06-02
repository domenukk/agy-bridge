"""Bootstrap an Antigravity SDK agent from a Rust-serialised JSON config.

This module is loaded once by the PyO3 runtime.  Every symbol except
``init_agent`` is an implementation detail and prefixed with ``_``.
"""

import json
import logging
import os
import re
import sys

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

_GLOBALS_MODULE_NAME = "_agy_bridge_globals"
_DEFAULT_SESSION_ID = "default_session"

_RATE_LIMIT_MARKERS = ("HTTP 429", "HTTP 503")
_RATE_LIMIT_QUOTA_KEYWORD = "quota"

# Hook points that *must* return a ``HookResult`` (even on error).
_DECISION_HOOK_POINTS = frozenset(
    {
        "pre_turn",
        "pre_tool_call_decide",
        "on_interaction",
    }
)

_SESSION_HOOK_POINTS = frozenset(
    {
        "on_session_start",
        "on_session_end",
    }
)

_CONFIRM_YES_VALUES = ("", "y", "yes")

# MCP type discriminator → SDK class name.
_MCP_TYPE_MAP = {
    "stdio": "McpStdioServer",
    "sse": "McpSseServer",
    "http": "McpStreamableHttpServer",
}

# ---------------------------------------------------------------------------
# Module-level loggers
# ---------------------------------------------------------------------------

logging.basicConfig(level=logging.INFO, stream=sys.stderr)

_LOG_POLICY = logging.getLogger("agy_bridge.policies")
_LOG_HOOK = logging.getLogger("agy_bridge.hooks")
_LOG_TRIGGER = logging.getLogger("agy_bridge.triggers")
_LOG_MCP = logging.getLogger("agy_bridge.mcp")


# ---------------------------------------------------------------------------
# Globals-module helpers
# ---------------------------------------------------------------------------


def _ensure_globals_module():
    """Create the ``_agy_bridge_globals`` synthetic module if absent."""
    if _GLOBALS_MODULE_NAME not in sys.modules:
        import types

        sys.modules[_GLOBALS_MODULE_NAME] = types.ModuleType(_GLOBALS_MODULE_NAME)


def _get_globals_module():
    """Return the globals module, or ``None``."""
    return sys.modules.get(_GLOBALS_MODULE_NAME)


def _require_globals_attr(attr):
    """Return the globals module if it exposes *attr*, else ``None``."""
    gm = _get_globals_module()
    if gm and hasattr(gm, attr):
        return gm
    return None


async def _await_if_needed(result):
    """``await`` *result* if it is awaitable, otherwise return it directly.

    PyO3's ``future_into_py`` returns an awaitable that is **not** detected by
    ``asyncio.iscoroutinefunction``, so we must probe with ``inspect``.
    """
    import inspect

    if inspect.isawaitable(result):
        return await result
    return result


# ---------------------------------------------------------------------------
# Rate-limit interceptor
# ---------------------------------------------------------------------------


class _RateLimitInterceptor(logging.Handler):
    """Logging handler that flags rate-limit responses in the globals module."""

    def emit(self, record):
        msg = self.format(record)
        is_rate_limited = (
            any(marker in msg for marker in _RATE_LIMIT_MARKERS)
            or _RATE_LIMIT_QUOTA_KEYWORD in msg.lower()
        )
        if is_rate_limited:
            gm = _get_globals_module()
            if gm:
                gm.RATE_LIMIT_HIT = True


# ---------------------------------------------------------------------------
# Async Rust proxy for tool dispatch
# ---------------------------------------------------------------------------


def _make_async_rust_proxy_class():
    """Build the ``AsyncRustProxy`` class (import-time one-shot).

    We defer the import of ``tool_runner`` to avoid hard failures when the
    SDK is not installed (e.g. during unit tests of the bridge itself).
    """
    from google.antigravity.tools import tool_runner

    class AsyncRustProxy(tool_runner.ToolWithSchema):
        """Proxy that forwards SDK tool calls to the Rust runtime."""

        def __init__(self, bridge_ctx, name, description, schema_dict):
            self.bridge_ctx = bridge_ctx
            self.__name__ = name
            self.__doc__ = description
            self.input_schema = schema_dict
            self.fn = self.__call__

        async def __call__(self, **kwargs):
            gm = _require_globals_attr("dispatch_rust_tool")
            if gm is None:
                raise RuntimeError(
                    "dispatch_rust_tool not found in _agy_bridge_globals"
                )
            args_json = json.dumps(kwargs)
            res = gm.dispatch_rust_tool(self.bridge_ctx, self.__name__, args_json)
            return await _await_if_needed(res)

    return AsyncRustProxy


# Lazily initialised on first use by ``_build_tool_proxies``.
_AsyncRustProxy = None


def _build_tool_proxies(tool_defs, bridge_ctx):
    """Convert raw tool dicts into ``AsyncRustProxy`` instances.

    Non-dict entries (pre-built tools) are passed through unchanged.
    """
    global _AsyncRustProxy
    if _AsyncRustProxy is None:
        _AsyncRustProxy = _make_async_rust_proxy_class()

    proxies = []
    for tool_def in tool_defs:
        if isinstance(tool_def, dict) and "name" in tool_def:
            proxies.append(
                _AsyncRustProxy(
                    bridge_ctx,
                    tool_def["name"],
                    tool_def.get("description", ""),
                    tool_def.get("parameter_schema", {}),
                )
            )
        else:
            proxies.append(tool_def)
    return proxies


# ---------------------------------------------------------------------------
# Tool resolution
# ---------------------------------------------------------------------------


def _resolve_tools(config, bridge_ctx):
    """Replace raw tool dicts in *config* with ``AsyncRustProxy`` objects."""
    if "tools" in config:
        config["tools"] = _build_tool_proxies(config["tools"], bridge_ctx)

    capabilities = config.get("capabilities")
    if (
        capabilities
        and isinstance(capabilities, dict)
        and capabilities.get("enabled_tools") is not None
    ):
        capabilities["enabled_tools"] = _build_tool_proxies(
            capabilities.get("enabled_tools") or [], bridge_ctx
        )

    if config.get("capabilities") is None:
        config.pop("capabilities", None)


# ---------------------------------------------------------------------------
# Policy parsing
# ---------------------------------------------------------------------------


def _make_rust_confirm_handler(bridge_ctx):
    """Return an async handler that delegates policy confirmation to Rust."""

    async def _handler(tc):
        gm = _require_globals_attr("dispatch_rust_policy_confirm")
        if gm is None:
            _LOG_POLICY.warning(
                "dispatch_rust_policy_confirm not found, "
                "falling back to console input"
            )
            print(
                f"\nAgent requested to run tool '{tc.name}' "
                f"with args {dict(tc.args)}"
            )
            return input("Allow? [Y/n]: ").strip().lower() in _CONFIRM_YES_VALUES

        tc_args_json = json.dumps(dict(tc.args))
        res = gm.dispatch_rust_policy_confirm(bridge_ctx, tc.name, tc_args_json)
        return await _await_if_needed(res)

    return _handler


def _parse_single_policy(raw_policy, bridge_ctx, policy_mod):
    """Parse one raw policy entry → SDK policy object, or ``None``."""
    if isinstance(raw_policy, str):
        if raw_policy == "AllowAll":
            return policy_mod.allow_all()
        if raw_policy == "DenyAll":
            return policy_mod.deny_all()
        _LOG_POLICY.warning("Unknown string policy %r, skipping", raw_policy)
        return None

    if not isinstance(raw_policy, dict):
        _LOG_POLICY.warning("Unknown policy type %r, skipping", raw_policy)
        return None

    if "Allow" in raw_policy:
        return policy_mod.allow(raw_policy["Allow"])
    if "Deny" in raw_policy:
        return policy_mod.deny(raw_policy["Deny"])
    if "AskUser" in raw_policy:
        handler = _make_rust_confirm_handler(bridge_ctx)
        return policy_mod.ask_user(raw_policy["AskUser"]["tool"], handler=handler)
    if "WorkspaceOnly" in raw_policy:
        # Handled by SDK via LocalAgentConfig.workspaces field.
        return None

    _LOG_POLICY.warning("Unknown policy type %r, skipping", raw_policy)
    return None


def _resolve_policies(config, bridge_ctx):
    """Replace raw policy dicts/strings with SDK policy objects."""
    if "policies" not in config:
        return

    from google.antigravity.hooks import policy as policy_mod

    parsed = []
    for raw in config["policies"]:
        result = _parse_single_policy(raw, bridge_ctx, policy_mod)
        if result is not None:
            parsed.append(result)
    config["policies"] = parsed


# ---------------------------------------------------------------------------
# Hook context serialisation helpers
# ---------------------------------------------------------------------------


def _dump_model(obj):
    """Serialise a pydantic-like object to a dict."""
    if hasattr(obj, "model_dump"):
        return obj.model_dump()
    if hasattr(obj, "dict"):
        return obj.dict()
    return obj


def _extract_tool_name(obj):
    """Extract a plain string tool name from various SDK representations."""
    name = getattr(obj, "name", "")
    if hasattr(name, "value"):
        return name.value
    return name if isinstance(name, str) else str(name)


def _result_to_str(result_val):
    """Convert a tool result value to a JSON-friendly string."""
    if result_val is None:
        return ""
    if isinstance(result_val, str):
        return result_val
    try:
        if hasattr(result_val, "model_dump_json"):
            return result_val.model_dump_json()
        if hasattr(result_val, "model_dump"):
            return json.dumps(result_val.model_dump())
        return json.dumps(result_val)
    except Exception:
        return str(result_val)


def _get_stashed_tool_call():
    """Retrieve the current tool call stashed by ``pre_tool_call_decide``."""
    gm = _get_globals_module()
    return getattr(gm, "CURRENT_TOOL_CALL", None) if gm else None


def _serialize_post_tool_call(ctx):
    """Build JSON payload for the ``post_tool_call`` hook."""
    current_tc = _get_stashed_tool_call()
    tool_args = _dump_model(getattr(current_tc, "args", {}) if current_tc else {})
    return json.dumps(
        {
            "name": _extract_tool_name(ctx),
            "args": tool_args,
            "result": _result_to_str(getattr(ctx, "result", None)),
        }
    )


def _serialize_on_tool_error(ctx):
    """Build JSON payload for the ``on_tool_error`` hook."""
    current_tc = _get_stashed_tool_call()
    tool_name = _extract_tool_name(current_tc) if current_tc else ""
    tool_args = _dump_model(getattr(current_tc, "args", {}) if current_tc else {})
    return json.dumps(
        {
            "tool_name": tool_name,
            "tool_args": tool_args,
            "error": str(ctx),
        }
    )


def _derive_conversation_id(config):
    """Try to derive a conversation ID from workspace paths."""
    workspaces = config.get("workspaces")
    if workspaces and isinstance(workspaces, list) and workspaces[0]:
        return os.path.basename(str(workspaces[0]).rstrip("/"))
    return None


def _serialize_session_hook(local_config, bridge_ctx):
    """Build JSON payload for ``on_session_start`` / ``on_session_end``."""
    conversation_id = (
        local_config.get("conversation_id")
        or _derive_conversation_id(local_config)
        or _DEFAULT_SESSION_ID
    )
    return json.dumps(
        {
            "session": {
                "session_id": str(conversation_id),
                "agent_id": bridge_ctx.agent_id,
            }
        }
    )


def _serialize_hook_context(point_label, ctx, local_config, bridge_ctx):
    """Map an SDK hook context to a JSON string for the Rust hook handler."""
    if point_label == "post_tool_call":
        return _serialize_post_tool_call(ctx)
    if point_label == "on_tool_error":
        return _serialize_on_tool_error(ctx)

    if ctx is None:
        if point_label in _SESSION_HOOK_POINTS:
            return _serialize_session_hook(local_config, bridge_ctx)
        return "{}"

    if point_label == "post_turn":
        return json.dumps(
            {
                "response_text": getattr(ctx, "text", str(ctx)),
                "turn_number": getattr(ctx, "turn_number", 0),
            }
        )
    if point_label == "pre_turn":
        text_val = ctx if isinstance(ctx, str) else str(ctx)
        return json.dumps(
            {
                "prompt": text_val,
                "turn_number": getattr(ctx, "turn_number", 0),
            }
        )

    if isinstance(ctx, str):
        return json.dumps({"value": ctx})
    if hasattr(ctx, "model_dump_json"):
        return ctx.model_dump_json()
    if isinstance(ctx, dict):
        return json.dumps(ctx)
    return json.dumps(str(ctx))


# ---------------------------------------------------------------------------
# Hook callback factory & registration
# ---------------------------------------------------------------------------


def _parse_hook_result(result_json, point_label, hooks_module):
    """Convert a Rust JSON response into an SDK ``HookResult`` (or ``None``)."""
    if result_json:
        try:
            res_dict = json.loads(result_json)
            if "allow" in res_dict:
                return hooks_module.HookResult(
                    allow=res_dict.get("allow", True),
                    message=res_dict.get("message", ""),
                )
        except json.JSONDecodeError:
            pass

    if point_label in _DECISION_HOOK_POINTS:
        return hooks_module.HookResult(allow=True, message="")
    return None


def _make_hook_callback(name, point_label, bridge_ctx, local_config, hooks_module):
    """Create a single async hook callback bound to *name* / *point_label*."""

    async def _hook_callback(ctx=None):
        gm = _require_globals_attr("dispatch_rust_hook")
        if gm is None:
            _LOG_HOOK.warning("dispatch_rust_hook not found, skipping hook %r", name)
            return None

        if point_label == "pre_tool_call_decide":
            gm.CURRENT_TOOL_CALL = ctx

        try:
            ctx_json = _serialize_hook_context(
                point_label, ctx, local_config, bridge_ctx
            )
        except Exception as exc:
            _LOG_HOOK.error("Failed to serialize hook context for %r: %s", name, exc)
            ctx_json = "{}"

        try:
            res = gm.dispatch_rust_hook(bridge_ctx, point_label, ctx_json)
            result_json = await _await_if_needed(res)
        except Exception as exc:
            _LOG_HOOK.error("dispatch_rust_hook failed for %r: %s", name, exc)
            if point_label in _DECISION_HOOK_POINTS:
                return hooks_module.HookResult(allow=True, message=str(exc))
            return None

        return _parse_hook_result(result_json, point_label, hooks_module)

    return _hook_callback


def _camel_to_snake(name):
    """Translate ``CamelCase`` to ``snake_case`` (e.g. PreTurn → pre_turn)."""
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def _register_hooks(config, bridge_ctx):
    """Parse hook entries from *config* and register them with the SDK."""
    hooks_entries = config.pop("hooks", [])
    if not hooks_entries:
        return

    try:
        from google.antigravity.hooks import hooks as hooks_module

        registered = []
        for entry in hooks_entries:
            hook_name = entry.get("name", "unnamed")
            hook_point = entry.get("point", "")
            sdk_point = _camel_to_snake(hook_point)

            decorator = getattr(hooks_module, sdk_point, None)
            if decorator is None:
                _LOG_HOOK.warning(
                    "Unsupported hook point %r (%r) for hook %r, skipping",
                    hook_point,
                    sdk_point,
                    hook_name,
                )
                continue

            callback = _make_hook_callback(
                hook_name, sdk_point, bridge_ctx, config, hooks_module
            )
            registered.append(decorator(callback))
            _LOG_HOOK.info("Registered hook %r at point %s", hook_name, sdk_point)

        if registered:
            existing = config.get("hooks", [])
            config["hooks"] = (
                existing + registered if isinstance(existing, list) else registered
            )
    except ImportError:
        _LOG_HOOK.warning(
            "google.antigravity.hooks.hooks not available, "
            "skipping hook registration"
        )
    except Exception as exc:
        _LOG_HOOK.error("Failed to register hooks: %s", exc)


# ---------------------------------------------------------------------------
# Trigger wiring
# ---------------------------------------------------------------------------


def _make_trigger_callback(name, message_template):
    """Return an async callback that sends *message_template* on trigger."""

    async def _trigger_callback(ctx, *_args):
        _LOG_TRIGGER.info("Trigger %r fired, sending: %s", name, message_template)
        try:
            await ctx.send(message_template)
        except Exception as exc:
            _LOG_TRIGGER.error(
                "Failed to send notification for trigger %r: %s", name, exc
            )

    return _trigger_callback


def _register_triggers(config):
    """Parse trigger entries from *config* and return SDK trigger objects."""
    trigger_entries = config.pop("triggers", [])
    sdk_triggers = []
    if not trigger_entries:
        return sdk_triggers

    try:
        from google.antigravity.triggers import every, on_file_change

        for entry in trigger_entries:
            trigger_name = entry.get("name", "unnamed")
            trigger_config = entry.get("config", {})
            callback = _make_trigger_callback(
                trigger_name, entry.get("message_template", "")
            )

            if "Every" in trigger_config:
                interval_secs = trigger_config["Every"].get("interval", 0)
                sdk_triggers.append(every(interval_secs, callback))
                _LOG_TRIGGER.info(
                    "Registered every(%ds) trigger %r", interval_secs, trigger_name
                )
            elif "OnFileChange" in trigger_config:
                path = trigger_config["OnFileChange"].get("path", "")
                sdk_triggers.append(on_file_change(path, callback))
                _LOG_TRIGGER.info(
                    "Registered on_file_change(%r) trigger %r", path, trigger_name
                )
            else:
                _LOG_TRIGGER.warning(
                    "Unknown trigger config for %r: %s, skipping",
                    trigger_name,
                    trigger_config,
                )
    except Exception as exc:
        _LOG_TRIGGER.error("Failed to register triggers: %s", exc)

    return sdk_triggers


# ---------------------------------------------------------------------------
# MCP server parsing
# ---------------------------------------------------------------------------


def _resolve_mcp_servers(config):
    """Convert raw MCP server dicts into SDK pydantic types."""
    raw_servers = config.pop("mcp_servers", None)
    if not raw_servers:
        return

    try:
        import google.antigravity.types as agy_types

        parsed = []
        for mcp in raw_servers:
            typ = mcp.pop("type", None)
            cls_name = _MCP_TYPE_MAP.get(typ)
            if cls_name is None:
                _LOG_MCP.warning("Unknown MCP type %r, skipping", typ)
                continue
            parsed.append(getattr(agy_types, cls_name)(**mcp))
        config["mcp_servers"] = parsed
    except Exception as exc:
        _LOG_MCP.error("Failed to parse MCP configs: %s", exc)


# ---------------------------------------------------------------------------
# Agent lifecycle management
# ---------------------------------------------------------------------------


class _AgentLifecycleController:
    """Manages the async lifecycle (enter / exit) of the SDK agent."""

    def __init__(self, future, exit_event):
        self.future = future
        self.exit_event = exit_event

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        import asyncio

        self.exit_event.set()
        try:
            await asyncio.wrap_future(self.future)
        except asyncio.CancelledError:
            pass


def _start_agent_lifecycle(agent, loop):
    """Start the agent's async context manager on *loop*.

    Returns ``(controller, awaitable)`` where the awaitable resolves to the
    live agent instance once the context manager has been entered.
    """
    import asyncio

    enter_event = asyncio.Event()
    exit_event = asyncio.Event()
    result_holder = {}

    async def _run():
        try:
            async with agent:
                result_holder["instance"] = agent
                enter_event.set()
                await exit_event.wait()
        except Exception as exc:
            result_holder["error"] = exc
            if not enter_event.is_set():
                enter_event.set()

    future = asyncio.run_coroutine_threadsafe(_run(), loop)

    async def _wait_for_enter():
        await enter_event.wait()
        if "error" in result_holder:
            raise result_holder["error"]
        return result_holder["instance"]

    return _AgentLifecycleController(future, exit_event), _wait_for_enter()


# ---------------------------------------------------------------------------
# Top-level entry point
# ---------------------------------------------------------------------------


def init_agent(config_json, agent_id_u64, agent_cls, bridge_ctx, event_loop):
    """Initialise and start an SDK agent from a Rust-serialised JSON config.

    Returns ``(controller, awaitable)`` — the awaitable resolves to the live
    agent instance once its async context has been entered.
    """
    # Bootstrap.
    logging.getLogger().addHandler(_RateLimitInterceptor())
    _ensure_globals_module()

    config = json.loads(config_json)

    # Resolve tools → AsyncRustProxy objects.
    _resolve_tools(config, bridge_ctx)

    # Parse policy definitions.
    _resolve_policies(config, bridge_ctx)

    # Wire hooks.
    _register_hooks(config, bridge_ctx)

    # Wire triggers.
    sdk_triggers = _register_triggers(config)

    # Parse MCP server configs.
    _resolve_mcp_servers(config)

    # Rust serializes `skills` as `"skills_paths"` and `gemini` as
    # `"gemini_config"` via serde(rename); strip null so the SDK defaults.
    if config.get("gemini_config") is None:
        config.pop("gemini_config", None)

    # Build the SDK config and agent.
    from google.antigravity.connections.local.local_connection_config import (
        LocalAgentConfig,
    )

    agent = agent_cls(LocalAgentConfig(triggers=sdk_triggers, **config))

    return _start_agent_lifecycle(agent, event_loop)
