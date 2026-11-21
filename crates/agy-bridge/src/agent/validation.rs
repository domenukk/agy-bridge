//! Structural and cross-field validation for [`AgentConfig`](crate::config::AgentConfig).

use std::{collections::HashSet, sync::Arc};

use crate::{
    config::{AgentConfig, BuiltinTools},
    error::Error,
};

/// Freeze `registry` with its output floor raised to at least `floor`.
///
/// Never lowers a stricter floor the caller already set on the registry.
pub(crate) fn floored_registry(
    mut registry: crate::tools::ToolRegistry,
    floor: crate::tools::OutputSanitization,
) -> Arc<crate::tools::ToolRegistry> {
    let effective = registry.output_floor().at_least(floor);
    registry.set_output_floor(effective);
    Arc::new(registry)
}

fn validate_subagents_and_skills(config: &AgentConfig) -> Result<(), Error> {
    if !config.skills.is_empty() && !config.inline_skills.is_empty() {
        return Err(Error::InvalidConfig {
            message: "combining inline_skills and skills_paths is not supported by LocalHarness; \
                      provide either inline_skills or skills, not both"
                .to_owned(),
        });
    }
    if let Some(ref dir) = config.app_data_dir
        && !dir.is_absolute()
    {
        return Err(Error::InvalidConfig {
            message: format!(
                "app_data_dir must be an absolute path, got '{}'",
                dir.display()
            ),
        });
    }

    let declared_names: HashSet<&str> = config
        .subagents
        .iter()
        .map(|s| s.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();

    if let Some(ref caps) = config.capabilities
        && let Some(ref allowed) = caps.allowed_subagents
    {
        let subagents_disabled = !caps.enable_subagents
            || caps
                .enabled_tools
                .as_ref()
                .is_some_and(|t| !t.contains(&BuiltinTools::InvokeSubagent))
            || caps
                .disabled_tools
                .as_ref()
                .is_some_and(|t| t.contains(&BuiltinTools::InvokeSubagent));
        if subagents_disabled {
            return Err(Error::InvalidConfig {
                message: "allowed_subagents cannot be specified when subagents are disabled"
                    .to_owned(),
            });
        }
        let mut unknown: Vec<&str> = allowed
            .iter()
            .map(String::as_str)
            .filter(|n| !declared_names.contains(n))
            .collect();
        if !unknown.is_empty() {
            unknown.sort_unstable();
            return Err(Error::InvalidConfig {
                message: format!(
                    "Unknown subagent name(s) in CapabilitiesConfig.allowed_subagents: {unknown:?}"
                ),
            });
        }
    }

    for sub in &config.subagents {
        if let Some(ref sc) = sub.capabilities {
            if sc.enabled_tools.is_some() && sc.disabled_tools.is_some() {
                return Err(Error::InvalidConfig {
                    message: format!(
                        "SubagentConfig('{}'): enabled_tools and disabled_tools are mutually exclusive",
                        sub.name
                    ),
                });
            }
            if let Some(ref allowed) = sc.allowed_subagents {
                let subagent_disabled = sc
                    .enabled_tools
                    .as_ref()
                    .is_some_and(|t| !t.contains(&BuiltinTools::InvokeSubagent))
                    || sc
                        .disabled_tools
                        .as_ref()
                        .is_some_and(|t| t.contains(&BuiltinTools::InvokeSubagent));
                if subagent_disabled {
                    return Err(Error::InvalidConfig {
                        message: format!(
                            "SubagentConfig('{}'): allowed_subagents cannot be specified when INVOKE_SUBAGENT is disabled",
                            sub.name
                        ),
                    });
                }
                let mut unknown: Vec<&str> = allowed
                    .iter()
                    .map(String::as_str)
                    .filter(|n| !declared_names.contains(n))
                    .collect();
                if !unknown.is_empty() {
                    unknown.sort_unstable();
                    return Err(Error::InvalidConfig {
                        message: format!(
                            "Unknown subagent name(s) in SubagentConfig('{}').capabilities.allowed_subagents: {unknown:?}",
                            sub.name
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Validate [`AgentConfig`] capabilities, skills, subagents, response schema,
/// MCP servers, and backend endpoint configuration before creating a session.
///
/// # Errors
///
/// Returns [`Error::InvalidConfig`] if any configuration invariant is violated.
pub fn validate_create_config(config: &AgentConfig) -> Result<(), Error> {
    crate::config::validation::validate_env_map(&config.env)
        .map_err(|message| Error::InvalidConfig { message })?;
    if let Some(ref instr) = config.system_instructions {
        crate::config::validation::validate_system_instructions(instr)
            .map_err(|message| Error::InvalidConfig { message })?;
    }
    if let Some(ref caps) = config.capabilities {
        caps.validate().map_err(|msg| Error::InvalidConfig {
            message: msg.to_string(),
        })?;
    }
    validate_subagents_and_skills(config)?;
    if let Some(ref schema) = config.response_schema {
        schema.validate().map_err(|msg| Error::InvalidConfig {
            message: msg.to_string(),
        })?;
    }
    for mcp in &config.mcp_servers {
        mcp.validate()
            .map_err(|message| Error::InvalidConfig { message })?;
    }

    let backend_count = usize::from(config.gemini.is_some())
        + usize::from(config.vertex.is_some())
        + usize::from(config.local_openai.is_some())
        + usize::from(config.litert.is_some());
    if backend_count > 1 {
        return Err(Error::InvalidConfig {
            message: "At most one model backend (gemini, vertex, local_openai, or litert) may be configured on AgentConfig"
                .to_owned(),
        });
    }
    if let Some(ref g) = config.gemini {
        g.validate()
            .map_err(|message| Error::InvalidConfig { message })?;
    }
    if let Some(ref v) = config.vertex {
        v.validate_with_fallback(&config.env, config.api_key.as_deref())
            .map_err(|message| Error::InvalidConfig { message })?;
    }
    if let Some(ref oai) = config.local_openai {
        oai.validate()
            .map_err(|message| Error::InvalidConfig { message })?;
    }
    if let Some(ref lrt) = config.litert {
        lrt.validate()
            .map_err(|message| Error::InvalidConfig { message })?;
    }
    Ok(())
}

/// Propagate `WorkspaceOnly` paths into `config.workspaces` and, when restrictive
/// rules (`DenyAll` or explicit `Allow`/`Deny`/`AskUser`) are present without `AllowAll`,
/// auto-allow non-destructive custom tools that do not already have an explicit policy rule.
pub(crate) fn reconcile_workspace_only_policies(config: &mut AgentConfig) {
    let mut has_workspace_only = false;
    for rule in &config.policies {
        if let crate::policies::PolicyRule::WorkspaceOnly(paths) = rule {
            has_workspace_only = true;
            for p in paths {
                if !config.workspaces.contains(p) {
                    config.workspaces.push(p.clone());
                }
            }
        }
    }
    if !has_workspace_only || config.tools.is_empty() {
        return;
    }
    let has_allow_all = config
        .policies
        .iter()
        .any(|r| matches!(r, crate::policies::PolicyRule::AllowAll));
    if has_allow_all {
        return;
    }
    let has_explicit_restrictive_or_allow_rules = config.policies.iter().any(|r| {
        matches!(
            r,
            crate::policies::PolicyRule::DenyAll
                | crate::policies::PolicyRule::Allow(_)
                | crate::policies::PolicyRule::Deny(_)
                | crate::policies::PolicyRule::AskUser { .. }
        )
    });
    if !has_explicit_restrictive_or_allow_rules {
        return;
    }
    let mut to_allow = Vec::new();
    for def in &config.tools {
        if def.effect.is_destructive() {
            continue;
        }
        let explicitly_configured = config.policies.iter().any(|r| match r {
            crate::policies::PolicyRule::Allow(t)
            | crate::policies::PolicyRule::Deny(t)
            | crate::policies::PolicyRule::AskUser { tool: t, .. } => t == &def.name,
            _ => false,
        });
        if !explicitly_configured {
            to_allow.push(crate::policies::PolicyRule::allow(def.name.clone()));
        }
    }
    if to_allow.is_empty() {
        return;
    }
    let insert_pos = config
        .policies
        .iter()
        .position(|r| matches!(r, crate::policies::PolicyRule::DenyAll))
        .unwrap_or(config.policies.len());
    config.policies.splice(insert_pos..insert_pos, to_allow);
}

/// Reject tool configurations that would silently not do what they say.
///
/// # Errors
///
/// Returns [`Error::InvalidConfig`] if `finish_on_tools` names an unregistered
/// tool or the `ToolRegistry` output sanitization floor is weaker than
/// `config.tool_output_floor`.
pub fn validate_tool_config(
    config: &AgentConfig,
    registry: Option<&crate::tools::ToolRegistry>,
) -> Result<(), Error> {
    if let Some(reg) = registry {
        let floor = reg.output_floor();
        if floor.at_least(config.tool_output_floor) != floor {
            return Err(Error::InvalidConfig {
                message: format!(
                    "ToolRegistry output floor {floor:?} is weaker than \
                     AgentConfig.tool_output_floor {:?}; build agents via \
                     AgyBridge::agent(..).tools(..) or call ToolRegistry::set_output_floor",
                    config.tool_output_floor
                ),
            });
        }
    }
    let unknown: Vec<&str> = config
        .finish_on_tools
        .iter()
        .map(String::as_str)
        .filter(|name| !registry.is_some_and(|reg| reg.contains(name)))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let registered: Vec<&str> = match registry {
        Some(reg) => reg.iter().map(|(name, _)| name).collect(),
        None => Vec::new(),
    };
    Err(Error::InvalidConfig {
        message: format!(
            "finish_on_tools names unregistered tools {unknown:?}; registered custom tools: \
             {registered:?}"
        ),
    })
}
