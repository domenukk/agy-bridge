//! Standardized `mcp.json` configuration loading.
//!
//! Mainstream MCP clients (Claude Desktop, Cursor, VS Code, the Antigravity
//! IDE, …) all share a common JSON configuration shape:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "filesystem": {
//!       "command": "npx",
//!       "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
//!       "env": { "LOG_LEVEL": "debug" }
//!     },
//!     "remote": {
//!       "url": "https://example.com/mcp",
//!       "headers": { "Authorization": "Bearer token123" }
//!     }
//!   }
//! }
//! ```
//!
//! This module parses that format into the strongly-typed [`McpServer`] enum so
//! callers can point any consumer at an existing, copy-pasted
//! `mcp.json` instead of re-declaring servers in a bespoke format.
//!
//! # Transport inference
//!
//! Each entry's transport is resolved from an explicit `type` field when
//! present (`"stdio"`, `"sse"`, `"http"`/`"streamable-http"`), otherwise it is
//! inferred from the fields that are set:
//!
//! - `command` present → **stdio**
//! - `url` present → **streamable HTTP**
//!
//! # Per-server environment variables
//!
//! The standard `mcp.json` format allows a per-server `env` map for stdio
//! servers.  These are **process-launch** variables, not MCP protocol fields:
//! they should be set in the environment of the spawned server subprocess.
//!
//! The Antigravity Python SDK's `McpStdioServer` only exposes `command` and
//! `args` (no `env` field), so we lower the env map into a portable POSIX
//! `env KEY=VALUE … <command> <args…>` wrapper.  The SDK spawns that wrapper
//! command verbatim, and the `env(1)` utility applies the variables before
//! exec-ing the real server binary.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use serde::{Deserialize, Serialize};

use super::{
    McpServer, McpSseServer, McpStdioServer, McpStreamableHttpServer, default_mcp_sse_read_timeout,
    default_mcp_timeout,
};

/// The POSIX command used to wrap stdio servers that declare per-server `env`.
///
/// `env(1)` applies the supplied `KEY=VALUE` assignments and then `exec`s the
/// real command, making it a transparent, portable wrapper.
const ENV_WRAPPER_COMMAND: &str = "env";

/// Errors that can occur while loading a standardized `mcp.json` file.
#[derive(Debug, thiserror::Error)]
pub enum McpConfigError {
    /// The config file could not be read from disk.
    #[error("failed to read MCP config file '{path}': {source}")]
    Io {
        /// The path that could not be read.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The config file contents were not valid JSON in the expected shape.
    #[error("failed to parse MCP config JSON: {0}")]
    Parse(#[source] serde_json::Error),

    /// A named server entry was structurally invalid (e.g. no transport could
    /// be resolved, or a required field was missing).
    #[error("invalid MCP server '{name}': {reason}")]
    InvalidServer {
        /// The offending server's name (its key in the `mcpServers` map).
        name: String,
        /// A human-readable description of what was wrong.
        reason: String,
    },
}

/// A standardized MCP configuration file: `{ "mcpServers": { … } }`.
///
/// Parse one with [`McpConfigFile::from_json_str`] or
/// [`McpConfigFile::from_path`], then convert the entries into strongly-typed
/// [`McpServer`] values with [`McpConfigFile::into_servers`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfigFile {
    /// Map of server name → server specification.
    ///
    /// Serialized under the standardized `mcpServers` key. A [`BTreeMap`] keeps
    /// iteration order deterministic (sorted by name), which makes downstream
    /// output stable and testable.
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: BTreeMap<String, McpServerSpec>,
}

/// A single entry from a standardized `mcp.json` file.
///
/// This mirrors the permissive superset of fields accepted by mainstream MCP
/// clients. Which fields are meaningful depends on the resolved transport; see
/// the [module docs](self) for the inference rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpServerSpec {
    /// Explicit transport selector: `"stdio"`, `"sse"`, `"http"`, or
    /// `"streamable-http"`. When absent, the transport is inferred.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,

    /// Command to spawn for a stdio server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Arguments passed to `command` (stdio only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// Per-server environment variables set when launching the subprocess
    /// (stdio only).
    ///
    /// Lowered into an `env KEY=VALUE …` wrapper at conversion time so the
    /// SDK’s `command`/`args`-only model can still apply them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,

    /// Endpoint URL for an SSE or HTTP server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Optional headers sent with SSE/HTTP connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,

    /// Connection timeout in seconds (HTTP only). Defaults to the SDK value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,

    /// SSE read timeout in seconds (HTTP only). Defaults to the SDK value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sse_read_timeout: Option<f64>,

    /// Whether to terminate the connection on close (HTTP only). Defaults to
    /// `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate_on_close: Option<bool>,
}

/// The transport a [`McpServerSpec`] resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Stdio,
    Sse,
    Http,
}

/// Build an [`McpStdioServer`], lowering a non-empty `env` map into a POSIX
/// `env KEY=VALUE … <command> <args…>` wrapper.
///
/// When `env` is empty the original command is returned unchanged.  Keys are
/// emitted in sorted order (the input is a [`BTreeMap`]) so the generated
/// argument vector is deterministic and testable.
fn build_stdio_server(
    command: String,
    args: Vec<String>,
    env: &BTreeMap<String, String>,
) -> McpStdioServer {
    if env.is_empty() {
        return McpStdioServer { command, args };
    }

    let mut wrapped = Vec::with_capacity(env.len() + 1 + args.len());
    for (key, value) in env {
        wrapped.push(format!("{key}={value}"));
    }
    wrapped.push(command);
    wrapped.extend(args);

    McpStdioServer {
        command: ENV_WRAPPER_COMMAND.to_owned(),
        args: wrapped,
    }
}

impl McpServerSpec {
    /// Resolve the transport for this spec, using the explicit `type` field
    /// when present and otherwise inferring from which fields are set.
    fn resolve_transport(&self, name: &str) -> Result<Transport, McpConfigError> {
        if let Some(raw) = &self.transport {
            return match raw.trim().to_ascii_lowercase().as_str() {
                "stdio" => Ok(Transport::Stdio),
                "sse" => Ok(Transport::Sse),
                "http" | "streamable-http" | "streamable_http" | "streamablehttp" => {
                    Ok(Transport::Http)
                }
                other => Err(McpConfigError::InvalidServer {
                    name: name.to_owned(),
                    reason: format!("unknown transport type '{other}'"),
                }),
            };
        }

        match (self.command.is_some(), self.url.is_some()) {
            (true, false) => Ok(Transport::Stdio),
            (false, true) => Ok(Transport::Http),
            (true, true) => Err(McpConfigError::InvalidServer {
                name: name.to_owned(),
                reason: "both 'command' and 'url' are set; specify an explicit 'type'".to_owned(),
            }),
            (false, false) => Err(McpConfigError::InvalidServer {
                name: name.to_owned(),
                reason: "must specify either 'command' (stdio) or 'url' (sse/http)".to_owned(),
            }),
        }
    }

    /// Convert this spec into a strongly-typed [`McpServer`].
    ///
    /// `name` is used only to produce descriptive error messages.
    ///
    /// # Errors
    ///
    /// Returns [`McpConfigError::InvalidServer`] if the transport cannot be
    /// resolved or a field required by the resolved transport is missing.
    pub fn into_server(self, name: &str) -> Result<McpServer, McpConfigError> {
        let transport = self.resolve_transport(name)?;
        match transport {
            Transport::Stdio => {
                let command = self.command.ok_or_else(|| McpConfigError::InvalidServer {
                    name: name.to_owned(),
                    reason: "stdio transport requires a 'command'".to_owned(),
                })?;
                Ok(McpServer::Stdio(build_stdio_server(
                    command, self.args, &self.env,
                )))
            }
            Transport::Sse => {
                let url = self.url.ok_or_else(|| McpConfigError::InvalidServer {
                    name: name.to_owned(),
                    reason: "sse transport requires a 'url'".to_owned(),
                })?;
                Ok(McpServer::Sse(McpSseServer {
                    url,
                    headers: self.headers,
                }))
            }
            Transport::Http => {
                let url = self.url.ok_or_else(|| McpConfigError::InvalidServer {
                    name: name.to_owned(),
                    reason: "http transport requires a 'url'".to_owned(),
                })?;
                Ok(McpServer::Http(McpStreamableHttpServer {
                    url,
                    headers: self.headers,
                    timeout: self.timeout.unwrap_or_else(default_mcp_timeout),
                    sse_read_timeout: self
                        .sse_read_timeout
                        .unwrap_or_else(default_mcp_sse_read_timeout),
                    // Match the `McpStreamableHttpServer` serde default
                    // (`default_true`): the SDK terminates connections on close
                    // unless the user explicitly opts out.
                    terminate_on_close: self.terminate_on_close.unwrap_or_else(super::default_true),
                }))
            }
        }
    }
}

impl McpConfigFile {
    /// Parse a standardized `mcp.json` document from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns [`McpConfigError::Parse`] if the string is not valid JSON in the
    /// `{ "mcpServers": { … } }` shape.
    pub fn from_json_str(json: &str) -> Result<Self, McpConfigError> {
        serde_json::from_str(json).map_err(McpConfigError::Parse)
    }

    /// Load and parse a standardized `mcp.json` file from disk.
    ///
    /// # Errors
    ///
    /// Returns [`McpConfigError::Io`] if the file cannot be read, or
    /// [`McpConfigError::Parse`] if its contents are not valid JSON.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, McpConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| McpConfigError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_json_str(&contents)
    }

    /// Returns `true` if the config declares no servers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mcp_servers.is_empty()
    }

    /// Number of declared servers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mcp_servers.len()
    }

    /// Convert every entry into a `(name, McpServer)` pair.
    ///
    /// Pairs are returned sorted by server name (the backing map is a
    /// [`BTreeMap`]), giving deterministic ordering.
    ///
    /// # Errors
    ///
    /// Returns the first [`McpConfigError::InvalidServer`] encountered.
    pub fn into_servers(self) -> Result<Vec<(String, McpServer)>, McpConfigError> {
        self.mcp_servers
            .into_iter()
            .map(|(name, spec)| {
                let server = spec.into_server(&name)?;
                Ok((name, server))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fully-featured, standardized `mcp.json` exercising every transport.
    /// Kept as a single canonical fixture so tests parse real-world JSON.
    const STANDARD_CONFIG: &str = r#"{
      "mcpServers": {
        "filesystem": {
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
        },
        "github": {
          "command": "docker",
          "args": ["run", "-i", "ghcr.io/github/github-mcp-server"],
          "env": { "GITHUB_TOKEN": "secret", "API_HOST": "github.com" }
        },
        "remote-http": {
          "url": "https://example.com/mcp",
          "headers": { "Authorization": "Bearer token123" }
        },
        "remote-sse": {
          "type": "sse",
          "url": "https://example.com/sse"
        }
      }
    }"#;

    #[test]
    fn parses_standardized_config_with_all_transports() {
        let config = McpConfigFile::from_json_str(STANDARD_CONFIG).unwrap();
        assert_eq!(config.len(), 4);
        assert!(!config.is_empty());

        let servers = config.into_servers().unwrap();
        // BTreeMap ordering: filesystem, github, remote-http, remote-sse.
        let names: Vec<&str> = servers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["filesystem", "github", "remote-http", "remote-sse"]);
    }

    #[test]
    fn stdio_without_env_is_passed_through_verbatim() {
        let config = McpConfigFile::from_json_str(STANDARD_CONFIG).unwrap();
        let servers = config.into_servers().unwrap();
        let (_, server) = &servers[0];
        match server {
            McpServer::Stdio(s) => {
                assert_eq!(s.command, "npx");
                assert_eq!(
                    s.args,
                    ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
                );
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn stdio_with_env_is_lowered_to_env_wrapper() {
        let config = McpConfigFile::from_json_str(STANDARD_CONFIG).unwrap();
        let servers = config.into_servers().unwrap();
        let (name, server) = &servers[1];
        assert_eq!(name, "github");
        match server {
            McpServer::Stdio(s) => {
                assert_eq!(s.command, "env");
                // Env keys are sorted (BTreeMap): API_HOST before GITHUB_TOKEN,
                // followed by the original command and its args.
                assert_eq!(
                    s.args,
                    [
                        "API_HOST=github.com",
                        "GITHUB_TOKEN=secret",
                        "docker",
                        "run",
                        "-i",
                        "ghcr.io/github/github-mcp-server",
                    ]
                );
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn url_without_type_infers_http() {
        let config = McpConfigFile::from_json_str(STANDARD_CONFIG).unwrap();
        let servers = config.into_servers().unwrap();
        let (name, server) = &servers[2];
        assert_eq!(name, "remote-http");
        match server {
            McpServer::Http(s) => {
                assert_eq!(s.url, "https://example.com/mcp");
                assert_eq!(
                    s.headers.as_ref().unwrap()["Authorization"],
                    "Bearer token123"
                );
                // Unspecified fields fall back to SDK defaults.
                assert!((s.timeout - default_mcp_timeout()).abs() < f64::EPSILON);
                assert!((s.sse_read_timeout - default_mcp_sse_read_timeout()).abs() < f64::EPSILON);
                assert!(s.terminate_on_close);
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn explicit_sse_type_selects_sse_transport() {
        let config = McpConfigFile::from_json_str(STANDARD_CONFIG).unwrap();
        let servers = config.into_servers().unwrap();
        let (name, server) = &servers[3];
        assert_eq!(name, "remote-sse");
        match server {
            McpServer::Sse(s) => {
                assert_eq!(s.url, "https://example.com/sse");
                assert!(s.headers.is_none());
            }
            other => panic!("expected Sse, got {other:?}"),
        }
    }

    #[test]
    fn explicit_stdio_type_is_honored() {
        let json = r#"{"mcpServers": {"s": {"type": "stdio", "command": "run"}}}"#;
        let servers = McpConfigFile::from_json_str(json)
            .unwrap()
            .into_servers()
            .unwrap();
        assert!(matches!(servers[0].1, McpServer::Stdio(_)));
    }

    #[test]
    fn streamable_http_type_alias_selects_http() {
        let json = r#"{"mcpServers": {"s": {"type": "streamable-http", "url": "https://x/mcp"}}}"#;
        let servers = McpConfigFile::from_json_str(json)
            .unwrap()
            .into_servers()
            .unwrap();
        assert!(matches!(servers[0].1, McpServer::Http(_)));
    }

    #[test]
    fn http_custom_timeouts_are_preserved() {
        let json = r#"{
          "mcpServers": {
            "s": {
              "url": "https://x/mcp",
              "timeout": 12.5,
              "sse_read_timeout": 99.0,
              "terminate_on_close": false
            }
          }
        }"#;
        let servers = McpConfigFile::from_json_str(json)
            .unwrap()
            .into_servers()
            .unwrap();
        match &servers[0].1 {
            McpServer::Http(s) => {
                assert!((s.timeout - 12.5).abs() < f64::EPSILON);
                assert!((s.sse_read_timeout - 99.0).abs() < f64::EPSILON);
                assert!(!s.terminate_on_close);
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn empty_config_is_empty() {
        let config = McpConfigFile::from_json_str(r#"{"mcpServers": {}}"#).unwrap();
        assert!(config.is_empty());
        assert_eq!(config.len(), 0);
        assert!(config.into_servers().unwrap().is_empty());
    }

    #[test]
    fn missing_mcp_servers_key_defaults_to_empty() {
        let config = McpConfigFile::from_json_str("{}").unwrap();
        assert!(config.is_empty());
    }

    #[test]
    fn invalid_json_is_a_parse_error() {
        let err = McpConfigFile::from_json_str("{ not json").unwrap_err();
        assert!(matches!(err, McpConfigError::Parse(_)));
    }

    #[test]
    fn entry_without_command_or_url_is_rejected() {
        let json = r#"{"mcpServers": {"broken": {"args": ["x"]}}}"#;
        let err = McpConfigFile::from_json_str(json)
            .unwrap()
            .into_servers()
            .unwrap_err();
        match err {
            McpConfigError::InvalidServer { name, .. } => assert_eq!(name, "broken"),
            other => panic!("expected InvalidServer, got {other:?}"),
        }
    }

    #[test]
    fn entry_with_both_command_and_url_is_rejected() {
        let json = r#"{"mcpServers": {"ambiguous": {"command": "c", "url": "https://x"}}}"#;
        let err = McpConfigFile::from_json_str(json)
            .unwrap()
            .into_servers()
            .unwrap_err();
        assert!(matches!(err, McpConfigError::InvalidServer { .. }));
    }

    #[test]
    fn unknown_transport_type_is_rejected() {
        let json = r#"{"mcpServers": {"s": {"type": "carrier-pigeon", "command": "c"}}}"#;
        let err = McpConfigFile::from_json_str(json)
            .unwrap()
            .into_servers()
            .unwrap_err();
        assert!(matches!(err, McpConfigError::InvalidServer { .. }));
    }

    #[test]
    fn explicit_stdio_without_command_is_rejected() {
        let json = r#"{"mcpServers": {"s": {"type": "stdio", "url": "https://x"}}}"#;
        let err = McpConfigFile::from_json_str(json)
            .unwrap()
            .into_servers()
            .unwrap_err();
        assert!(matches!(err, McpConfigError::InvalidServer { .. }));
    }

    #[test]
    fn from_path_reads_and_parses_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, STANDARD_CONFIG).unwrap();

        let config = McpConfigFile::from_path(&path).unwrap();
        assert_eq!(config.len(), 4);
    }

    #[test]
    fn from_path_missing_file_is_io_error() {
        let err = McpConfigFile::from_path("/nonexistent/definitely/mcp.json").unwrap_err();
        assert!(matches!(err, McpConfigError::Io { .. }));
    }

    #[test]
    fn build_stdio_server_without_env_is_verbatim() {
        let server = build_stdio_server("cmd".to_owned(), vec!["a".to_owned()], &BTreeMap::new());
        assert_eq!(server.command, "cmd");
        assert_eq!(server.args, ["a"]);
    }

    #[test]
    fn build_stdio_server_with_env_wraps_sorted() {
        let mut env = BTreeMap::new();
        env.insert("Z".to_owned(), "last".to_owned());
        env.insert("A".to_owned(), "first".to_owned());
        let server = build_stdio_server("my-server".to_owned(), vec!["--flag".to_owned()], &env);
        assert_eq!(server.command, "env");
        assert_eq!(server.args, ["A=first", "Z=last", "my-server", "--flag"]);
    }
}
