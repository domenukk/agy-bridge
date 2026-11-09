//! MCP (Model Context Protocol) server configuration types.

use serde::{Deserialize, Serialize};

use super::{default_mcp_sse_read_timeout, default_mcp_timeout, default_true};

/// Configuration for an MCP server connected via stdio.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpStdioServer {
    /// Server name used by the harness (e.g. `ServerName` in `call_mcp_tool`).
    ///
    /// Defaults to a name derived from the command / URL; see [`McpServer::server_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The command to run to start the server.
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Explicit allowlist of MCP tool names exposed from this server.
    ///
    /// Mutually exclusive with [`disabled_tools`](Self::disabled_tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<String>>,
    /// Explicit denylist of MCP tool names hidden from this server.
    ///
    /// Mutually exclusive with [`enabled_tools`](Self::enabled_tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
    /// Optional timeout in seconds for connecting to the server and listing tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<i32>,
    /// When `true`, forces all tools from this server to be loaded eagerly into
    /// the model's prompt rather than lazily via tool search.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_all_tools_eager: bool,
}

/// Configuration for an MCP server connected via SSE.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpSseServer {
    /// Server name used by the harness (e.g. `ServerName` in `call_mcp_tool`).
    ///
    /// Defaults to a name derived from the command / URL; see [`McpServer::server_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The URL of the SSE endpoint.
    pub url: String,
    /// Optional headers to send with the connection request.
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,
    /// Explicit allowlist of MCP tool names exposed from this server.
    ///
    /// Mutually exclusive with [`disabled_tools`](Self::disabled_tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<String>>,
    /// Explicit denylist of MCP tool names hidden from this server.
    ///
    /// Mutually exclusive with [`enabled_tools`](Self::enabled_tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
    /// Optional timeout in seconds for connecting to the server and listing tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<i32>,
    /// When `true`, forces all tools from this server to be loaded eagerly into
    /// the model's prompt rather than lazily via tool search.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_all_tools_eager: bool,
}

/// Configuration for an MCP server connected via Streamable HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStreamableHttpServer {
    /// Server name used by the harness (e.g. `ServerName` in `call_mcp_tool`).
    ///
    /// Defaults to a name derived from the command / URL; see [`McpServer::server_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The URL of the HTTP endpoint.
    pub url: String,
    /// Optional headers to send with the connection request.
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,
    /// Connection timeout in seconds.
    #[serde(default = "default_mcp_timeout")]
    pub timeout: f64,
    /// SSE read timeout in seconds.
    #[serde(default = "default_mcp_sse_read_timeout")]
    pub sse_read_timeout: f64,
    /// Whether to terminate the connection on close.
    #[serde(default = "default_true")]
    pub terminate_on_close: bool,
    /// Explicit allowlist of MCP tool names exposed from this server.
    ///
    /// Mutually exclusive with [`disabled_tools`](Self::disabled_tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<String>>,
    /// Explicit denylist of MCP tool names hidden from this server.
    ///
    /// Mutually exclusive with [`enabled_tools`](Self::enabled_tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
    /// Optional timeout in seconds for connecting to the server and listing tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<i32>,
    /// When `true`, forces all tools from this server to be loaded eagerly into
    /// the model's prompt rather than lazily via tool search.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_all_tools_eager: bool,
}

impl Default for McpStreamableHttpServer {
    fn default() -> Self {
        Self::new(String::new())
    }
}

/// An MCP server, identified by its transport.
///
/// All MCP transports speak JSON-RPC 2.0; the variants describe *how* the
/// client connects to the server process.
///
/// Use the convenience constructors [`McpServer::stdio`], [`McpServer::sse`],
/// and [`McpServer::http`] to avoid importing the inner transport types.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpServer {
    #[serde(rename = "stdio")]
    Stdio(McpStdioServer),
    #[serde(rename = "sse")]
    Sse(McpSseServer),
    #[serde(rename = "http")]
    Http(McpStreamableHttpServer),
}

/// Fallback server name when nothing usable can be derived.
const DEFAULT_MCP_SERVER_NAME: &str = "mcp_server";

/// Replace every character outside `[A-Za-z0-9_-]` with `_`.
fn sanitize_server_name(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        DEFAULT_MCP_SERVER_NAME.to_owned()
    } else {
        sanitized
    }
}

/// Derive a name from a command path: its file stem (`/usr/bin/python3` -> `python3`).
fn name_from_command(command: &str) -> String {
    let stem = match std::path::Path::new(command)
        .file_stem()
        .and_then(|s| s.to_str())
    {
        Some(stem) => stem,
        None => command,
    };
    sanitize_server_name(stem)
}

/// Derive a name from a URL: its authority (`http://127.0.0.1:9090/mcp` -> `127_0_0_1_9090`).
fn name_from_url(url: &str) -> String {
    let without_scheme = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let authority = match without_scheme.split('/').next() {
        Some(authority) => authority,
        None => without_scheme,
    };
    sanitize_server_name(authority)
}

impl McpServer {
    /// The canonical server name sent to the harness by every runtime.
    ///
    /// The explicit [`name`](McpStdioServer::name) wins (sanitized to `[A-Za-z0-9_-]`);
    /// otherwise it is derived from the stdio command stem or the URL authority.
    #[must_use]
    pub fn server_name(&self) -> String {
        let (explicit, derived) = match self {
            Self::Stdio(s) => {
                let (effective_cmd, _, _) = s.unwrap_env_wrapper();
                (s.name.as_deref(), name_from_command(effective_cmd))
            }
            Self::Sse(s) => (s.name.as_deref(), name_from_url(&s.url)),
            Self::Http(s) => (s.name.as_deref(), name_from_url(&s.url)),
        };
        match explicit {
            Some(name) => sanitize_server_name(name),
            None => derived,
        }
    }

    /// Create a stdio-transport MCP server that spawns `command` as a child process.
    #[must_use]
    pub fn stdio(command: impl Into<String>) -> McpStdioServer {
        McpStdioServer::new(command)
    }

    /// Create an SSE-transport MCP server at the given `url`.
    #[must_use]
    pub fn sse(url: impl Into<String>) -> McpSseServer {
        McpSseServer::new(url)
    }

    /// Create a Streamable-HTTP-transport MCP server at the given `url`.
    #[must_use]
    pub fn http(url: impl Into<String>) -> McpStreamableHttpServer {
        McpStreamableHttpServer::new(url)
    }

    /// Explicit allowlist of MCP tool names exposed from this server, if set.
    #[must_use]
    pub fn enabled_tools(&self) -> Option<&[String]> {
        match self {
            Self::Stdio(s) => s.enabled_tools.as_deref(),
            Self::Sse(s) => s.enabled_tools.as_deref(),
            Self::Http(s) => s.enabled_tools.as_deref(),
        }
    }

    /// Explicit denylist of MCP tool names hidden from this server, if set.
    #[must_use]
    pub fn disabled_tools(&self) -> Option<&[String]> {
        match self {
            Self::Stdio(s) => s.disabled_tools.as_deref(),
            Self::Sse(s) => s.disabled_tools.as_deref(),
            Self::Http(s) => s.disabled_tools.as_deref(),
        }
    }

    /// Optional timeout in seconds for connecting and listing tools on this server.
    #[must_use]
    pub const fn timeout_seconds(&self) -> Option<i32> {
        match self {
            Self::Stdio(s) => s.timeout_seconds,
            Self::Sse(s) => s.timeout_seconds,
            Self::Http(s) => s.timeout_seconds,
        }
    }

    /// Whether tools from this server should be loaded eagerly into the prompt.
    #[must_use]
    pub const fn force_all_tools_eager(&self) -> bool {
        match self {
            Self::Stdio(s) => s.force_all_tools_eager,
            Self::Sse(s) => s.force_all_tools_eager,
            Self::Http(s) => s.force_all_tools_eager,
        }
    }

    /// Validate structural invariants for this MCP server configuration.
    ///
    /// # Errors
    ///
    /// Returns a human-readable error message when both `enabled_tools` and
    /// `disabled_tools` are set on the same server, or when `timeout_seconds`
    /// is negative.
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled_tools().is_some() && self.disabled_tools().is_some() {
            return Err(format!(
                "MCP server '{}' cannot set both enabled_tools and disabled_tools",
                self.server_name()
            ));
        }
        if let Some(secs) = self.timeout_seconds()
            && secs < 0
        {
            return Err(format!(
                "MCP server '{}' timeout_seconds must be non-negative, got {secs}",
                self.server_name()
            ));
        }
        Ok(())
    }
}

// ─── MCP Server Builders ───────────────────────────────────────────────────

impl From<McpStdioServer> for McpServer {
    fn from(val: McpStdioServer) -> Self {
        Self::Stdio(val)
    }
}

impl McpStdioServer {
    /// Create a new Stdio MCP Server configuration.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            name: None,
            command: command.into(),
            args: Vec::new(),
            enabled_tools: None,
            disabled_tools: None,
            timeout_seconds: None,
            force_all_tools_eager: false,
        }
    }

    /// Set an explicit server name (see [`McpServer::server_name`]).
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Add an argument to the command.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments to the command at once.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Add an environment variable for the spawned MCP server subprocess.
    ///
    /// Stored in the portable `env KEY=VALUE … <command> <args…>` encoding on
    /// [`McpStdioServer`] and lowered by both `NativeRuntime` and `PythonRuntime`
    /// into the transport's native `env` dictionary (so secrets are passed via
    /// process environment rather than `/proc/<pid>/cmdline`).
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let assignment = format!("{}={}", key.into(), value.into());
        if self.command == "env" {
            let env_count = self
                .args
                .iter()
                .take_while(|a| is_env_assignment(a))
                .count();
            self.args.insert(env_count, assignment);
        } else {
            let real_cmd = std::mem::replace(&mut self.command, "env".to_owned());
            let mut new_args = Vec::with_capacity(self.args.len() + 2);
            new_args.push(assignment);
            new_args.push(real_cmd);
            new_args.append(&mut self.args);
            self.args = new_args;
        }
        self
    }

    /// Extract `(command, args, env_map)` by unwrapping a leading `env KEY=VALUE …`
    /// wrapper when present.
    #[must_use]
    pub(crate) fn unwrap_env_wrapper(
        &self,
    ) -> (&str, &[String], std::collections::HashMap<String, String>) {
        if self.command == "env" {
            let env_count = self
                .args
                .iter()
                .take_while(|a| is_env_assignment(a))
                .count();
            if env_count > 0 && env_count < self.args.len() {
                let mut env_map = std::collections::HashMap::with_capacity(env_count);
                for arg in &self.args[..env_count] {
                    if let Some((k, v)) = arg.split_once('=') {
                        env_map.insert(k.to_owned(), v.to_owned());
                    }
                }
                return (
                    self.args[env_count].as_str(),
                    &self.args[env_count + 1..],
                    env_map,
                );
            }
        }
        (
            self.command.as_str(),
            &self.args,
            std::collections::HashMap::new(),
        )
    }

    /// Restrict this server to an explicit allowlist of MCP tool names.
    ///
    /// Clears any previously configured [`disabled_tools`](Self::disabled_tools).
    #[must_use]
    pub fn enabled_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.enabled_tools = Some(tools.into_iter().map(Into::into).collect());
        self.disabled_tools = None;
        self
    }

    /// Hide an explicit denylist of MCP tool names from this server.
    ///
    /// Clears any previously configured [`enabled_tools`](Self::enabled_tools).
    #[must_use]
    pub fn disabled_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.disabled_tools = Some(tools.into_iter().map(Into::into).collect());
        self.enabled_tools = None;
        self
    }

    /// Set the timeout in seconds for connecting to the server and listing tools.
    #[must_use]
    pub const fn timeout_seconds(mut self, seconds: i32) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }

    /// Force all tools from this server to be loaded eagerly into the prompt.
    #[must_use]
    pub const fn force_all_tools_eager(mut self, eager: bool) -> Self {
        self.force_all_tools_eager = eager;
        self
    }

    /// Build this stdio configuration into an [`McpServer`].
    #[must_use]
    pub fn build(self) -> McpServer {
        McpServer::Stdio(self)
    }
}

fn is_env_assignment(arg: &str) -> bool {
    let Some((key, _)) = arg.split_once('=') else {
        return false;
    };
    !key.is_empty()
        && key.bytes().enumerate().all(|(idx, b)| {
            if idx == 0 {
                b.is_ascii_alphabetic() || b == b'_'
            } else {
                b.is_ascii_alphanumeric() || b == b'_'
            }
        })
}

impl From<McpSseServer> for McpServer {
    fn from(val: McpSseServer) -> Self {
        Self::Sse(val)
    }
}

impl McpSseServer {
    /// Create a new SSE MCP Server configuration.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            name: None,
            url: url.into(),
            headers: None,
            enabled_tools: None,
            disabled_tools: None,
            timeout_seconds: None,
            force_all_tools_eager: false,
        }
    }

    /// Set an explicit server name (see [`McpServer::server_name`]).
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Add a header to the SSE connection.
    #[must_use]
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(k.into(), v.into());
        self
    }

    /// Restrict this server to an explicit allowlist of MCP tool names.
    ///
    /// Clears any previously configured [`disabled_tools`](Self::disabled_tools).
    #[must_use]
    pub fn enabled_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.enabled_tools = Some(tools.into_iter().map(Into::into).collect());
        self.disabled_tools = None;
        self
    }

    /// Hide an explicit denylist of MCP tool names from this server.
    ///
    /// Clears any previously configured [`enabled_tools`](Self::enabled_tools).
    #[must_use]
    pub fn disabled_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.disabled_tools = Some(tools.into_iter().map(Into::into).collect());
        self.enabled_tools = None;
        self
    }

    /// Set the timeout in seconds for connecting to the server and listing tools.
    #[must_use]
    pub const fn timeout_seconds(mut self, seconds: i32) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }

    /// Force all tools from this server to be loaded eagerly into the prompt.
    #[must_use]
    pub const fn force_all_tools_eager(mut self, eager: bool) -> Self {
        self.force_all_tools_eager = eager;
        self
    }

    /// Build this SSE configuration into an [`McpServer`].
    #[must_use]
    pub fn build(self) -> McpServer {
        McpServer::Sse(self)
    }
}

impl From<McpStreamableHttpServer> for McpServer {
    fn from(val: McpStreamableHttpServer) -> Self {
        Self::Http(val)
    }
}

impl McpStreamableHttpServer {
    /// Create a new Streamable HTTP MCP Server configuration.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            name: None,
            url: url.into(),
            headers: None,
            timeout: default_mcp_timeout(),
            sse_read_timeout: default_mcp_sse_read_timeout(),
            terminate_on_close: true,
            enabled_tools: None,
            disabled_tools: None,
            timeout_seconds: None,
            force_all_tools_eager: false,
        }
    }

    /// Add a header to the HTTP connection.
    #[must_use]
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(k.into(), v.into());
        self
    }

    /// Set an explicit server name (see [`McpServer::server_name`]).
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the HTTP connection/request timeout in seconds.
    #[must_use]
    pub const fn timeout(mut self, timeout: f64) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the streaming read timeout in seconds.
    #[must_use]
    pub const fn sse_read_timeout(mut self, timeout: f64) -> Self {
        self.sse_read_timeout = timeout;
        self
    }

    /// Restrict this server to an explicit allowlist of MCP tool names.
    ///
    /// Clears any previously configured [`disabled_tools`](Self::disabled_tools).
    #[must_use]
    pub fn enabled_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.enabled_tools = Some(tools.into_iter().map(Into::into).collect());
        self.disabled_tools = None;
        self
    }

    /// Hide an explicit denylist of MCP tool names from this server.
    ///
    /// Clears any previously configured [`enabled_tools`](Self::enabled_tools).
    #[must_use]
    pub fn disabled_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.disabled_tools = Some(tools.into_iter().map(Into::into).collect());
        self.enabled_tools = None;
        self
    }

    /// Set the timeout in seconds for connecting to the server and listing tools.
    #[must_use]
    pub const fn timeout_seconds(mut self, seconds: i32) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }

    /// Force all tools from this server to be loaded eagerly into the prompt.
    #[must_use]
    pub const fn force_all_tools_eager(mut self, eager: bool) -> Self {
        self.force_all_tools_eager = eager;
        self
    }

    /// Build this HTTP configuration into an [`McpServer`].
    #[must_use]
    pub fn build(self) -> McpServer {
        McpServer::Http(self)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "python")]
    use pyo3::types::PyAnyMethods;

    #[cfg(feature = "python")]
    use super::super::{DEFAULT_MCP_SSE_READ_TIMEOUT_SECS, DEFAULT_MCP_TIMEOUT_SECS};
    use super::*;

    #[cfg(feature = "python")]
    fn py_pydantic_field_default(module: &str, class: &str, field: &str) -> f64 {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::runtime::venv::configure_python_sys_path(py)
                .unwrap_or_else(|e| panic!("Failed to configure python sys.path: {e}"));
            let m = crate::runtime::py_scripts::import_serialized(py, module)
                .unwrap_or_else(|e| panic!("Failed to import {module}: {e}"));
            let cls = m
                .getattr(class)
                .unwrap_or_else(|e| panic!("Failed to get {module}.{class}: {e}"));
            let fields = cls
                .getattr("model_fields")
                .unwrap_or_else(|e| panic!("Failed to get {module}.{class}.model_fields: {e}"));
            let field_info = fields.get_item(field).unwrap_or_else(|e| {
                panic!("Failed to get field '{field}' from {module}.{class}.model_fields: {e}")
            });
            field_info
                .getattr("default")
                .unwrap_or_else(|e| {
                    panic!("Failed to get default for {module}.{class}.{field}: {e}")
                })
                .extract::<f64>()
                .unwrap_or_else(|e| {
                    panic!("Failed to extract {module}.{class}.{field} default as f64: {e}")
                })
        })
    }

    #[test]
    fn mcp_server_config_stdio_roundtrip() {
        let config = McpServer::Stdio(McpStdioServer {
            name: None,
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
            ],
            ..McpStdioServer::default()
        });
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpServer = serde_json::from_str(&json).unwrap();
        match parsed {
            McpServer::Stdio(s) => {
                assert_eq!(s.command, "npx");
                assert_eq!(
                    s.args,
                    vec!["-y", "@modelcontextprotocol/server-filesystem"]
                );
            }
            other => panic!("Expected Stdio, got {other:?}"),
        }
        // Verify the JSON contains the "type" tag from serde.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "stdio");
        assert!(value.get("enabled_tools").is_none());
        assert!(value.get("disabled_tools").is_none());
        assert!(value.get("timeout_seconds").is_none());
        assert!(value.get("force_all_tools_eager").is_none());
    }

    #[test]
    fn mcp_server_config_sse_roundtrip() {
        let config = McpServer::Sse(McpSseServer {
            name: None,
            url: "http://localhost:8080/sse".to_string(),
            headers: Some(std::collections::HashMap::from([(
                "Authorization".to_string(),
                "Bearer token123".to_string(),
            )])),
            ..McpSseServer::default()
        });
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpServer = serde_json::from_str(&json).unwrap();
        match parsed {
            McpServer::Sse(s) => {
                assert_eq!(s.url, "http://localhost:8080/sse");
                assert_eq!(
                    s.headers.as_ref().unwrap()["Authorization"],
                    "Bearer token123"
                );
            }
            other => panic!("Expected Sse, got {other:?}"),
        }
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "sse");
    }

    #[test]
    fn mcp_server_config_http_roundtrip() {
        let config = McpServer::Http(McpStreamableHttpServer {
            name: None,
            url: "http://localhost:9090/mcp".to_string(),
            headers: None,
            timeout: 60.0,
            sse_read_timeout: 120.0,
            terminate_on_close: false,
            ..McpStreamableHttpServer::default()
        });
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpServer = serde_json::from_str(&json).unwrap();
        match parsed {
            McpServer::Http(s) => {
                assert_eq!(s.url, "http://localhost:9090/mcp");
                assert!(s.headers.is_none());
                assert!((s.timeout - 60.0).abs() < f64::EPSILON);
                assert!((s.sse_read_timeout - 120.0).abs() < f64::EPSILON);
                assert!(!s.terminate_on_close);
            }
            other => panic!("Expected Http, got {other:?}"),
        }
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "http");
    }

    #[test]
    fn mcp_server_config_http_defaults_roundtrip() {
        // Deserialize with only required fields to verify defaults.
        let json = r#"{"type":"http","url":"http://example.com/mcp"}"#;
        let parsed: McpServer = serde_json::from_str(json).unwrap();
        match parsed {
            McpServer::Http(s) => {
                assert_eq!(s.url, "http://example.com/mcp");
                assert!(s.headers.is_none());
                assert!((s.timeout - 30.0).abs() < f64::EPSILON);
                assert!((s.sse_read_timeout - 300.0).abs() < f64::EPSILON);
                assert!(s.terminate_on_close);
            }
            other => panic!("Expected Http, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "python")]
    fn mcp_timeout_matches_python_sdk() {
        let py_val = py_pydantic_field_default(
            "google.antigravity.types",
            "McpStreamableHttpServer",
            "timeout",
        );
        assert!(
            (DEFAULT_MCP_TIMEOUT_SECS - py_val).abs() < f64::EPSILON,
            "Rust DEFAULT_MCP_TIMEOUT_SECS ({DEFAULT_MCP_TIMEOUT_SECS}) != Python SDK ({py_val})"
        );
    }

    #[test]
    #[cfg(feature = "python")]
    fn mcp_sse_read_timeout_matches_python_sdk() {
        let py_val = py_pydantic_field_default(
            "google.antigravity.types",
            "McpStreamableHttpServer",
            "sse_read_timeout",
        );
        assert!(
            (DEFAULT_MCP_SSE_READ_TIMEOUT_SECS - py_val).abs() < f64::EPSILON,
            "Rust DEFAULT_MCP_SSE_READ_TIMEOUT_SECS ({DEFAULT_MCP_SSE_READ_TIMEOUT_SECS}) != Python SDK ({py_val})"
        );
    }

    #[test]
    fn test_mcp_server_builders() {
        let stdio = McpServer::stdio("npx")
            .args(["-y", "@modelcontextprotocol/server-postgres"])
            .build();
        match stdio {
            McpServer::Stdio(s) => {
                assert_eq!(s.command, "npx");
                assert_eq!(s.args, vec!["-y", "@modelcontextprotocol/server-postgres"]);
            }
            _ => panic!("Expected Stdio"),
        }

        let sse = McpServer::sse("http://example.com/sse")
            .header("Auth", "token")
            .build();
        match sse {
            McpServer::Sse(s) => {
                assert_eq!(s.url, "http://example.com/sse");
                assert_eq!(s.headers.as_ref().unwrap()["Auth"], "token");
            }
            _ => panic!("Expected Sse"),
        }

        let http = McpServer::http("http://example.com/http")
            .header("Auth", "token")
            .timeout(10.0)
            .build();
        match http {
            McpServer::Http(s) => {
                assert_eq!(s.url, "http://example.com/http");
                assert_eq!(s.headers.as_ref().unwrap()["Auth"], "token");
                assert!((s.timeout - 10.0).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Http"),
        }
    }

    #[test]
    fn server_name_prefers_explicit_name_and_derives_consistently() {
        assert_eq!(
            McpServer::stdio("/usr/bin/python3").build().server_name(),
            "python3"
        );
        assert_eq!(
            McpServer::stdio("npx")
                .name("file system")
                .build()
                .server_name(),
            "file_system"
        );
        assert_eq!(
            McpServer::http("http://127.0.0.1:9090/mcp")
                .build()
                .server_name(),
            "127_0_0_1_9090"
        );
        assert_eq!(
            McpServer::sse("https://mcp.example.com/sse")
                .name("docs")
                .build()
                .server_name(),
            "docs"
        );
        assert_eq!(
            McpServer::stdio("").build().server_name(),
            DEFAULT_MCP_SERVER_NAME
        );
    }

    #[test]
    fn per_server_tool_filtering_eager_and_validation() {
        let stdio = McpServer::stdio("ghidra-mcp")
            .disabled_tools(["delete_project"])
            .enabled_tools(["decompile_function", "list_functions"])
            .timeout_seconds(45)
            .force_all_tools_eager(true)
            .build();
        stdio.validate().expect("valid stdio config");
        assert_eq!(
            stdio.enabled_tools(),
            Some(["decompile_function".to_owned(), "list_functions".to_owned()].as_slice())
        );
        assert_eq!(stdio.disabled_tools(), None);
        assert_eq!(stdio.timeout_seconds(), Some(45));
        assert!(stdio.force_all_tools_eager());

        let invalid_both = McpServer::Stdio(McpStdioServer {
            name: Some("conflict".to_owned()),
            command: "srv".to_owned(),
            args: Vec::new(),
            enabled_tools: Some(vec!["a".to_owned()]),
            disabled_tools: Some(vec!["b".to_owned()]),
            timeout_seconds: None,
            force_all_tools_eager: false,
        });
        let err = invalid_both.validate().expect_err("mutually exclusive");
        assert!(err.contains("cannot set both enabled_tools and disabled_tools"));

        let negative_timeout = McpServer::http("http://127.0.0.1:9090/mcp")
            .timeout_seconds(-5)
            .build();
        let err = negative_timeout.validate().expect_err("negative timeout");
        assert!(err.contains("timeout_seconds must be non-negative"));
    }

    #[test]
    fn stdio_env_wrapper_roundtrip_and_server_name_derivation() {
        let stdio = McpStdioServer::new("/usr/local/bin/my-mcp-bin")
            .arg("--port=8080")
            .env("API_KEY", "secret=value")
            .env("_DEBUG_MODE", "1");
        assert_eq!(stdio.command, "env");
        assert_eq!(
            stdio.args,
            [
                "API_KEY=secret=value",
                "_DEBUG_MODE=1",
                "/usr/local/bin/my-mcp-bin",
                "--port=8080"
            ]
        );
        let (cmd, args, env_map) = stdio.unwrap_env_wrapper();
        assert_eq!(cmd, "/usr/local/bin/my-mcp-bin");
        assert_eq!(args, ["--port=8080"]);
        assert_eq!(
            env_map.get("API_KEY").map(String::as_str),
            Some("secret=value")
        );
        assert_eq!(env_map.get("_DEBUG_MODE").map(String::as_str), Some("1"));

        let server = stdio.build();
        assert_eq!(server.server_name(), "my-mcp-bin");

        let bare_env = McpStdioServer::new("env").args(["FOO=bar"]);
        let (bare_cmd, bare_args, bare_map) = bare_env.unwrap_env_wrapper();
        assert_eq!(bare_cmd, "env");
        assert_eq!(bare_args, ["FOO=bar"]);
        assert!(bare_map.is_empty());
    }
}
