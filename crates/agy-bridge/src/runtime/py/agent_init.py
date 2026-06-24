def init_agent(config_json, agent_id_u64, agent_cls, passed_event_loop):
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

                self._is_idle = PatchedEvent(original_event, self)
                self._real_current_turn_context = None
                self._idle_deferred = False

            LocalConnection.__init__ = patched_init

            original_tool_result_to_dict = LocalConnection._tool_result_to_dict

            def patched_tool_result_to_dict(self, result):
                if result.error is not None:
                    return {"error": result.error}

                output = result.result
                if isinstance(output, dict) and "content" in output:
                    return {"result": output["content"]}

                return original_tool_result_to_dict(self, result)

            LocalConnection._tool_result_to_dict = patched_tool_result_to_dict
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
                                if point_label in (
                                    "pre_turn",
                                    "pre_tool_call_decide",
                                    "on_interaction",
                                ):
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
                                if point_label == "post_tool_call":
                                    current_tool_call = (
                                        globals_mod.CURRENT_TOOL_CALLS.get(agent_id_u64)
                                        if globals_mod
                                        and hasattr(globals_mod, "CURRENT_TOOL_CALLS")
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
                                    result_str = ""
                                    metadata = {}
                                    if result_val is None:
                                        result_str = ""
                                    elif isinstance(result_val, str):
                                        result_str = result_val
                                    elif (
                                        isinstance(result_val, dict)
                                        and "content" in result_val
                                    ):
                                        result_str = result_val["content"]
                                        metadata = result_val.get("metadata", {})
                                    else:
                                        tool_output = getattr(
                                            result_val, "result", None
                                        )
                                        if (
                                            isinstance(tool_output, dict)
                                            and "content" in tool_output
                                        ):
                                            result_str = tool_output["content"]
                                            metadata = tool_output.get("metadata", {})
                                        else:
                                            try:
                                                if hasattr(
                                                    result_val, "model_dump_json"
                                                ):
                                                    result_str = (
                                                        result_val.model_dump_json()
                                                    )
                                                elif hasattr(result_val, "model_dump"):
                                                    result_str = json.dumps(
                                                        result_val.model_dump()
                                                    )
                                                else:
                                                    result_str = json.dumps(result_val)
                                            except Exception:
                                                logging.getLogger(
                                                    "agy_bridge.tool_dispatch"
                                                ).warning(
                                                    "Failed to JSON-serialize tool result, falling back to str()",
                                                    exc_info=True,
                                                )
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
                                        "metadata": metadata,
                                    }
                                    ctx_json = json.dumps(payload)
                                elif point_label == "on_tool_error":
                                    current_tool_call = (
                                        globals_mod.CURRENT_TOOL_CALLS.get(agent_id_u64)
                                        if globals_mod
                                        and hasattr(globals_mod, "CURRENT_TOOL_CALLS")
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
                                            "turn_number": getattr(
                                                ctx, "turn_number", 0
                                            ),
                                        }
                                    )
                                elif point_label == "pre_turn":
                                    text_val = ctx if isinstance(ctx, str) else str(ctx)
                                    ctx_json = json.dumps(
                                        {
                                            "prompt": text_val,
                                            "turn_number": getattr(
                                                ctx, "turn_number", 0
                                            ),
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
                                    "Failed to serialize hook context for %r: %s",
                                    name,
                                    e,
                                )
                                if point_label in (
                                    "pre_turn",
                                    "pre_tool_call_decide",
                                    "on_interaction",
                                ):
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
                                if point_label in (
                                    "pre_turn",
                                    "pre_tool_call_decide",
                                    "on_interaction",
                                ):
                                    return hooks_module.HookResult(
                                        allow=False, message=str(e)
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
                                except json.JSONDecodeError as e:
                                    hook_logger.error(
                                        "Failed to decode hook result JSON %r: %s",
                                        result_json,
                                        e,
                                    )
                                    if point_label in (
                                        "pre_turn",
                                        "pre_tool_call_decide",
                                        "on_interaction",
                                    ):
                                        return hooks_module.HookResult(
                                            allow=False,
                                            message=f"Invalid hook result JSON: {e}",
                                        )

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

    if custom_base_url:
        # When routing through a proxy/gateway that handles auth (e.g., via
        # LOAS, mTLS, or bearer tokens), no API key is needed.  Set a
        # sentinel so the SDK's API key validation passes.
        if "gemini_config" in local_config and local_config["gemini_config"]:
            if not local_config["gemini_config"].get("api_key"):
                local_config["gemini_config"]["api_key"] = "LOAS"

        from google.antigravity.connections.local.local_connection import (
            LocalConnectionStrategy,
        )

        # Per-instance registry: maps strategy id() → base_url.
        # The class-level patch is installed once and dispatches
        # per-instance using this registry.
        if not hasattr(LocalConnectionStrategy, "_base_url_registry"):
            LocalConnectionStrategy._base_url_registry = {}
            _original_build = LocalConnectionStrategy._build_harness_config

            def _patched_build(self):
                config = _original_build(self)
                url = LocalConnectionStrategy._base_url_registry.get(id(self))
                if url:
                    config.gemini_config.base_url = url
                    if config.gemini_config.api_key == "LOAS":
                        config.gemini_config.ClearField("api_key")
                        logger.info(
                            "Injected base_url=%s into harness config (auth sentinel cleared)",
                            url,
                        )
                    else:
                        logger.info(
                            "Injected base_url=%s into harness config (real api_key kept)",
                            url,
                        )
                return config

            LocalConnectionStrategy._build_harness_config = _patched_build

            # Also patch __init__ to register from the pending queue
            _original_strategy_init = LocalConnectionStrategy.__init__

            def _patched_strategy_init(self, *args, **kwargs):
                _original_strategy_init(self, *args, **kwargs)
                # Consume the pending base_url (set by _agent_lifecycle just
                # before __aenter__) and associate it with this instance.
                pending = getattr(LocalConnectionStrategy, "_pending_base_url", None)
                if pending is not None:
                    LocalConnectionStrategy._base_url_registry[id(self)] = pending
                    LocalConnectionStrategy._pending_base_url = None

            LocalConnectionStrategy.__init__ = _patched_strategy_init

    # The Rust AgentConfig always serializes a `model` field (it's a required
    # String, not Option).  The SDK's LocalAgentConfig validator rejects configs
    # that set *both* the top-level `model` shorthand and
    # `gemini_config.models.default`.  When gemini_config carries the model via
    # `models.default`, drop the redundant top-level key.
    if "gemini_config" in local_config and local_config.get("gemini_config"):
        local_config.pop("model", None)

    from google.antigravity.connections.local.local_connection_config import (
        LocalAgentConfig,
    )

    config = LocalAgentConfig(triggers=sdk_triggers, **local_config)
    agent = agent_cls(config)

    # Store the base_url on the agent object so _agent_lifecycle can
    # set _pending_base_url at the right time (on the event loop thread,
    # right before __aenter__).
    if custom_base_url:
        agent._agy_pending_base_url = custom_base_url

    import asyncio

    enter_event = asyncio.Event()
    exit_event = asyncio.Event()
    result_holder = {}

    async def _agent_lifecycle():
        try:
            # Set _pending_base_url HERE, on the event loop thread, right
            # before __aenter__. This avoids the race where another
            # init_agent() call on the Python thread could overwrite it.
            #
            # The asyncio event loop is single-threaded and cooperative:
            # no other coroutine can interleave between setting the pending
            # URL and the strategy's __init__ consuming it during __aenter__.
            pending_url = getattr(agent, "_agy_pending_base_url", None)
            if pending_url is not None:
                from google.antigravity.connections.local.local_connection import (
                    LocalConnectionStrategy,
                )

                LocalConnectionStrategy._pending_base_url = pending_url
                delattr(agent, "_agy_pending_base_url")

            async with agent:
                result_holder["instance"] = agent
                enter_event.set()
                await exit_event.wait()
        except Exception as e:
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
