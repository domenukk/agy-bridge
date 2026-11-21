//! Shared Single-Source-of-Truth validators for environment maps, RFC-9110 HTTP
//! headers, endpoint URLs, and structured [`SystemInstructions`](super::SystemInstructions).

use std::collections::HashMap;

/// Environment variable keys that can hijack binary discovery, dynamic linking,
/// or interpreter startup if injected into `localharness` or Python subprocesses.
const DANGEROUS_ENV_KEYS_EXACT: &[&str] = &[
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
];

const DYLD_PREFIX: &str = "DYLD_";

/// Return `true` when `key` matches a dangerous binary, dynamic-linker, or
/// interpreter hijack variable (ASCII case-insensitive).
#[must_use]
pub(crate) fn is_dangerous_env_key(key: &str) -> bool {
    if key.len() >= DYLD_PREFIX.len()
        && key.as_bytes()[..DYLD_PREFIX.len()].eq_ignore_ascii_case(DYLD_PREFIX.as_bytes())
    {
        return true;
    }
    DANGEROUS_ENV_KEYS_EXACT
        .iter()
        .any(|blocked| key.eq_ignore_ascii_case(blocked))
}

/// Validate a single `(key, value)` environment variable entry.
pub(crate) fn validate_env_entry(key: &str, value: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("AgentConfig.env cannot contain an empty variable name".to_owned());
    }
    if key.contains(['=', '\0', '\r', '\n']) {
        return Err(format!(
            "AgentConfig.env key {key:?} contains illegal delimiter or control character ('=', NUL, CR, or LF)"
        ));
    }
    if value.contains('\0') {
        return Err(format!(
            "AgentConfig.env value for key '{key}' contains illegal NUL byte"
        ));
    }
    if is_dangerous_env_key(key) {
        return Err(format!(
            "AgentConfig.env key '{key}' is forbidden because it can hijack binary/linker/interpreter execution"
        ));
    }
    Ok(())
}

/// Validate all entries in an `AgentConfig.env` map.
pub(crate) fn validate_env_map(env: &HashMap<String, String>) -> Result<(), String> {
    for (key, value) in env {
        validate_env_entry(key, value)?;
    }
    Ok(())
}

/// RFC 9110 Section 5.6.2 token character (`tchar`).
const fn is_rfc9110_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Validate an HTTP header map against RFC 9110 header field-name (`tchar`) and
/// field-value control-character rules (rejecting CR/LF/NUL/C0 CTLs except HTAB, and DEL).
pub(crate) fn validate_http_headers(
    headers: &HashMap<String, String>,
    context: &str,
) -> Result<(), String> {
    for (name, value) in headers {
        if name.is_empty() {
            return Err(format!(
                "{context} cannot contain an empty HTTP header name"
            ));
        }
        if !name.bytes().all(is_rfc9110_tchar) {
            return Err(format!(
                "{context} header name {name:?} contains characters outside RFC 9110 tchar"
            ));
        }
        if value.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7F) {
            return Err(format!(
                "{context} header '{name}' value contains illegal control character (CR, LF, NUL, DEL, or C0 CTL)"
            ));
        }
    }
    Ok(())
}

/// Validate an endpoint or MCP HTTP URL against empty strings (unless `allow_empty`)
/// and C0 control characters / DEL (`\r`, `\n`, `\0`, `\t`, `0x00..=0x1F`, `0x7F`).
pub(crate) fn validate_http_url(url: &str, context: &str, allow_empty: bool) -> Result<(), String> {
    if url.is_empty() {
        if allow_empty {
            return Ok(());
        }
        return Err(format!("{context} requires a non-empty URL"));
    }
    if !allow_empty && url.trim().is_empty() {
        return Err(format!("{context} requires a non-empty URL"));
    }
    if url.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return Err(format!(
            "{context} URL contains illegal control character (CR, LF, NUL, C0 CTL, or DEL)"
        ));
    }
    Ok(())
}

fn contains_closing_tag_breakout(content: &str, title: &str) -> bool {
    let lower_content = content.to_lowercase();
    for candidate in [title.to_lowercase(), title.trim().to_lowercase()] {
        if candidate.is_empty() {
            continue;
        }
        let direct = format!("</{candidate}");
        if lower_content.contains(&direct) {
            return true;
        }
        let mut rest = lower_content.as_str();
        while let Some(pos) = rest.find('<') {
            let after_lt = rest[pos + 1..].trim_start_matches([' ', '\t']);
            if let Some(after_slash) = after_lt.strip_prefix('/') {
                let trimmed = after_slash.trim_start_matches([' ', '\t']);
                if trimmed.starts_with(&candidate) {
                    return true;
                }
            }
            rest = &rest[pos + 1..];
        }
    }
    false
}

fn validate_instruction_section(
    title: &str,
    content: &str,
    location_label: &str,
) -> Result<(), String> {
    if content.contains('\0') {
        return Err(format!("{location_label} content cannot contain NUL bytes"));
    }
    if title.trim().is_empty() {
        return Err(format!(
            "{location_label} title cannot be empty or whitespace-only"
        ));
    }
    if title
        .bytes()
        .any(|b| b < 0x20 || b == 0x7F || matches!(b, b'<' | b'>' | b'/'))
    {
        return Err(format!(
            "{location_label} title {title:?} cannot contain '<', '>', '/', or control characters"
        ));
    }
    if contains_closing_tag_breakout(content, title) {
        return Err(format!(
            "{location_label} content cannot contain closing tag '</{title}' matching its section title"
        ));
    }
    Ok(())
}

/// Validate [`SystemInstructions`](super::SystemInstructions) against NUL bytes,
/// malformed section titles, and `</{title}>` XML section breakout sequences.
pub(crate) fn validate_system_instructions(
    instructions: &super::SystemInstructions,
) -> Result<(), String> {
    match instructions {
        super::SystemInstructions::Custom(text) => {
            if text.contains('\0') {
                return Err("SystemInstructions::Custom text cannot contain NUL bytes".to_owned());
            }
        }
        super::SystemInstructions::TitledCustom { text, title } => {
            if text.contains('\0') {
                return Err(
                    "SystemInstructions::TitledCustom text cannot contain NUL bytes".to_owned(),
                );
            }
            if let Some(t) = title {
                validate_instruction_section(t, text, "SystemInstructions::TitledCustom")?;
            }
        }
        super::SystemInstructions::Templated { identity, sections } => {
            if let Some(id) = identity
                && id.contains('\0')
            {
                return Err(
                    "SystemInstructions::Templated identity cannot contain NUL bytes".to_owned(),
                );
            }
            for section in sections {
                validate_instruction_section(
                    &section.title,
                    &section.content,
                    "SystemInstructionSection",
                )?;
            }
        }
    }
    Ok(())
}
