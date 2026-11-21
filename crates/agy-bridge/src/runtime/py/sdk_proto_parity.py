# Fail-loud forwarding of localharness proto fields the SDK's pydantic types lack.

from __future__ import annotations

import logging
from typing import Any

try:
    from agent_init_helpers import _get_monkeypatch_lock, _resolve_installed_sdk_version
except ImportError as _imp_err:
    logging.getLogger("agy_bridge.init").debug(
        "agent_init_helpers not pre-imported: %s", _imp_err
    )


class SdkFeatureUnsupportedError(Exception):
    """A configured feature has no field in the installed SDK's localharness proto."""


PROTO_SUBAGENTS_CONFIG = "SubagentsConfig"
PROTO_CUSTOM_SI_PART = "CustomSystemInstructions.Part"
PROTO_COMPACTION_CONFIG = "CompactionConfig"
PROTO_MCP_SERVER_CONFIG = "McpServerConfig"
FIELD_DISABLE_DEFINE_SUBAGENT_TOOL = "disable_define_subagent_tool"
FIELD_DISABLE_MANAGE_SUBAGENTS_TOOL = "disable_manage_subagents_tool"
FIELD_TITLE = "title"
FIELD_INCLUDE_LAST_USER_MESSAGE = "include_last_user_message"
FIELD_MCP_ENABLED_TOOLS = "enabled_tools"
FIELD_MCP_DISABLED_TOOLS = "disabled_tools"
FIELD_MCP_TIMEOUT_SECONDS = "timeout_seconds"
FIELD_MCP_FORCE_ALL_TOOLS_EAGER = "force_all_tools_eager"
ATTR_DISABLE_DEFINE_SUBAGENT_TOOL = "_agy_disable_define_subagent_tool"
ATTR_DISABLE_MANAGE_SUBAGENTS_TOOL = "_agy_disable_manage_subagents_tool"
ATTR_CUSTOM_TITLE = "_agy_custom_title"


def _sdk_proto_supports(message_path: str, field: str) -> bool:
    """Return whether the SDK's bundled localharness proto message has `field`."""
    from google.antigravity.proto import localharness_pb2

    message: Any = localharness_pb2
    for part in message_path.split("."):
        message = getattr(message, part)
    return field in message.DESCRIPTOR.fields_by_name


def _require_sdk_proto_field(message_path: str, field: str, feature: str) -> None:
    """Fail loudly when `feature` cannot reach localharness through the installed SDK."""
    if not _sdk_proto_supports(message_path, field):
        raise SdkFeatureUnsupportedError(
            f"{feature} needs localharness proto field {message_path}.{field}, "
            f"which google-antigravity {_resolve_installed_sdk_version()} lacks; "
            "use the native runtime or upgrade the SDK"
        )


def _patch_harness_side_tools_proto(strategy_cls: Any, logger: logging.Logger) -> bool:
    """Teach `_to_harness_side_tools_proto` to forward `disable_define_subagent_tool`."""
    with _get_monkeypatch_lock():
        if getattr(strategy_cls, "_agy_harness_side_tools_patched", False):
            return True
        original = getattr(strategy_cls, "_to_harness_side_tools_proto", None)
        if original is None:
            logger.warning(
                "[MONKEYPATCH] SDK structural drift: LocalConnectionStrategy has no "
                "_to_harness_side_tools_proto; disable_define_subagent_tool unsupported"
            )
            return False

        def _patched(self: Any, cfg: Any, is_subagent: bool = False) -> Any:
            proto = original(self, cfg, is_subagent=is_subagent)
            has_ws_only = any(
                getattr(p, "name", "") == "workspace_only"
                for p in (getattr(self, "_policies", None) or ())
            )
            if has_ws_only and not is_subagent:
                proto.permissions.enforce_workspace_validation = True
            if getattr(cfg, ATTR_DISABLE_DEFINE_SUBAGENT_TOOL, False):
                setattr(proto.subagents, FIELD_DISABLE_DEFINE_SUBAGENT_TOOL, True)
            if getattr(cfg, ATTR_DISABLE_MANAGE_SUBAGENTS_TOOL, False):
                setattr(proto.subagents, FIELD_DISABLE_MANAGE_SUBAGENTS_TOOL, True)
            return proto

        strategy_cls._to_harness_side_tools_proto = _patched
        strategy_cls._agy_harness_side_tools_patched = True
        logger.info("[MONKEYPATCH] Enabled disable_define_subagent_tool forwarding")
        return True


def _patch_compaction_config_proto(local_conn_mod: Any, logger: logging.Logger) -> bool:
    """Teach `to_proto_compaction_config` and `to_system_instructions_proto` to forward extra fields."""
    with _get_monkeypatch_lock():
        if getattr(local_conn_mod, "_agy_compaction_proto_patched", False):
            return True
        original = getattr(local_conn_mod, "to_proto_compaction_config", None)
        if original is None:
            return False

        def _patched(compaction_config: Any, capabilities: Any) -> Any:
            proto, legacy = original(compaction_config, capabilities)
            extras = getattr(compaction_config, "_agy_extra_compaction", None) or {}
            if proto is not None and extras:
                if extras.get("checkpoint_interval_tokens") is not None:
                    proto.checkpoint_interval_tokens = int(
                        extras["checkpoint_interval_tokens"]
                    )
                if extras.get("max_context_tokens") is not None:
                    proto.max_context_tokens = int(extras["max_context_tokens"])
                if extras.get("summary_prompt_override"):
                    proto.summary_prompt_override = str(
                        extras["summary_prompt_override"]
                    )
                if extras.get("use_model_reported_token_count") is not None:
                    proto.use_model_reported_token_count = bool(
                        extras["use_model_reported_token_count"]
                    )
                if extras.get(FIELD_INCLUDE_LAST_USER_MESSAGE):
                    setattr(proto, FIELD_INCLUDE_LAST_USER_MESSAGE, True)
            return proto, legacy

        local_conn_mod.to_proto_compaction_config = _patched
        orig_si = getattr(local_conn_mod, "to_system_instructions_proto", None)
        if orig_si is not None:

            def _patched_si(instructions: Any) -> Any:
                proto = orig_si(instructions)
                title = getattr(instructions, ATTR_CUSTOM_TITLE, None)
                if (
                    proto is not None
                    and title
                    and proto.HasField("custom")
                    and proto.custom.part
                ):
                    setattr(proto.custom.part[0], FIELD_TITLE, str(title))
                return proto

            local_conn_mod.to_system_instructions_proto = _patched_si
        local_conn_mod._agy_compaction_proto_patched = True
        logger.info("[MONKEYPATCH] Enabled full CompactionConfig proto forwarding")
        return True


def _validate_mcp_server_parity(mcp: dict[str, Any]) -> None:
    """Ensure configured per-server MCP fields exist on the installed SDK proto."""
    if mcp.get(FIELD_MCP_ENABLED_TOOLS) is not None:
        _require_sdk_proto_field(
            PROTO_MCP_SERVER_CONFIG,
            FIELD_MCP_ENABLED_TOOLS,
            "McpServer.enabled_tools",
        )
    if mcp.get(FIELD_MCP_DISABLED_TOOLS) is not None:
        _require_sdk_proto_field(
            PROTO_MCP_SERVER_CONFIG,
            FIELD_MCP_DISABLED_TOOLS,
            "McpServer.disabled_tools",
        )
    if mcp.get(FIELD_MCP_TIMEOUT_SECONDS) is not None:
        _require_sdk_proto_field(
            PROTO_MCP_SERVER_CONFIG,
            FIELD_MCP_TIMEOUT_SECONDS,
            "McpServer.timeout_seconds",
        )
    if mcp.get(FIELD_MCP_FORCE_ALL_TOOLS_EAGER):
        _require_sdk_proto_field(
            PROTO_MCP_SERVER_CONFIG,
            FIELD_MCP_FORCE_ALL_TOOLS_EAGER,
            "McpServer.force_all_tools_eager",
        )


_SKILL_COERCE_ERRORS = (
    ImportError,
    RuntimeError,
    TypeError,
    ValueError,
    KeyError,
    AttributeError,
)


def _parse_inline_skill(sk: Any) -> Any:
    """Convert a serialized InlineSkill dict into an SDK InlineSkill object."""
    if not isinstance(sk, dict):
        return sk
    try:
        from google.antigravity.types import InlineSkill

        sk_copy = dict(sk)
        if hasattr(InlineSkill, "model_fields"):
            allowed = set(InlineSkill.model_fields.keys())
            sk_copy = {k: v for k, v in sk_copy.items() if k in allowed}
        return InlineSkill(**sk_copy)
    except _SKILL_COERCE_ERRORS:
        return sk


def _parse_subagent_skills_config(sc: Any) -> Any:
    """Convert a serialized Rust `SubagentSkillsConfig` enum dict into SDK types."""
    if sc is None:
        return None
    try:
        from google.antigravity.types import (
            SubagentInheritSkillsConfig,
            SubagentNoneSkillsConfig,
            SubagentOverrideSkillsConfig,
        )

        if sc == "none" or (isinstance(sc, dict) and "none" in sc):
            return SubagentNoneSkillsConfig()
        if isinstance(sc, dict):
            if "inherit" in sc and isinstance(sc["inherit"], dict):
                inh = sc["inherit"]
                return SubagentInheritSkillsConfig(
                    skill_names=inh.get("skill_names") or [],
                    extra_skills_paths=[
                        str(p) for p in (inh.get("extra_skills_paths") or [])
                    ],
                )
            if "override" in sc and isinstance(sc["override"], dict):
                ovr = sc["override"]
                return SubagentOverrideSkillsConfig(
                    skills_paths=[str(p) for p in (ovr.get("skills_paths") or [])],
                    inline_skills=[
                        _parse_inline_skill(sk)
                        for sk in (ovr.get("inline_skills") or [])
                    ],
                )
    except _SKILL_COERCE_ERRORS as exc:
        logging.getLogger("agy_bridge.init").debug(
            "SubagentSkillsConfig coercion skipped: %s", exc
        )
    return None


def _coerce_system_instructions(si: Any) -> Any:
    """Convert serialized Rust `SystemInstructions` into SDK types."""
    from google.antigravity.types import (
        CustomSystemInstructions,
        SystemInstructionSection,
        TemplatedSystemInstructions,
    )

    if isinstance(si, str):
        return CustomSystemInstructions(text=si)
    if not isinstance(si, dict):
        return si

    if ("text" in si or "custom_instructions" in si) and "sections" not in si:
        text_val = si.get("text") or si.get("custom_instructions") or ""
        si_kwargs: dict[str, Any] = {"text": text_val}
        custom_title = si.get(FIELD_TITLE)
        sdk_has_title = (
            hasattr(CustomSystemInstructions, "model_fields")
            and FIELD_TITLE in CustomSystemInstructions.model_fields
        )
        if custom_title and sdk_has_title:
            si_kwargs[FIELD_TITLE] = custom_title
        elif custom_title:
            _require_sdk_proto_field(
                PROTO_CUSTOM_SI_PART,
                FIELD_TITLE,
                "SystemInstructions::TitledCustom",
            )
        sdk_si = CustomSystemInstructions(**si_kwargs)
        if custom_title and not sdk_has_title:
            object.__setattr__(sdk_si, ATTR_CUSTOM_TITLE, str(custom_title))
        return sdk_si

    if "sections" in si or "identity" in si:
        sections_raw = si.get("sections") or []
        sections = [
            SystemInstructionSection(**sec) if isinstance(sec, dict) else sec
            for sec in sections_raw
        ]
        identity_val = si.get("identity")
        if identity_val is None and "base_instructions" in si:
            identity_val = si.get("base_instructions")
        return TemplatedSystemInstructions(identity=identity_val, sections=sections)
    return si


def _schema_expects_integer(schema: Any, root_schema: dict[str, Any]) -> bool:
    if not isinstance(schema, dict):
        return False
    ref = schema.get("$ref")
    if isinstance(ref, str) and ref.startswith("#/"):
        node: Any = root_schema
        for seg in ref[2:].split("/"):
            node = node.get(seg) if isinstance(node, dict) else None
        if isinstance(node, dict):
            return _schema_expects_integer(node, root_schema)
    ty = schema.get("type")
    if ty == "integer":
        return True
    if isinstance(ty, list) and "integer" in ty and "number" not in ty:
        return True
    for union_key in ("anyOf", "oneOf"):
        variants = schema.get(union_key)
        if isinstance(variants, list) and any(
            _schema_expects_integer(v, root_schema) for v in variants
        ):
            return True
    return False


def _coerce_schema_integers(
    value: Any, schema: Any, root_schema: dict[str, Any]
) -> Any:
    """Coerce whole-valued `float` (`45.0`) unpacked from `protobuf.Struct` back to `int` for integer schema fields."""
    if not isinstance(schema, dict):
        return value
    ref = schema.get("$ref")
    if isinstance(ref, str) and ref.startswith("#/"):
        node: Any = root_schema
        for seg in ref[2:].split("/"):
            node = node.get(seg) if isinstance(node, dict) else None
        if isinstance(node, dict):
            return _coerce_schema_integers(value, node, root_schema)
    if (
        isinstance(value, float)
        and not isinstance(value, bool)
        and value.is_integer()
        and _schema_expects_integer(schema, root_schema)
    ):
        return int(value)
    if isinstance(value, dict):
        props = schema.get("properties")
        if isinstance(props, dict):
            return {
                k: _coerce_schema_integers(v, props.get(k), root_schema)
                for k, v in value.items()
            }
    if isinstance(value, list) and "items" in schema:
        item_schema = schema.get("items")
        return [_coerce_schema_integers(v, item_schema, root_schema) for v in value]
    return value


def _wire_tool_proxies(local_config: dict[str, Any], agent_id_u64: int) -> None:
    """Replace tool/capability dict specs with AsyncRustProxy instances."""
    from google.antigravity.tools import tool_runner

    class AsyncRustProxy(tool_runner.ToolWithSchema):
        def __init__(
            self,
            agent_id: int,
            name: str,
            description: str,
            schema_dict: dict[str, Any],
        ) -> None:
            self.agent_id = str(agent_id)
            self.__name__ = name
            self.__doc__ = description
            self.input_schema = schema_dict
            self.fn = self.__call__

        async def __call__(self, **kwargs: Any) -> Any:
            import inspect
            import json
            import sys

            globals_mod: Any = sys.modules.get("_agy_bridge_globals")
            if not globals_mod or not hasattr(globals_mod, "dispatch_rust_tool"):
                raise RuntimeError(
                    "dispatch_rust_tool not found in _agy_bridge_globals"
                )

            class DummyToolCall:
                def __init__(self, name: str, args: dict[str, Any]) -> None:
                    self.name = name
                    self.args = args

            active_call_id = (
                globals_mod.get_current_client_tool_call_id()
                if hasattr(globals_mod, "get_current_client_tool_call_id")
                else None
            )
            coerced_kwargs = _coerce_schema_integers(
                kwargs, self.input_schema, self.input_schema
            )
            if hasattr(globals_mod, "_store_tracked_tool_call"):
                globals_mod._store_tracked_tool_call(
                    globals_mod,
                    int(self.agent_id),
                    active_call_id,
                    DummyToolCall(self.__name__, coerced_kwargs),
                )
            else:
                if not hasattr(globals_mod, "CURRENT_TOOL_CALLS"):
                    globals_mod.CURRENT_TOOL_CALLS = {}
                globals_mod.CURRENT_TOOL_CALLS[int(self.agent_id)] = {
                    str(active_call_id) if active_call_id else "_latest": DummyToolCall(
                        self.__name__, coerced_kwargs
                    )
                }
            args_json = json.dumps(coerced_kwargs)

            res = globals_mod.dispatch_rust_tool(
                int(self.agent_id), self.__name__, args_json
            )
            if inspect.isawaitable(res):
                res = await res
            if isinstance(res, dict) and "content" in res:
                if hasattr(globals_mod, "_store_tool_metadata"):
                    globals_mod._store_tool_metadata(
                        globals_mod,
                        int(self.agent_id),
                        active_call_id,
                        self.__name__,
                        res.get("metadata"),
                    )
                return res["content"]
            return res

    def create_proxy(
        agent_id: int, name: str, desc: str, schema: dict[str, Any]
    ) -> AsyncRustProxy:
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


def _unwrap_stdio_env_wrapper(mcp: dict[str, Any]) -> None:
    """Unwrap an `env KEY=VALUE … cmd args` stdio wrapper into `mcp["env"]`."""
    if mcp.get("command") != "env" or not isinstance(mcp.get("args"), list):
        return
    raw_args = mcp["args"]
    env_dict: dict[str, str] = {}
    idx = 0
    while idx < len(raw_args) and isinstance(raw_args[idx], str):
        item = raw_args[idx]
        if "=" not in item:
            break
        key, val = item.split("=", 1)
        if not key or not key.isidentifier():
            break
        env_dict[key] = val
        idx += 1
    if env_dict and idx < len(raw_args):
        mcp["command"] = raw_args[idx]
        mcp["args"] = raw_args[idx + 1 :]
        existing_env = mcp.get("env")
        if isinstance(existing_env, dict):
            env_dict.update(existing_env)
        mcp["env"] = env_dict


def _wire_subagents(local_config: dict[str, Any]) -> None:
    """Translate serialized subagent specs into SDK SubagentConfig objects."""
    if local_config.get("subagents"):
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
        except (*_SKILL_COERCE_ERRORS, OSError):
            return

        parsed_subagents = []
        for s in local_config["subagents"]:
            if isinstance(s, dict):
                caps = None
                if s.get("capabilities"):
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
                        except (*_SKILL_COERCE_ERRORS, OSError) as exc:
                            logging.getLogger("agy_bridge.init").debug(
                                "subagent RunCommandConfig coercion skipped: %s", exc
                            )
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
                    if c_dict.get(FIELD_DISABLE_DEFINE_SUBAGENT_TOOL):
                        _require_sdk_proto_field(
                            PROTO_SUBAGENTS_CONFIG,
                            FIELD_DISABLE_DEFINE_SUBAGENT_TOOL,
                            "SubagentCapabilities.disable_define_subagent_tool",
                        )
                        object.__setattr__(
                            caps, ATTR_DISABLE_DEFINE_SUBAGENT_TOOL, True
                        )
                    if c_dict.get(FIELD_DISABLE_MANAGE_SUBAGENTS_TOOL):
                        _require_sdk_proto_field(
                            PROTO_SUBAGENTS_CONFIG,
                            FIELD_DISABLE_MANAGE_SUBAGENTS_TOOL,
                            "SubagentCapabilities.disable_manage_subagents_tool",
                        )
                        object.__setattr__(
                            caps, ATTR_DISABLE_MANAGE_SUBAGENTS_TOOL, True
                        )
                sub_si = s.get("system_instructions")
                if sub_si is not None:
                    sub_si = _coerce_system_instructions(sub_si)
                sub_kwargs: dict[str, Any] = {
                    "name": s["name"],
                    "description": s.get("description", ""),
                    "system_instructions": sub_si,
                    "capabilities": caps,
                    "tools": s.get("tools", []),
                }
                if s.get("model"):
                    sub_kwargs["model"] = s["model"]
                skills_cfg = _parse_subagent_skills_config(s.get("skills_config"))
                if skills_cfg is not None:
                    sub_kwargs["skills_config"] = skills_cfg
                if hasattr(SubagentConfig, "model_fields"):
                    allowed = set(SubagentConfig.model_fields.keys())
                    sub_kwargs = {k: v for k, v in sub_kwargs.items() if k in allowed}
                parsed_subagents.append(SubagentConfig(**sub_kwargs))
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
        if cc.get(FIELD_INCLUDE_LAST_USER_MESSAGE):
            _require_sdk_proto_field(
                PROTO_COMPACTION_CONFIG,
                FIELD_INCLUDE_LAST_USER_MESSAGE,
                "CompactionConfig.include_last_user_message",
            )
        try:
            from google.antigravity.types import CompactionConfig

            raw_threshold = cc.get("token_threshold")
            token_threshold = (
                int(raw_threshold)
                if raw_threshold is not None and int(raw_threshold) > 0
                else None
            )
            extras = {
                k: cc[k]
                for k in (
                    "checkpoint_interval_tokens",
                    "max_context_tokens",
                    "summary_prompt_override",
                    "use_model_reported_token_count",
                    "include_last_user_message",
                )
                if cc.get(k) is not None
            }
            if token_threshold is not None or extras:
                cc_kwargs: dict[str, Any] = {}
                if token_threshold is not None:
                    cc_kwargs["token_threshold"] = token_threshold
                cc_kwargs.update(extras)
                if hasattr(CompactionConfig, "model_fields"):
                    allowed = set(CompactionConfig.model_fields.keys())
                    cc_kwargs = {k: v for k, v in cc_kwargs.items() if k in allowed}
                sdk_cc = (
                    CompactionConfig(**cc_kwargs)
                    if token_threshold is not None
                    else CompactionConfig.model_construct(token_threshold=None)
                )
                object.__setattr__(sdk_cc, "_agy_extra_compaction", extras)
                local_config["compaction_config"] = sdk_cc
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


def _decode_question_hook_result(result_json: str, hook_logger: logging.Logger) -> Any:
    """Decode a serialized Rust `QuestionHookResult` into an SDK `QuestionHookResult`."""
    import json

    from google.antigravity import types as sdk_types

    if not result_json:
        return sdk_types.QuestionHookResult(responses=[], cancelled=False)
    try:
        res_dict = json.loads(result_json)
        if not isinstance(res_dict, dict):
            return sdk_types.QuestionHookResult(responses=[], cancelled=False)
        raw_responses = res_dict.get("responses") or []
        parsed_responses = []
        for item in raw_responses:
            if isinstance(item, dict):
                sel = item.get("selected_option_ids")
                parsed_responses.append(
                    sdk_types.QuestionResponse(
                        selected_option_ids=(
                            [str(x) for x in sel] if isinstance(sel, list) else None
                        ),
                        freeform_response=str(item.get("freeform_response") or ""),
                        skipped=bool(item.get("skipped", False)),
                    )
                )
        return sdk_types.QuestionHookResult(
            responses=parsed_responses,
            cancelled=bool(res_dict.get("cancelled", False)),
        )
    except (json.JSONDecodeError, TypeError, ValueError) as exc:
        hook_logger.error("Failed to decode on_interaction result JSON: %s", exc)
        return sdk_types.QuestionHookResult(responses=[], cancelled=True)


def _build_vertex_model_target(
    entry: Any,
    vertex_cfg: dict[str, Any],
    env_map: dict[str, str],
    fallback_model_name: str,
    image_shorthand: str | None,
    is_image: bool,
) -> Any:
    """Build an SDK `ModelTarget` backed by `VertexEndpoint`."""
    import os

    from google.antigravity import models as sdk_models
    from google.antigravity.types import (
        GeminiModelOptions,
        ModelTarget,
        ModelType,
        ServiceTier,
        ThinkingLevel,
        VertexEndpoint,
    )

    entry_dict = entry if isinstance(entry, dict) else {"name": str(entry or "")}
    raw_name = entry_dict.get("name") or ""
    if is_image:
        if (
            image_shorthand
            and image_shorthand != sdk_models.DEFAULT_IMAGE_GENERATION_MODEL
            and (not raw_name or raw_name == sdk_models.DEFAULT_IMAGE_GENERATION_MODEL)
        ):
            name = image_shorthand
        else:
            name = raw_name or sdk_models.DEFAULT_IMAGE_GENERATION_MODEL
    else:
        if not raw_name or raw_name == sdk_models.DEFAULT_MODEL:
            name = fallback_model_name or raw_name or sdk_models.DEFAULT_MODEL
        else:
            name = raw_name

    gen = entry_dict.get("generation") or {}
    options = None
    if isinstance(gen, dict) and (
        gen.get("thinking_level")
        or gen.get("service_tier")
        or gen.get("max_output_tokens") is not None
    ):
        tl_raw = gen.get("thinking_level")
        tl_norm = str(tl_raw).lower() if tl_raw else None
        if tl_norm == "extra_high":
            tl_norm = "xhigh"
        st_raw = gen.get("service_tier")
        mot_raw = gen.get("max_output_tokens")
        opt_kwargs: dict[str, Any] = {
            "thinking_level": ThinkingLevel(tl_norm) if tl_norm else None,
            "service_tier": ServiceTier(str(st_raw).lower()) if st_raw else None,
        }
        if mot_raw is not None and int(mot_raw) > 0:
            opt_kwargs["max_output_tokens"] = int(mot_raw)
        options = GeminiModelOptions(**opt_kwargs)

    base_url = vertex_cfg.get("base_url") or None
    api_key = entry_dict.get("api_key") or vertex_cfg.get("api_key") or None
    use_ambient = not base_url and not api_key
    project = vertex_cfg.get("project") or (
        (env_map.get("GOOGLE_CLOUD_PROJECT") or os.environ.get("GOOGLE_CLOUD_PROJECT"))
        if use_ambient
        else None
    )
    location = vertex_cfg.get("location") or (
        (
            env_map.get("GOOGLE_CLOUD_LOCATION")
            or os.environ.get("GOOGLE_CLOUD_LOCATION")
        )
        if use_ambient
        else None
    )
    headers = vertex_cfg.get("http_headers") or None
    endpoint = VertexEndpoint(
        project=project,
        location=location,
        base_url=base_url,
        http_headers=headers,
        api_key=api_key,
        options=options,
    )
    return ModelTarget(
        name=name,
        types=[ModelType.IMAGE] if is_image else [ModelType.TEXT],
        endpoint=endpoint,
    )


_DANGEROUS_ENV_KEYS_UPPER: frozenset[str] = frozenset(
    {
        "ANTIGRAVITY_HARNESS_PATH",
        "LD_PRELOAD",
        "LD_AUDIT",
        "LD_LIBRARY_PATH",
        "LD_CONFIG_FILE",
        "GLIBC_TUNABLES",
        "PYTHONSTARTUP",
        "PYTHONPATH",
        "PYTHONHOME",
        "PYTHONINSPECT",
        "BASH_ENV",
        "ENV",
        "PROMPT_COMMAND",
    }
)


def _validate_agent_env(raw_env: Any) -> None:
    """Defense-in-depth validation of `AgentConfig.env` matching Rust `validate_env_map`."""
    if raw_env is None:
        return
    if not isinstance(raw_env, dict):
        raise TypeError("AgentConfig.env must be a dictionary of string pairs")
    for raw_k, raw_v in raw_env.items():
        key = str(raw_k)
        val = str(raw_v)
        if not key:
            raise ValueError("AgentConfig.env cannot contain an empty variable name")
        if any(ch in key for ch in ("=", "\0", "\r", "\n")):
            raise ValueError(
                f"AgentConfig.env key {key!r} contains illegal delimiter or control character"
            )
        if "\0" in val:
            raise ValueError(
                f"AgentConfig.env value for key {key!r} contains illegal NUL byte"
            )
        upper_key = key.upper()
        if upper_key in _DANGEROUS_ENV_KEYS_UPPER or upper_key.startswith("DYLD_"):
            raise ValueError(
                f"AgentConfig.env key {key!r} is forbidden because it can hijack binary/linker/interpreter execution"
            )


def _build_python_agent_config(
    local_config: dict[str, Any], sdk_triggers: list[Any]
) -> Any:
    """Instantiate `LocalAgentConfig`, `LocalOpenAIAgentConfig`, or `LiteRTAgentConfig`."""
    image_shorthand = local_config.pop("_agy_image_model_shorthand", None)
    litert_cfg = local_config.pop("litert_config", None)
    if isinstance(litert_cfg, dict):
        from google.antigravity.connections.local.litert_connection_config import (
            LiteRTAgentConfig,
        )

        local_config.pop("model", None)
        local_config.pop("models", None)
        local_config.pop("api_key", None)
        local_config.pop("gemini_config", None)
        local_config.pop("vertex_config", None)
        local_config.pop("local_openai_config", None)
        lrt_kwargs = dict(litert_cfg)
        lrt_kwargs["model_path"] = str(lrt_kwargs["model_path"])
        if lrt_kwargs.get("cache_dir") is not None:
            lrt_kwargs["cache_dir"] = str(lrt_kwargs["cache_dir"])
        return LiteRTAgentConfig(triggers=sdk_triggers, **lrt_kwargs, **local_config)

    oai_cfg = local_config.pop("local_openai_config", None)
    if isinstance(oai_cfg, dict):
        from google.antigravity.connections.local.local_openai_connection_config import (
            LocalOpenAIAgentConfig,
        )

        fallback_model = local_config.pop("model", None)
        local_config.pop("models", None)
        local_config.pop("api_key", None)
        local_config.pop("gemini_config", None)
        local_config.pop("vertex_config", None)
        resolved_model = oai_cfg.get("model") or fallback_model
        return LocalOpenAIAgentConfig(
            model=resolved_model,
            base_url=oai_cfg.get("base_url"),
            triggers=sdk_triggers,
            **local_config,
        )

    vertex_cfg = local_config.pop("vertex_config", None)
    if isinstance(vertex_cfg, dict):
        from google.antigravity.connections.local.local_connection_config import (
            LocalAgentConfig,
        )

        fallback_model = str(local_config.pop("model", "") or "")
        top_key = local_config.pop("api_key", None)
        has_explicit_regional = bool(
            vertex_cfg.get("project") or vertex_cfg.get("location")
        )
        if top_key and not vertex_cfg.get("api_key") and not has_explicit_regional:
            vertex_cfg = {**vertex_cfg, "api_key": top_key}
        local_config.pop("gemini_config", None)
        raw_env = local_config.get("env")
        env_map: dict[str, str] = (
            {str(k): str(v) for k, v in raw_env.items()}
            if isinstance(raw_env, dict)
            else {}
        )
        models_dict = vertex_cfg.get("models") or {}
        t_default = _build_vertex_model_target(
            models_dict.get("default") or {},
            vertex_cfg,
            env_map,
            fallback_model,
            image_shorthand,
            is_image=False,
        )
        t_image = _build_vertex_model_target(
            models_dict.get("image_generation") or {},
            vertex_cfg,
            env_map,
            fallback_model,
            image_shorthand,
            is_image=True,
        )
        local_config["models"] = [t_default, t_image]
        return LocalAgentConfig(triggers=sdk_triggers, **local_config)

    from google.antigravity.connections.local.local_connection_config import (
        LocalAgentConfig,
    )

    return LocalAgentConfig(triggers=sdk_triggers, **local_config)
