def init_agent(config_json, agent_id_u64, agent_cls):
    import logging, sys

    logging.basicConfig(level=logging.INFO, stream=sys.stderr)
    logger = logging.getLogger("agy_bridge.init")

    try:
        from google.antigravity.connections.local.local_connection import (
            LocalConnection,
        )
        import asyncio

        if not getattr(LocalConnection, "_is_monkeypatched", False):
            logger.info(
                "[MONKEYPATCH] Applying LocalConnection fix for turn context and idle event race"
            )

            LocalConnection._real_current_turn_context = None

            @property
            def current_turn_context(self):
                return getattr(self, "_real_current_turn_context", None)

            @current_turn_context.setter
            def current_turn_context(self, value):
                self._real_current_turn_context = value
                if value is None:
                    if getattr(self, "_parent_idle", False) and not getattr(
                        self, "_active_subagent_ids", set()
                    ):
                        logger.info(
                            "[MONKEYPATCH] Triggering delayed is_idle.set() now that current_turn_context is None"
                        )
                        if hasattr(self, "_is_idle") and hasattr(
                            self._is_idle, "_event"
                        ):
                            self._is_idle._event.set()

            LocalConnection._current_turn_context = current_turn_context

            original_init = LocalConnection.__init__

            def patched_init(self, *args, **kwargs):
                original_init(self, *args, **kwargs)
                original_event = self._is_idle

                class PatchedEvent:
                    def __init__(self, event, conn):
                        self._event = event
                        self._conn = conn

                    def set(self):
                        if self._conn._current_turn_context is not None:
                            logger.info(
                                "[MONKEYPATCH] Delaying is_idle.set() because _current_turn_context is not None"
                            )
                            return
                        self._event.set()

                    def clear(self):
                        self._event.clear()

                    def is_set(self):
                        return self._event.is_set()

                    async def wait(self):
                        await self._event.wait()

                self._is_idle = PatchedEvent(original_event, self)
                self._real_current_turn_context = None

            LocalConnection.__init__ = patched_init
            LocalConnection._is_monkeypatched = True
    except Exception as e:
        logger.warning("Failed to apply LocalConnection monkeypatch: %s", e)

    class RateLimitInterceptor(logging.Handler):
        def emit(self, record):
            msg = self.format(record)
            if "HTTP 429" in msg or "HTTP 503" in msg or "quota" in msg.lower():
                gm = sys.modules.get("_agy_bridge_globals")
                if gm:
                    gm.RATE_LIMIT_HIT = True

    intercept = RateLimitInterceptor()
    logging.getLogger().addHandler(intercept)
    if "_agy_bridge_globals" not in sys.modules:
        import types

        sys.modules["_agy_bridge_globals"] = types.ModuleType("_agy_bridge_globals")

    from google.antigravity.tools import tool_runner
    import json

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
            args_json = json.dumps(kwargs)

            # PyO3 future_into_py returns an awaitable, but iscoroutinefunction is False.
            # We must call it on the main event loop thread, not in a thread pool.
            res = globals_mod.dispatch_rust_tool(
                int(self.agent_id), self.__name__, args_json
            )
            if inspect.isawaitable(res):
                return await res
            return res

    def create_proxy(agent_id, name, desc, schema):
        return AsyncRustProxy(agent_id, name, desc, schema)

    local_config = json.loads(config_json)

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
                    import sys, json, inspect

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
                        return input("Allow? [Y/n]: ").strip().lower() in (
                            "",
                            "y",
                            "yes",
                        )

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

    # --- Wire hooks ---
    hooks_entries = local_config.pop("hooks", [])

    if hooks_entries:
        try:
            from google.antigravity.hooks import hooks as hooks_module

            hook_logger = logging.getLogger("agy_bridge.hooks")

            import re

            registered_hooks = []
            for entry in hooks_entries:
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

                    async def _hook_callback(ctx=None):
                        import json, inspect

                        globals_mod = sys.modules.get("_agy_bridge_globals")
                        if not globals_mod or not hasattr(
                            globals_mod, "dispatch_rust_hook"
                        ):
                            hook_logger.warning(
                                "dispatch_rust_hook not found in _agy_bridge_globals, skipping hook %r",
                                name,
                            )
                            return

                        if point_label == "pre_tool_call_decide":
                            if globals_mod:
                                globals_mod.CURRENT_TOOL_CALL = ctx

                        # Map SDK context types to JSON for the Rust hook handler.
                        # The SDK passes known pydantic types: ToolCall, ToolResult,
                        # Content (str or BaseModel), or None (session hooks).
                        try:
                            if point_label == "post_tool_call":
                                current_tool_call = (
                                    getattr(globals_mod, "CURRENT_TOOL_CALL", None)
                                    if globals_mod
                                    else None
                                )
                                tool_args = (
                                    getattr(current_tool_call, "args", {})
                                    if current_tool_call
                                    else {}
                                )
                                if hasattr(tool_args, "model_dump"):
                                    tool_args = tool_args.model_dump()
                                elif hasattr(tool_args, "dict"):
                                    tool_args = tool_args.dict()

                                result_val = getattr(ctx, "result", None)
                                if result_val is None:
                                    result_str = ""
                                elif isinstance(result_val, str):
                                    result_str = result_val
                                else:
                                    try:
                                        if hasattr(result_val, "model_dump_json"):
                                            result_str = result_val.model_dump_json()
                                        elif hasattr(result_val, "model_dump"):
                                            result_str = json.dumps(
                                                result_val.model_dump()
                                            )
                                        else:
                                            result_str = json.dumps(result_val)
                                    except Exception:
                                        result_str = str(result_val)

                                tool_name = getattr(ctx, "name", "")
                                if hasattr(tool_name, "value"):
                                    tool_name = tool_name.value
                                elif not isinstance(tool_name, str):
                                    tool_name = str(tool_name)

                                payload = {
                                    "name": tool_name,
                                    "args": tool_args,
                                    "result": result_str,
                                }
                                ctx_json = json.dumps(payload)
                            elif point_label == "on_tool_error":
                                current_tool_call = (
                                    getattr(globals_mod, "CURRENT_TOOL_CALL", None)
                                    if globals_mod
                                    else None
                                )
                                tool_name = (
                                    getattr(current_tool_call, "name", "")
                                    if current_tool_call
                                    else ""
                                )
                                tool_args = (
                                    getattr(current_tool_call, "args", {})
                                    if current_tool_call
                                    else {}
                                )
                                if hasattr(tool_args, "model_dump"):
                                    tool_args = tool_args.model_dump()
                                elif hasattr(tool_args, "dict"):
                                    tool_args = tool_args.dict()

                                if hasattr(tool_name, "value"):
                                    tool_name = tool_name.value
                                elif not isinstance(tool_name, str):
                                    tool_name = str(tool_name)

                                payload = {
                                    "tool_name": tool_name,
                                    "tool_args": tool_args,
                                    "error": str(ctx),
                                }
                                ctx_json = json.dumps(payload)
                            elif ctx is None:
                                if point_label in (
                                    "on_session_start",
                                    "on_session_end",
                                ):
                                    conversation_id = local_config.get(
                                        "conversation_id"
                                    )
                                    if not conversation_id:
                                        workspaces = local_config.get("workspaces")
                                        if (
                                            workspaces
                                            and isinstance(workspaces, list)
                                            and len(workspaces) > 0
                                            and workspaces[0]
                                        ):
                                            import os

                                            conversation_id = os.path.basename(
                                                str(workspaces[0]).rstrip("/")
                                            )
                                    if not conversation_id:
                                        conversation_id = "default_session"
                                    payload = {
                                        "session": {
                                            "session_id": str(conversation_id),
                                            "agent_id": int(agent_id_u64),
                                        }
                                    }
                                    ctx_json = json.dumps(payload)
                                else:
                                    ctx_json = "{}"
                            elif point_label == "post_turn":
                                text_val = getattr(ctx, "text", str(ctx))
                                ctx_json = json.dumps(
                                    {
                                        "response_text": text_val,
                                        "turn_number": getattr(ctx, "turn_number", 0),
                                    }
                                )
                            elif point_label == "pre_turn":
                                text_val = ctx if isinstance(ctx, str) else str(ctx)
                                ctx_json = json.dumps(
                                    {
                                        "prompt": text_val,
                                        "turn_number": getattr(ctx, "turn_number", 0),
                                    }
                                )
                            elif isinstance(ctx, str):
                                ctx_json = json.dumps({"value": ctx})
                            elif hasattr(ctx, "model_dump_json"):
                                ctx_json = ctx.model_dump_json()
                            elif isinstance(ctx, dict):
                                ctx_json = json.dumps(ctx)
                            else:
                                ctx_json = json.dumps(str(ctx))
                        except Exception as e:
                            hook_logger.error(
                                "Failed to serialize hook context for %r: %s", name, e
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
                            if point_label in (
                                "pre_turn",
                                "pre_tool_call_decide",
                                "on_interaction",
                            ):
                                return hooks_module.HookResult(
                                    allow=True, message=str(e)
                                )
                            return

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

                        if point_label in (
                            "pre_turn",
                            "pre_tool_call_decide",
                            "on_interaction",
                        ):
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
                hook_logger.info("Registered hook %r at point %s", hook_name, sdk_point)

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
        except Exception as exc:

            hook_logger = logging.getLogger("agy_bridge.hooks")
            hook_logger.error("Failed to register hooks: %s", exc)

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
                trigger_name = entry.get("name", "unnamed")
                config = entry.get("config", {})
                message_template = entry.get("message_template", "")

                callback = _make_trigger_cb(trigger_name, message_template)

                if "Every" in config:
                    interval_secs = config["Every"].get("interval", 0)
                    sdk_triggers.append(every(interval_secs, callback))
                    trigger_logger.info(
                        "Registered every(%ds) trigger %r", interval_secs, trigger_name
                    )
                elif "OnFileChange" in config:
                    path = config["OnFileChange"].get("path", "")
                    sdk_triggers.append(on_file_change(path, callback))
                    trigger_logger.info(
                        "Registered on_file_change(%r) trigger %r", path, trigger_name
                    )
                else:
                    trigger_logger.warning(
                        "Unknown trigger config for %r: %s, skipping",
                        trigger_name,
                        config,
                    )
        except Exception as exc:
            trigger_logger = logging.getLogger("agy_bridge.triggers")
            trigger_logger.error("Failed to register triggers: %s", exc)

    # --- Wire MCP Servers ---
    # Rust serializes MCP servers as JSON dicts with a "type" discriminator.
    # Convert them to the correct SDK pydantic types.
    if "mcp_servers" in local_config and local_config["mcp_servers"]:
        try:
            import google.antigravity.types as agy_types

            parsed_mcp = []
            for mcp in local_config.pop("mcp_servers"):
                typ = mcp.pop("type", None)
                if typ == "stdio":
                    parsed_mcp.append(agy_types.McpStdioServer(**mcp))
                elif typ == "sse":
                    parsed_mcp.append(agy_types.McpSseServer(**mcp))
                elif typ == "http":
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

    # Rust serializes `skills` as `"skills_paths"` and `gemini` as
    # `"gemini_config"` via serde(rename), so no Python-side renaming needed.
    # Strip null gemini_config so the SDK uses its defaults.
    if "gemini_config" in local_config and local_config["gemini_config"] is None:
        local_config.pop("gemini_config")

    from google.antigravity.connections.local.local_connection_config import (
        LocalAgentConfig,
    )

    config = LocalAgentConfig(triggers=sdk_triggers, **local_config)
    agent = agent_cls(config)

    import asyncio

    enter_event = asyncio.Event()
    exit_event = asyncio.Event()
    result_holder = {}

    async def _agent_lifecycle():
        try:
            async with agent:
                result_holder["instance"] = agent
                enter_event.set()
                await exit_event.wait()
        except Exception as e:
            result_holder["error"] = e
            if not enter_event.is_set():
                enter_event.set()

    globals_mod = sys.modules.get("_agy_bridge_globals")
    if not globals_mod or not hasattr(globals_mod, "EVENT_LOOP"):
        raise RuntimeError("EVENT_LOOP not found in _agy_bridge_globals")
    loop = globals_mod.EVENT_LOOP
    future = asyncio.run_coroutine_threadsafe(_agent_lifecycle(), loop)

    class AgentLifecycleController:
        def __init__(self, future, exit_event):
            self.future = future
            self.exit_event = exit_event

        async def __aexit__(self, exc_type, exc_val, exc_tb):
            self.exit_event.set()
            try:
                await asyncio.wrap_future(self.future)
            except asyncio.CancelledError:
                pass

    async def _wait_for_enter():
        await enter_event.wait()
        if "error" in result_holder:
            raise result_holder["error"]
        return result_holder["instance"]

    controller = AgentLifecycleController(future, exit_event)
    return (controller, _wait_for_enter())
