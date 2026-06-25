//! Python scripts for the agy-bridge runtime.
//!
//! These scripts are executed inside the dedicated Python thread (see
//! `runtime/mod.rs`) and bridge the gap between the Rust runtime and the
//! `google.antigravity` Python SDK.
//!
//! ## Why inline Python instead of pure `PyO3`?
//!
//! The project uses `PyO3` (`pyo3 = "0.22"`) extensively for the Rust↔Python
//! boundary (dispatching tool calls, hook callbacks, etc.). However, these
//! scripts remain in Python because they interact with SDK-specific APIs
//! that have no Rust equivalents:
//!
//! - **SDK type construction**: `types.Image(data=..., mime_type=...)`,
//!   `LocalAgentConfig(...)`, policy/hook decorators — all Python classes.
//! - **Async protocol**: `agent.chat()`, `conv.send()`, `conv.cancel()` are
//!   Python async methods. `PyO3` can convert the resulting coroutines to Rust
//!   futures (via `pyo3_async_runtimes`), but the call site must be Python.
//! - **Dynamic serialization**: `model_dump_json()`, `getattr` chains with
//!   fallbacks — idiomatic Python patterns that would be verbose in Rust.
//!
//! Frequently-used helpers are compiled once and cached via `OnceLock`
//! (see `command_loop::get_or_compile_py_helper`). The larger init script
//! lives in `py/agent_init.py` and is embedded via `include_str!`.

pub const PYTHON_AGENT_INIT_SCRIPT: &str = include_str!("py/agent_init.py");

/// Shared helper for decoding multimodal content from JSON.
///
/// Both `_start_chat` and `_send` need to decode base64-encoded
/// `Image`/`Document`/`Audio`/`Video` payloads into SDK types.
const PYTHON_DECODE_CONTENT: &str = r#"
def _decode_content(data):
    import base64
    from google.antigravity import types
    if isinstance(data, str):
        return data
    elif isinstance(data, list):
        return [_decode_content(item) for item in data]
    elif isinstance(data, dict):
        typ = data.get("type")
        if typ in ("Image", "Document", "Audio", "Video"):
            raw_bytes = base64.b64decode(data.get("data", ""))
            kwargs = {"data": raw_bytes, "mime_type": data.get("mime_type", "")}
            if "description" in data:
                kwargs["description"] = data["description"]

            if typ == "Image":
                return types.Image(**kwargs)
            elif typ == "Document":
                return types.Document(**kwargs)
            elif typ == "Audio":
                return types.Audio(**kwargs)
            elif typ == "Video":
                return types.Video(**kwargs)
    return data

def _decode_prompt(prompt):
    import json
    try:
        parsed = json.loads(prompt)
    except json.JSONDecodeError:
        parsed = prompt
    return _decode_content(parsed)
"#;

/// Concatenated script: shared decode helper + `_start_chat`.
pub static PYTHON_CHAT_START_SCRIPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "{PYTHON_DECODE_CONTENT}\n{}",
        r#"
async def _start_chat(agent, prompt, timeout_secs):
    """Start chat() and return the response object.

    Retry/backoff is handled by the Rust layer (QuotaState + agent/mod.rs).
    This function attempts once and lets exceptions propagate.
    """
    import asyncio
    decoded = _decode_prompt(prompt)
    response = await asyncio.wait_for(agent.chat(decoded), timeout=timeout_secs)
    return response
"#
    )
});

pub const PYTHON_NEXT_STEP_SCRIPT: &str = r#"
async def _next_step(aiter, timeout_secs):
    """Wait for the next step from the async iterator."""
    import asyncio
    try:
        step = await asyncio.wait_for(aiter.__anext__(), timeout=timeout_secs)
        # Ensure we return a JSON string representation to Rust
        if hasattr(step, "model_dump_json"):
            return step.model_dump_json()
        return None
    except StopAsyncIteration:
        raise
"#;

pub const PYTHON_CANCEL_SCRIPT: &str = r"
async def _cancel(agent):
    if hasattr(agent, 'conversation') and agent.conversation:
        await agent.conversation.cancel()
";

pub const PYTHON_WAIT_FOR_IDLE_SCRIPT: &str = r"
async def _wait_for_idle(agent):
    if hasattr(agent, 'conversation') and agent.conversation:
        await agent.conversation.wait_for_idle()
";

pub const PYTHON_GET_HISTORY_SCRIPT: &str = r#"
def _get_history(agent):
    """Extract conversation history as a list of {role, content} dicts."""
    import json
    messages = []
    conv = getattr(agent, 'conversation', None)
    if conv is None:
        return json.dumps(messages)
    history = getattr(conv, 'history', None)
    if history is None:
        return json.dumps(messages)
    for msg in history:
        source = getattr(msg, 'source', None)
        if source is not None:
            if hasattr(source, 'value'):
                role = str(source.value).lower()
            elif hasattr(source, 'name'):
                role = str(source.name).lower()
            else:
                role = str(source).lower()
        else:
            role = 'unknown'
        content = getattr(msg, 'content', '') or ''
        messages.append({'role': role, 'content': content})
    return json.dumps(messages)
"#;

pub const PYTHON_GET_TURN_COUNT_SCRIPT: &str = r"
def _get_turn_count(agent):
    conv = getattr(agent, 'conversation', None)
    if conv is None:
        return 0
    tc = getattr(conv, 'turn_count', None)
    if tc is None:
        return 0
    return int(tc)
";

pub const PYTHON_GET_TOTAL_USAGE_SCRIPT: &str = r#"
def _get_total_usage(agent):
    """Return cumulative usage as a JSON dict."""
    import json
    conv = getattr(agent, 'conversation', None)
    if conv is None:
        return json.dumps({})
    usage = getattr(conv, 'total_usage', None)
    if usage is None:
        return json.dumps({})
    return json.dumps({
        'prompt_token_count': getattr(usage, 'prompt_token_count', None),
        'cached_content_token_count': getattr(usage, 'cached_content_token_count', None),
        'candidates_token_count': getattr(usage, 'candidates_token_count', None),
        'thoughts_token_count': getattr(usage, 'thoughts_token_count', None),
        'total_token_count': getattr(usage, 'total_token_count', None),
    })
"#;

pub const PYTHON_GET_LAST_TURN_USAGE_SCRIPT: &str = r#"
def _get_last_turn_usage(agent):
    """Return usage from the most recent turn as a JSON dict."""
    import json
    conv = getattr(agent, 'conversation', None)
    if conv is None:
        return json.dumps({})
    usage = getattr(conv, 'last_turn_usage', None)
    if usage is None:
        return json.dumps({})
    return json.dumps({
        'prompt_token_count': getattr(usage, 'prompt_token_count', None),
        'cached_content_token_count': getattr(usage, 'cached_content_token_count', None),
        'candidates_token_count': getattr(usage, 'candidates_token_count', None),
        'thoughts_token_count': getattr(usage, 'thoughts_token_count', None),
        'total_token_count': getattr(usage, 'total_token_count', None),
    })
"#;

pub const PYTHON_CLEAR_HISTORY_SCRIPT: &str = r"
async def _clear_history(agent):
    conv = getattr(agent, 'conversation', None)
    if conv is not None and hasattr(conv, 'clear_history'):
        conv.clear_history()
";

pub static PYTHON_SEND_SCRIPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "{PYTHON_DECODE_CONTENT}\n{}",
        r"
async def _send(agent, prompt):
    decoded = _decode_prompt(prompt)
    conv = getattr(agent, 'conversation', None)
    if conv is None:
        raise RuntimeError('agent.conversation is not available; cannot send')
    await conv.send(decoded)
"
    )
});

pub const PYTHON_SIGNAL_IDLE_SCRIPT: &str = r"
async def _signal_idle(agent):
    conv = getattr(agent, 'conversation', None)
    if conv is None:
        raise RuntimeError('agent.conversation is not available; cannot signal idle')
    await conv.signal_idle()
";

pub const PYTHON_WAIT_FOR_WAKEUP_SCRIPT: &str = r"
async def _wait_for_wakeup(agent, timeout_secs):
    conv = getattr(agent, 'conversation', None)
    if conv is None:
        raise RuntimeError('agent.conversation is not available; cannot wait for wakeup')
    return await conv.wait_for_wakeup(timeout=timeout_secs)
";

/// Extracts `usage_metadata` and `structured_output` from a Python response
/// object, serialising each to JSON (or `None` if absent/unconvertible).
pub const PYTHON_EXTRACT_METADATA_SCRIPT: &str = r"
def _extract(response, agent):
    import json
    import logging
    _log = logging.getLogger('agy_bridge.extract_metadata')
    u = getattr(response, 'usage_metadata', None)
    if u is None and agent is not None:
        conv = getattr(agent, 'conversation', None)
        if conv is not None:
            u = getattr(conv, 'last_turn_usage', None)
    s = getattr(response, 'structured_output', None)
    u_json = None
    s_json = None
    if u is not None:
        if hasattr(u, 'model_dump_json'):
            u_json = u.model_dump_json()
        elif hasattr(u, 'to_json'):
            val = u.to_json()
            u_json = val if isinstance(val, str) else json.dumps(val)
        elif hasattr(u, '__dict__'):
            u_json = json.dumps(u.__dict__)
        else:
            try:
                u_json = json.dumps(dict(u))
            except Exception:
                _log.debug('Failed to serialize usage_metadata', exc_info=True)
    if s is not None:
        if hasattr(s, 'model_dump_json'):
            s_json = s.model_dump_json()
        elif hasattr(s, 'to_json'):
            val = s.to_json()
            s_json = val if isinstance(val, str) else json.dumps(val)
        elif isinstance(s, dict):
            s_json = json.dumps(s)
        else:
            try:
                s_json = json.dumps(dict(s))
            except Exception:
                _log.debug('Failed to serialize structured_output', exc_info=True)
    return (u_json, s_json)
";

pub const PYTHON_DELETE_SCRIPT: &str = r"
async def _delete(agent):
    conv = getattr(agent, 'conversation', None)
    if conv is not None and hasattr(conv, 'delete'):
        await conv.delete()
";

pub const PYTHON_DISCONNECT_SCRIPT: &str = r"
async def _disconnect(agent):
    conv = getattr(agent, 'conversation', None)
    if conv is not None and hasattr(conv, 'disconnect'):
        await conv.disconnect()
";

pub const PYTHON_IS_IDLE_SCRIPT: &str = r"
def _is_idle(agent):
    conv = getattr(agent, 'conversation', None)
    if conv is None:
        return True
    return bool(getattr(conv, 'is_idle', True))
";
