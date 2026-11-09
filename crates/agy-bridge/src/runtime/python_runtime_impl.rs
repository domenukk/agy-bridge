//! `Runtime` trait implementation and config serialization for [`PythonRuntime`].

use tokio::sync::oneshot;

use super::{AgentId, BackendLogLevel, PyCommand, PythonRuntime};
use crate::error::Error;

/// Compute which SDK builtin tools are active based on the agent's
/// [`CapabilitiesConfig`](crate::config::CapabilitiesConfig).
///
/// - `enabled_tools: Some(list)` → only those tools are active.
/// - `disabled_tools: Some(list)` → all tools minus the disabled ones.
/// - Neither set → all builtin tools are active.
pub(super) fn compute_active_builtins(
    config: &crate::config::AgentConfig,
) -> Vec<crate::config::BuiltinTools> {
    crate::config::CapabilitiesConfig::resolve_active_builtins(config.capabilities.as_ref())
}

/// Validate `AgentConfig` options before dispatching `create_agent` to Python.
///
/// Rejects native-only options (`finish_on_tools`) and invalid per-server MCP
/// configurations (`enabled_tools` + `disabled_tools` or negative timeouts).
fn validate_python_create_config(config: &crate::config::AgentConfig) -> Result<(), Error> {
    if !config.finish_on_tools.is_empty() {
        return Err(Error::InvalidConfig {
            message: format!(
                "finish_on_tools ({:?}) is only supported by the native runtime; \
                 clear it when using the Python backend",
                config.finish_on_tools
            ),
        });
    }
    for mcp in &config.mcp_servers {
        mcp.validate()
            .map_err(|message| Error::InvalidConfig { message })?;
    }
    Ok(())
}

/// Return a clone of `server` with its canonical [`server_name`](crate::config::McpServer::server_name)
/// populated so the Python SDK uses the same name as the native runtime.
fn with_resolved_mcp_name(server: &crate::config::McpServer) -> crate::config::McpServer {
    let resolved_name = Some(server.server_name());
    let mut cloned = server.clone();
    match &mut cloned {
        crate::config::McpServer::Stdio(s) => s.name = resolved_name,
        crate::config::McpServer::Sse(s) => s.name = resolved_name,
        crate::config::McpServer::Http(s) => s.name = resolved_name,
    }
    cloned
}

/// Minimal `gemini_config` wire payload emitted when `AgentConfig::gemini` is `None`
/// and `effective_base_url()` resolves a custom endpoint.
#[derive(serde::Serialize)]
struct BaseUrlOnlyGeminiConfig {
    base_url: String,
}

/// Wire envelope passed to Python `create_agent`, enriching [`AgentConfig`](crate::config::AgentConfig)
/// without untyped JSON map mutation.
#[derive(serde::Serialize)]
struct PythonAgentConfigWire {
    #[serde(flatten)]
    config: crate::config::AgentConfig,
    #[serde(rename = "gemini_config", skip_serializing_if = "Option::is_none")]
    fallback_gemini_config: Option<BaseUrlOnlyGeminiConfig>,
    #[serde(rename = "_backend_log_level")]
    backend_log_level: &'static str,
}

fn serialize_python_agent_config(
    config: &crate::config::AgentConfig,
    backend_log_level: BackendLogLevel,
) -> Result<String, Error> {
    let mut patched = config.clone();
    patched.mcp_servers = config
        .mcp_servers
        .iter()
        .map(with_resolved_mcp_name)
        .collect();
    if let Some(api_key) = config.effective_api_key() {
        if patched.api_key.is_none() {
            patched.api_key = Some(api_key.clone());
        }
        if let Some(ref mut gemini) = patched.gemini
            && gemini.api_key.is_none()
        {
            gemini.api_key = Some(api_key);
        }
    }
    let fallback_gemini_config = if let Some(base_url) = config.effective_base_url() {
        if let Some(ref mut gemini) = patched.gemini {
            if gemini.base_url.is_none() {
                gemini.base_url = Some(base_url);
            }
            None
        } else {
            Some(BaseUrlOnlyGeminiConfig { base_url })
        }
    } else {
        None
    };

    let wire = PythonAgentConfigWire {
        config: patched,
        fallback_gemini_config,
        backend_log_level: backend_log_level.as_str(),
    };
    serde_json::to_string(&wire).map_err(|e| Error::BackendError {
        message: format!("Failed to serialize AgentConfig: {e}"),
    })
}

impl crate::agent::Runtime for PythonRuntime {
    async fn create_agent(
        &self,
        agent_id: u64,
        config: crate::config::AgentConfig,
    ) -> Result<(crate::agent::AgentId, Vec<crate::tools::AvailableTool>), Error> {
        validate_python_create_config(&config)?;

        // The local harness persists the trajectory to `save_dir` only if that
        // directory already exists; neither it nor the Python SDK creates it.
        // A missing directory means persistence is silently skipped, so a later
        // resume fails with "conversation not found". Since `save_dir` is an
        // explicit opt-in to persistence, create it here. On failure we log
        // rather than abort: the SDK treats a missing directory as an ephemeral
        // session, and a subsequent resume attempt would surface the problem
        // loudly on its own.
        if let Some(save_dir) = config.save_dir.as_ref()
            && let Err(e) = std::fs::create_dir_all(save_dir)
        {
            tracing::warn!(
                save_dir = %save_dir.display(),
                error = ?e,
                "Failed to create save_dir; conversation state may not persist \
                 and resume may fail with \"conversation not found\""
            );
        }

        // Serialize the AgentConfig and inject the runtime's backend log
        // level so the Python init script can configure logging without
        // needing a separate FFI parameter.
        let config_json = serialize_python_agent_config(&config, self.config.backend_log_level)?;

        // Collect the names of custom Rust tools so we can tag them correctly.
        let custom_tool_names: std::collections::HashSet<String> =
            config.tools.iter().map(|t| t.name.clone()).collect();

        let (raw_id, raw_tools) = self
            .send_command("create_agent", |reply| PyCommand::CreateAgent {
                agent_id,
                config_json,
                reply,
            })
            .await?;

        // Compute which builtins are active so we can tag and deduplicate them.
        let active_builtins = compute_active_builtins(&config);
        let builtin_names: std::collections::HashSet<&str> = active_builtins
            .iter()
            .map(crate::config::BuiltinTools::as_sdk_name)
            .collect();

        // Convert RawToolInfo → AvailableTool with source tags.
        // Python's ToolRunner includes builtins in its `tools` dict, so we
        // skip them here and add them back below with the Builtin tag.
        let mut available_tools: Vec<crate::tools::AvailableTool> = raw_tools
            .into_iter()
            .filter(|raw| !builtin_names.contains(raw.name.as_str()))
            .map(|raw| {
                let source = if custom_tool_names.contains(&raw.name) {
                    crate::tools::ToolSource::Custom
                } else {
                    crate::tools::ToolSource::Mcp
                };
                crate::tools::AvailableTool {
                    name: raw.name,
                    description: raw.description,
                    parameter_schema: raw.parameter_schema,
                    source,
                }
            })
            .collect();

        // Add builtin tools with their known descriptions.
        for builtin in active_builtins {
            available_tools.push(crate::tools::AvailableTool {
                name: builtin.as_sdk_name().to_owned(),
                description: builtin.description().to_owned(),
                parameter_schema: serde_json::Value::Null,
                source: crate::tools::ToolSource::Builtin,
            });
        }

        tracing::info!(
            agent_id = raw_id.0,
            tool_count = available_tools.len(),
            tools = ?available_tools.iter().map(|t| format!("{t}")).collect::<Vec<_>>(),
            "Agent created with available tools"
        );

        Ok((raw_id.0, available_tools))
    }

    async fn chat(
        &self,
        agent_id: crate::agent::AgentId,
        content: &crate::content::Content,
    ) -> Result<crate::streaming::ChatResponseHandle, Error> {
        let prompt = match content {
            crate::content::Content::Text { text } => text.clone(),
            other => crate::content::content_to_json(other)?,
        };
        self.send_command("chat", |reply| PyCommand::Chat {
            agent_id: AgentId(agent_id),
            prompt,
            reply,
        })
        .await
    }

    async fn shutdown_agent(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("shutdown_agent", |reply| PyCommand::ShutdownAgent {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    fn try_shutdown_agent(&self, agent_id: crate::agent::AgentId) {
        // Fire-and-forget: create a oneshot whose receiver we drop immediately.
        // The Python thread will still process the shutdown; we just don't wait
        // for the result.
        if let Some(ref tx) = self.cmd_tx {
            let (reply, _) = oneshot::channel();
            if let Err(e) = tx.try_send(PyCommand::ShutdownAgent {
                agent_id: AgentId(agent_id),
                reply,
            }) {
                tracing::debug!(
                    agent_id = agent_id,
                    error = %e,
                    "try_shutdown_agent: channel send failed (runtime may already be gone)"
                );
            }
        }
    }

    async fn cancel(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("cancel", |reply| PyCommand::Cancel {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn wait_for_idle(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("wait_for_idle", |reply| PyCommand::WaitForIdle {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn send(
        &self,
        agent_id: crate::agent::AgentId,
        content: &crate::content::Content,
    ) -> Result<(), Error> {
        let prompt = match content {
            crate::content::Content::Text { text } => text.clone(),
            other => crate::content::content_to_json(other)?,
        };
        self.send_command("send", |reply| PyCommand::Send {
            agent_id: AgentId(agent_id),
            prompt,
            reply,
        })
        .await
    }

    async fn signal_idle(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("signal_idle", |reply| PyCommand::SignalIdle {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn wait_for_wakeup(
        &self,
        agent_id: crate::agent::AgentId,
        timeout: std::time::Duration,
    ) -> Result<bool, Error> {
        self.send_command("wait_for_wakeup", |reply| PyCommand::WaitForWakeup {
            agent_id: AgentId(agent_id),
            timeout_secs: timeout.as_secs_f64(),
            reply,
        })
        .await
    }

    async fn history(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<Vec<crate::types::ConversationMessage>, Error> {
        self.send_command("get_history", |reply| PyCommand::GetHistory {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn turn_count(&self, agent_id: crate::agent::AgentId) -> Result<u32, Error> {
        self.send_command("get_turn_count", |reply| PyCommand::GetTurnCount {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn total_usage(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<crate::types::UsageMetadata, Error> {
        self.send_command("get_total_usage", |reply| PyCommand::GetTotalUsage {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn last_turn_usage(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<crate::types::UsageMetadata, Error> {
        self.send_command("get_last_turn_usage", |reply| PyCommand::GetLastTurnUsage {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn clear_history(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("clear_history", |reply| PyCommand::ClearHistory {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn compaction_indices(&self, agent_id: crate::agent::AgentId) -> Result<Vec<u32>, Error> {
        self.send_command("compaction_indices", |reply| {
            PyCommand::GetCompactionIndices {
                agent_id: AgentId(agent_id),
                reply,
            }
        })
        .await
    }

    async fn last_response(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<Option<String>, Error> {
        self.send_command("last_response", |reply| PyCommand::GetLastResponse {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn delete(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("delete", |reply| PyCommand::Delete {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn disconnect(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("disconnect", |reply| PyCommand::Disconnect {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn is_idle(&self, agent_id: crate::agent::AgentId) -> Result<bool, Error> {
        self.send_command("is_idle", |reply| PyCommand::IsIdle {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn sandbox_status(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<Option<crate::types::SandboxStatus>, Error> {
        self.send_command("sandbox_status", |reply| PyCommand::GetSandboxStatus {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }
}
