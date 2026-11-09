# Section wiring, base_url routing, and lifecycle orchestration for Python SDK agents.

from __future__ import annotations

import logging
import os
from typing import Any

_INIT_LOGGER = logging.getLogger("agy_bridge.init")
_RECOVERABLE_ERRORS = (
    ImportError,
    RuntimeError,
    TypeError,
    ValueError,
    KeyError,
    AttributeError,
    OSError,
)

try:
    from agent_init_helpers import (
        _PROXY_AUTH_SENTINEL,
        _WS_MAXSIZE_DEFAULT_CAP,
        _WS_MAXSIZE_PATCH_SDK_VERSION,
        RESULT_HOOK_POINTS,
        _apply_sdk_monkeypatches,
        _build_model_target,
        _extract_initial_history,
        _get_monkeypatch_lock,
        _munge_config_model,
        _normalize_tool_name,
        _patch_subagent_inline_skills,
        _patch_websockets_max_size,
        _resolve_installed_sdk_version,
        _resolve_ws_max_size,
        _seed_conversation_history,
        _serialize_generic_ctx,
        _serialize_on_tool_error_ctx,
        _serialize_post_tool_call_ctx,
        _serialize_post_turn_ctx,
        _serialize_pre_turn_ctx,
        _serialize_session_ctx,
        _serialize_stop_args,
        _subagent_skills_proto_with_inline,
        _to_dict,
    )
except ImportError as _imp_err:
    _INIT_LOGGER.debug("agent_init_helpers not pre-imported: %s", _imp_err)

try:
    from sdk_proto_parity import (
        ATTR_DISABLE_DEFINE_SUBAGENT_TOOL,
        FIELD_DISABLE_DEFINE_SUBAGENT_TOOL,
        PROTO_SUBAGENTS_CONFIG,
        _coerce_builtin_tool,
        _coerce_system_instructions,
        _parse_inline_skill,
        _patch_compaction_config_proto,
        _patch_harness_side_tools_proto,
        _require_sdk_proto_field,
        _unwrap_stdio_env_wrapper,
        _validate_mcp_server_parity,
        _wire_budget_config,
        _wire_compaction_config,
        _wire_retry_config,
        _wire_subagents,
        _wire_tool_proxies,
    )
except ImportError as _imp_err:
    _INIT_LOGGER.debug("sdk_proto_parity not pre-imported: %s", _imp_err)

__all__ = [
    "_WS_MAXSIZE_DEFAULT_CAP",
    "_WS_MAXSIZE_PATCH_SDK_VERSION",
    "_build_model_target",
    "_get_monkeypatch_lock",
    "_normalize_tool_name",
    "_patch_compaction_config_proto",
    "_patch_harness_side_tools_proto",
    "_patch_subagent_inline_skills",
    "_resolve_installed_sdk_version",
    "_resolve_ws_max_size",
    "_seed_conversation_history",
    "_subagent_skills_proto_with_inline",
    "_to_dict",
    "_wire_budget_config",
    "_wire_compaction_config",
    "_wire_retry_config",
    "_wire_subagents",
    "init_agent",
]


def _normalize_capabilities(local_config: dict[str, Any]) -> None:
    """Normalize capabilities fields (agent_behavior, allowed_subagents, run_command_config, tool_output_truncation_config, builtin tools)."""
    if local_config.get("capabilities"):
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
                except _RECOVERABLE_ERRORS as exc:
                    _INIT_LOGGER.debug("agent_behavior coercion skipped: %s", exc)
            if "run_command_config" in caps and isinstance(
                caps["run_command_config"], dict
            ):
                try:
                    from google.antigravity.types import RunCommandConfig

                    caps["run_command_config"] = RunCommandConfig(
                        **caps["run_command_config"]
                    )
                except _RECOVERABLE_ERRORS as exc:
                    _INIT_LOGGER.debug("run_command_config coercion skipped: %s", exc)
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

                    trunc_dict = dict(caps["tool_output_truncation_config"])
                    if hasattr(ToolOutputTruncationConfig, "model_fields"):
                        allowed_keys = set(
                            ToolOutputTruncationConfig.model_fields.keys()
                        )
                        trunc_dict = {
                            k: v for k, v in trunc_dict.items() if k in allowed_keys
                        }
                    caps["tool_output_truncation_config"] = ToolOutputTruncationConfig(
                        **trunc_dict
                    )
                except _RECOVERABLE_ERRORS as exc:
                    _INIT_LOGGER.debug(
                        "tool_output_truncation_config coercion skipped: %s", exc
                    )
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
    caps_raw = local_config.get("capabilities")
    disable_define_subagent = isinstance(caps_raw, dict) and bool(
        caps_raw.get(FIELD_DISABLE_DEFINE_SUBAGENT_TOOL, False)
    )
    if disable_define_subagent:
        _require_sdk_proto_field(
            PROTO_SUBAGENTS_CONFIG,
            FIELD_DISABLE_DEFINE_SUBAGENT_TOOL,
            "CapabilitiesConfig.disable_define_subagent_tool",
        )
    if "capabilities" in local_config and isinstance(
        local_config["capabilities"], dict
    ):
        try:
            from google.antigravity.types import CapabilitiesConfig, RunCommandConfig

            if hasattr(CapabilitiesConfig, "model_rebuild"):
                CapabilitiesConfig.model_rebuild()
            caps_dict = dict(local_config["capabilities"])
            cmd_timeout_ms = caps_dict.pop("command_timeout_ms", None)
            if (
                cmd_timeout_ms is not None
                and caps_dict.get("run_command_config") is None
            ):
                caps_dict["run_command_config"] = RunCommandConfig(
                    timeout_seconds=float(cmd_timeout_ms) / 1000.0
                )
            caps_dict.pop("image_model", None)
            if hasattr(CapabilitiesConfig, "model_fields"):
                allowed_keys = set(CapabilitiesConfig.model_fields.keys())
                caps_dict = {k: v for k, v in caps_dict.items() if k in allowed_keys}
            sdk_caps = CapabilitiesConfig(**caps_dict)
            if disable_define_subagent:
                object.__setattr__(sdk_caps, ATTR_DISABLE_DEFINE_SUBAGENT_TOOL, True)
            local_config["capabilities"] = sdk_caps
        except _RECOVERABLE_ERRORS as exc:
            _INIT_LOGGER.debug("CapabilitiesConfig coercion skipped: %s", exc)


def _wire_inline_skills(local_config: dict[str, Any]) -> None:
    """Translate serialized top-level inline_skills into SDK InlineSkill objects."""
    if "inline_skills" not in local_config:
        return
    raw_skills = local_config["inline_skills"]
    if not raw_skills:
        local_config.pop("inline_skills", None)
        return
    if isinstance(raw_skills, list):
        local_config["inline_skills"] = [_parse_inline_skill(sk) for sk in raw_skills]


def _wire_system_instructions(local_config: dict[str, Any]) -> None:
    """Translate serialized system_instructions into SDK Custom/TemplatedSystemInstructions."""
    if "system_instructions" not in local_config:
        return
    si = local_config["system_instructions"]
    if si is None:
        local_config.pop("system_instructions", None)
        return
    try:
        local_config["system_instructions"] = _coerce_system_instructions(si)
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
                    import asyncio
                    import inspect
                    import json
                    import sys

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
                            import inspect
                            import json
                            import sys

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

                            if point_label == "pre_tool_call_decide" and globals_mod:
                                if not hasattr(globals_mod, "CURRENT_TOOL_CALLS"):
                                    globals_mod.CURRENT_TOOL_CALLS = {}
                                globals_mod.CURRENT_TOOL_CALLS[agent_id_u64] = ctx

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
                            except _RECOVERABLE_ERRORS as e:
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
                            except _RECOVERABLE_ERRORS as e:
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
                                except _RECOVERABLE_ERRORS as e:
                                    hook_logger.error(
                                        "Failed to decode stop result JSON %r: %s",
                                        result_json,
                                        e,
                                    )
                                    return sdk_types.StopHookResult()

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
                except _RECOVERABLE_ERRORS as exc:
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
                    except _RECOVERABLE_ERRORS as notify_exc:
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
                except _RECOVERABLE_ERRORS as exc:
                    trigger_logger.error(
                        "Failed to register trigger %r: %s",
                        entry.get("name", "unnamed"),
                        exc,
                    )
        except ImportError as exc:
            trigger_logger = logging.getLogger("agy_bridge.triggers")
            trigger_logger.error("Failed to import trigger modules: %s", exc)
    return sdk_triggers


def _wire_mcp_servers(local_config):
    """Convert serialized MCP server dicts into SDK pydantic types."""
    import logging

    raw_servers = local_config.pop("mcp_servers", None)
    if not raw_servers:
        return
    for mcp in raw_servers:
        _validate_mcp_server_parity(mcp)
    try:
        import google.antigravity.types as agy_types

        parsed_mcp = []
        for mcp in raw_servers:
            typ = mcp.pop("type", None)
            if "name" not in mcp or not mcp["name"]:
                mcp["name"] = "mcp_server"
            if typ == "stdio":
                _unwrap_stdio_env_wrapper(mcp)
                parsed_mcp.append(agy_types.McpStdioServer(**mcp))
            elif typ in ("sse", "http"):
                parsed_mcp.append(agy_types.McpStreamableHttpServer(**mcp))
            else:
                logging.getLogger("agy_bridge.mcp").warning("Unknown MCP type %r", typ)
        local_config["mcp_servers"] = parsed_mcp
    except _RECOVERABLE_ERRORS as exc:
        logging.getLogger("agy_bridge.mcp").error(
            "Failed to parse MCP configs: %s", exc
        )


def _setup_base_url_routing(local_config):
    """Wire per-instance base_url routing into LocalConnectionStrategy."""
    import logging

    logger = logging.getLogger("agy_bridge.init")

    custom_base_url = None
    if local_config.get("gemini_config"):
        custom_base_url = local_config["gemini_config"].pop("base_url", None)
    if not custom_base_url:
        custom_base_url = os.environ.get("GEMINI_API_BASE_URL")

    try:
        from google.antigravity.connections.local import local_connection as _lc_mod
        from google.antigravity.connections.local.local_connection import (
            LocalConnectionStrategy,
        )

        _patch_subagent_inline_skills(LocalConnectionStrategy, logger)
        _patch_harness_side_tools_proto(LocalConnectionStrategy, logger)
        _patch_compaction_config_proto(_lc_mod, logger)
    except ImportError:
        LocalConnectionStrategy = None  # type: ignore[assignment,misc]

    if custom_base_url:
        if "gemini_config" not in local_config or not local_config["gemini_config"]:
            local_config["gemini_config"] = {}
        if not local_config["gemini_config"].get("api_key"):
            local_config["gemini_config"]["api_key"] = _PROXY_AUTH_SENTINEL

        if LocalConnectionStrategy is None or not hasattr(
            LocalConnectionStrategy, "_build_harness_config"
        ):
            logger.warning(
                "[BASE_URL] LocalConnectionStrategy missing _build_harness_config; base_url routing skipped"
            )
            return custom_base_url

        import contextvars

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

            _original_strategy_init = LocalConnectionStrategy.__init__

            def _patched_strategy_init(self, *args, **kwargs):
                _original_strategy_init(self, *args, **kwargs)
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

    if custom_base_url:
        agent._agy_pending_base_url = custom_base_url

    enter_event = asyncio.Event()
    exit_event = asyncio.Event()
    result_holder = {}

    async def _agent_lifecycle():
        try:
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

                if (
                    initial_history
                    and hasattr(agent, "conversation")
                    and agent.conversation is not None
                ):
                    _seed_conversation_history(agent.conversation, initial_history)

                result_holder["instance"] = agent
                enter_event.set()
                await exit_event.wait()
            finally:
                if _entered_agent:
                    await agent.__aexit__(*sys.exc_info())
        except (
            *_RECOVERABLE_ERRORS,
            asyncio.CancelledError,
            KeyboardInterrupt,
            SystemExit,
        ) as e:
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
            except _RECOVERABLE_ERRORS:
                logging.getLogger("agy_bridge.init").debug(
                    "asyncio.get_event_loop() failed, falling back to _agy_bridge_globals.EVENT_LOOP",
                    exc_info=True,
                )
                globals_mod = sys.modules.get("_agy_bridge_globals")
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
            except (TimeoutError, asyncio.CancelledError):
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
    import json
    import logging
    import sys

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
    _wire_inline_skills(local_config)
    _wire_subagents(local_config)
    _wire_budget_config(local_config)
    _wire_compaction_config(local_config)
    _wire_retry_config(local_config)
    _wire_system_instructions(local_config)
    _wire_policies(local_config, agent_id_u64)
    _wire_hooks(local_config, agent_id_u64)
    sdk_triggers = _wire_triggers(local_config)
    _wire_mcp_servers(local_config)

    if "gemini_config" in local_config and local_config["gemini_config"] is None:
        local_config.pop("gemini_config")
    # Enforced Rust-side by the ToolRegistry output floor on every tool dispatch.
    local_config.pop("tool_output_floor", None)

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

    config = LocalAgentConfig(triggers=sdk_triggers, **local_config)
    agent = agent_cls(config)

    return _build_agent_lifecycle(
        agent, agent_id_u64, initial_history, custom_base_url, passed_event_loop
    )
