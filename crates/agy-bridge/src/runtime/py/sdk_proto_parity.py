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
FIELD_DISABLE_DEFINE_SUBAGENT_TOOL = "disable_define_subagent_tool"
FIELD_TITLE = "title"
FIELD_INCLUDE_LAST_USER_MESSAGE = "include_last_user_message"
ATTR_DISABLE_DEFINE_SUBAGENT_TOOL = "_agy_disable_define_subagent_tool"
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
            if not is_subagent and getattr(
                cfg, ATTR_DISABLE_DEFINE_SUBAGENT_TOOL, False
            ):
                setattr(proto.subagents, FIELD_DISABLE_DEFINE_SUBAGENT_TOOL, True)
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
                if title:
                    setattr(proto.custom.part[0], FIELD_TITLE, str(title))
                return proto

            local_conn_mod.to_system_instructions_proto = _patched_si
        local_conn_mod._agy_compaction_proto_patched = True
        logger.info("[MONKEYPATCH] Enabled full CompactionConfig proto forwarding")
        return True
