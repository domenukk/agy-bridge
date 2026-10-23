//! MCP (Model Context Protocol) server configuration types.

use serde::{Deserialize, Serialize};

use super::{default_mcp_sse_read_timeout, default_mcp_timeout, default_true};

/// Configuration for an MCP server connected via stdio.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Configuration for an MCP server connected via SSE.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            Self::Stdio(s) => (s.name.as_deref(), name_from_command(&s.command)),
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

    /// Build this stdio configuration into an [`McpServer`].
    #[must_use]
    pub fn build(self) -> McpServer {
        McpServer::Stdio(self)
    }
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
}
