//! Child process lifecycle and Length-Delimited Protobuf handshake for `localharness`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
};

use crate::{error::Error, proto};

#[cfg(windows)]
pub(crate) const HARNESS_BIN_NAME: &str = "localharness.exe";
#[cfg(not(windows))]
pub(crate) const HARNESS_BIN_NAME: &str = "localharness";

const ENV_HARNESS_PATH: &str = "ANTIGRAVITY_HARNESS_PATH";
const ENV_VIRTUAL_ENV: &str = "VIRTUAL_ENV";
const ENV_HOME: &str = "HOME";
const GEMINI_CACHE_DIR: &str = ".gemini";
const ANTIGRAVITY_DIR: &str = "antigravity";
const BIN_DIR: &str = "bin";
const CLIENT_LANGUAGE: &str = "rust";
const RUST_LANGUAGE_VERSION: &str = "1.95";
const MAX_SPAWN_ATTEMPTS: u32 = 10;
const SPAWN_RETRY_BACKOFF_MS: u64 = 15;
const MAX_HANDSHAKE_BYTES: usize = 10 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Host environment variables safe to forward to `localharness` after `env_clear()`.
const ALLOWLISTED_HOST_ENV_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "TERM",
    "TZ",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "REQUESTS_CA_BUNDLE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SYSTEMROOT",
    "SystemRoot",
    "SYSTEMDRIVE",
    "SystemDrive",
    "WINDIR",
    "windir",
    "COMSPEC",
    "ComSpec",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "ProgramData",
    "PROGRAMFILES",
    "ProgramFiles",
    "PATHEXT",
    "HOMEDRIVE",
    "HOMEPATH",
];

fn is_allowlisted_host_env_key_with_casing(key: &str, case_insensitive: bool) -> bool {
    if crate::config::validation::is_dangerous_env_key(key) {
        return false;
    }
    if case_insensitive {
        ALLOWLISTED_HOST_ENV_KEYS
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(key))
            || key
                .as_bytes()
                .get(..3)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"LC_"))
            || key
                .as_bytes()
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"SSL_"))
    } else {
        ALLOWLISTED_HOST_ENV_KEYS.contains(&key)
            || key.starts_with("LC_")
            || key.starts_with("SSL_")
    }
}

fn is_allowlisted_host_env_key(key: &str) -> bool {
    // Windows environment variable names are case-insensitive (e.g. `Path`,
    // `Temp`, `ComSpec`, `SystemRoot`), so exact-case matching would drop
    // required system keys like `Path`.
    is_allowlisted_host_env_key_with_casing(key, cfg!(windows))
}

/// Verify that `binary_path` exists, is a non-empty regular file, and is executable.
pub(crate) fn validate_harness_binary(binary_path: &Path) -> Result<(), Error> {
    let meta = std::fs::metadata(binary_path).map_err(|e| Error::BackendError {
        message: format!(
            "Local harness binary not found or inaccessible at {}: {e}",
            binary_path.display()
        ),
    })?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(Error::BackendError {
            message: format!(
                "Local harness path {} is not a non-empty regular executable file",
                binary_path.display()
            ),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(Error::BackendError {
                message: format!(
                    "Local harness binary at {} is not executable (missing +x permission)",
                    binary_path.display()
                ),
            });
        }
    }
    Ok(())
}

/// Running local harness process with active stdin/stdout and parsed port.
pub(crate) struct HarnessProcess {
    pub(crate) child: Option<Child>,
    pub(crate) port: u16,
    pub(crate) api_key: String,
    pub(crate) _stdin: ChildStdin,
}

/// Locate the `localharness` binary on the current system.
pub(crate) fn find_harness_binary() -> Result<PathBuf, Error> {
    if let Some(custom) = std::env::var_os(ENV_HARNESS_PATH) {
        let p = PathBuf::from(custom);
        if validate_harness_binary(&p).is_ok() {
            return Ok(p);
        }
    }

    if let Some(embedded) = option_env!("NATIVE_HARNESS_PATH") {
        let p = PathBuf::from(embedded);
        if validate_harness_binary(&p).is_ok() {
            return Ok(p);
        }
    }

    if let Some(home) = std::env::var_os(ENV_HOME)
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        let bin_root = home
            .join(GEMINI_CACHE_DIR)
            .join(ANTIGRAVITY_DIR)
            .join(BIN_DIR);
        if let Some(ver) = option_env!("TARGET_ANTIGRAVITY_SDK_VERSION") {
            let versioned = bin_root.join(format!("v{ver}")).join(HARNESS_BIN_NAME);
            if validate_harness_binary(&versioned).is_ok() {
                return Ok(versioned);
            }
        }
        let shared = bin_root.join(HARNESS_BIN_NAME);
        if validate_harness_binary(&shared).is_ok() {
            return Ok(shared);
        }
    }

    if let Some(venv_bin) = find_venv_harness_binary() {
        return Ok(venv_bin);
    }

    Err(Error::BackendError {
        message: format!(
            "Could not find usable {HARNESS_BIN_NAME} binary. Set {ENV_HARNESS_PATH} or build with the 'native' feature."
        ),
    })
}

fn find_venv_harness_binary() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(active_venv) = std::env::var_os(ENV_VIRTUAL_ENV)
        && !active_venv.is_empty()
    {
        roots.push(PathBuf::from(active_venv));
    }
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        let mut cur = Some(Path::new(manifest_dir));
        while let Some(dir) = cur {
            let candidate = dir.join(".venv");
            if candidate.is_dir() {
                roots.push(candidate);
                break;
            }
            cur = dir.parent();
        }
    }

    for venv in roots {
        let win_candidate = venv
            .join("Lib")
            .join("site-packages")
            .join("google")
            .join("antigravity")
            .join("bin")
            .join(HARNESS_BIN_NAME);
        if validate_harness_binary(&win_candidate).is_ok() {
            return Some(win_candidate);
        }
        let entries = match std::fs::read_dir(venv.join("lib")) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::trace!(error = %err, path = %venv.display(), "No .venv/lib directory found");
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(v) => v,
                Err(err) => {
                    tracing::debug!(error = %err, "Failed reading .venv/lib entry");
                    continue;
                }
            };
            if entry.file_name().to_string_lossy().starts_with("python") {
                let unix_candidate = entry
                    .path()
                    .join("site-packages")
                    .join("google")
                    .join("antigravity")
                    .join("bin")
                    .join(HARNESS_BIN_NAME);
                if validate_harness_binary(&unix_candidate).is_ok() {
                    return Some(unix_candidate);
                }
            }
        }
    }
    None
}

pub(crate) fn configure_scrubbed_command_env(
    cmd: &mut Command,
    explicit_env: Option<&HashMap<String, String>>,
) {
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        if let Some(key_str) = k.to_str()
            && is_allowlisted_host_env_key(key_str)
        {
            cmd.env(k, v);
        }
    }
    #[cfg(windows)]
    if std::env::var_os("USERPROFILE").is_none() {
        let fallback = std::env::var_os(ENV_HOME).map_or_else(std::env::temp_dir, PathBuf::from);
        cmd.env("USERPROFILE", fallback);
    }
    if let Some(vars) = explicit_env {
        for (k, v) in vars {
            match crate::config::validation::validate_env_entry(k, v) {
                Ok(()) => {
                    cmd.env(k, v);
                }
                Err(reason) => {
                    tracing::warn!(key = %k, reason = %reason, "Dropped unsafe explicit_env entry before spawning localharness");
                }
            }
        }
    }
}

async fn send_handshake_config<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    save_dir: &Path,
    env_vars: Option<&HashMap<String, String>>,
) -> Result<(), Error> {
    let mut input_config = proto::localharness::InputConfig {
        storage_directory: save_dir.to_string_lossy().to_string(),
        client_info: Some(proto::localharness::ClientInfo {
            language: CLIENT_LANGUAGE.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            language_version: RUST_LANGUAGE_VERSION.to_string(),
            os: std::env::consts::OS.to_string(),
            os_version: String::new(),
        }),
        ..proto::localharness::InputConfig::default()
    };
    if let Some(vars) = env_vars {
        for (k, v) in vars {
            crate::config::validation::validate_env_entry(k, v)
                .map_err(|message| Error::InvalidConfig { message })?;
            input_config.env.insert(k.clone(), v.clone());
        }
    }

    let mut encoded = Vec::new();
    input_config
        .encode(&mut encoded)
        .map_err(|e| Error::BackendError {
            message: format!("Failed to encode InputConfig: {e}"),
        })?;

    let len = u32::try_from(encoded.len()).map_err(|e| Error::BackendError {
        message: format!("InputConfig length exceeds u32: {e}"),
    })?;
    stdin
        .write_all(&len.to_le_bytes())
        .await
        .map_err(|e| Error::BackendError {
            message: format!("Failed to write InputConfig length: {e}"),
        })?;
    stdin
        .write_all(&encoded)
        .await
        .map_err(|e| Error::BackendError {
            message: format!("Failed to write InputConfig bytes: {e}"),
        })?;
    stdin.flush().await.map_err(|e| Error::BackendError {
        message: format!("Failed to flush stdin: {e}"),
    })?;

    Ok(())
}

async fn read_handshake_response<R: AsyncRead + Unpin>(
    stdout: &mut R,
) -> Result<proto::localharness::OutputConfig, Error> {
    let mut len_buf = [0u8; 4];
    stdout
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| Error::BackendError {
            message: format!("Failed to read OutputConfig length from harness stdout: {e}"),
        })?;
    let resp_len = u32::from_le_bytes(len_buf) as usize;
    if resp_len > MAX_HANDSHAKE_BYTES {
        return Err(Error::BackendError {
            message: format!(
                "Handshake OutputConfig payload size ({resp_len} bytes) exceeds 10 MiB limit"
            ),
        });
    }
    let mut resp_buf = vec![0u8; resp_len];
    stdout
        .read_exact(&mut resp_buf)
        .await
        .map_err(|e| Error::BackendError {
            message: format!("Failed to read OutputConfig payload from harness stdout: {e}"),
        })?;

    proto::localharness::OutputConfig::decode(&resp_buf[..]).map_err(|e| Error::BackendError {
        message: format!("Failed to decode OutputConfig protobuf: {e}"),
    })
}

async fn kill_and_reap_child(mut child: Child) {
    if let Err(e) = child.start_kill() {
        tracing::debug!(error = %e, "Failed to signal child during handshake cleanup");
    }
    if let Err(e) = child.wait().await {
        tracing::debug!(error = %e, "Failed waiting on child during handshake cleanup");
    }
}

impl HarnessProcess {
    /// Spawn the local harness binary with a scrubbed environment and complete the
    /// length-delimited protobuf handshake within [`HANDSHAKE_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// Returns an error if the binary cannot be found, validated, spawned, or if the handshake fails.
    pub async fn spawn(
        save_dir: &Path,
        env_vars: Option<&HashMap<String, String>>,
        custom_binary_path: Option<&Path>,
    ) -> Result<Self, Error> {
        let binary_path = if let Some(custom) = custom_binary_path {
            custom.to_path_buf()
        } else {
            find_harness_binary()?
        };
        validate_harness_binary(&binary_path)?;

        let mut cmd = Command::new(&binary_path);
        cmd.kill_on_drop(true);
        configure_scrubbed_command_env(&mut cmd, env_vars);

        let mut attempts = 0u32;
        let mut child = loop {
            match cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
            {
                Ok(child) => break child,
                Err(e)
                    if attempts < MAX_SPAWN_ATTEMPTS
                        && (e.kind() == std::io::ErrorKind::ExecutableFileBusy
                            || e.raw_os_error() == Some(26)
                            || e.raw_os_error() == Some(32)) =>
                {
                    attempts += 1;
                    tracing::warn!(
                        attempt = attempts,
                        error = %e,
                        "Harness binary is busy; retrying spawn"
                    );
                    tokio::time::sleep(Duration::from_millis(
                        SPAWN_RETRY_BACKOFF_MS * u64::from(attempts),
                    ))
                    .await;
                }
                Err(e) => {
                    return Err(Error::BackendError {
                        message: format!(
                            "Failed to spawn localharness binary at {}: {e}",
                            binary_path.display()
                        ),
                    });
                }
            }
        };

        let Some(mut stdin) = child.stdin.take() else {
            kill_and_reap_child(child).await;
            return Err(Error::BackendError {
                message: "Failed to open child stdin".to_string(),
            });
        };
        let Some(mut stdout) = child.stdout.take() else {
            kill_and_reap_child(child).await;
            return Err(Error::BackendError {
                message: "Failed to open child stdout".to_string(),
            });
        };

        let handshake_res = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            send_handshake_config(&mut stdin, save_dir, env_vars).await?;
            read_handshake_response(&mut stdout).await
        })
        .await;

        let output_config = match handshake_res {
            Ok(Ok(cfg)) => cfg,
            Ok(Err(err)) => {
                drop(stdin);
                kill_and_reap_child(child).await;
                return Err(err);
            }
            Err(_elapsed) => {
                drop(stdin);
                kill_and_reap_child(child).await;
                return Err(Error::Timeout {
                    duration: HANDSHAKE_TIMEOUT,
                    operation: "localharness stdio handshake".to_string(),
                });
            }
        };

        let port = match u16::try_from(output_config.port) {
            Ok(p) if p > 0 => p,
            _ => {
                drop(stdin);
                kill_and_reap_child(child).await;
                return Err(Error::BackendError {
                    message: format!("Invalid port {} from localharness", output_config.port),
                });
            }
        };

        tracing::info!(port, "Localharness process started successfully");

        Ok(Self {
            child: Some(child),
            port,
            api_key: output_config.api_key,
            _stdin: stdin,
        })
    }

    /// Check whether the child process is still running.
    pub(crate) fn is_alive(&mut self) -> bool {
        let Some(ref mut child) = self.child else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) => false,
            Err(e) => {
                tracing::debug!(error = %e, "is_alive: error checking child process status");
                false
            }
        }
    }

    /// Terminate the child process and asynchronously wait for it to exit.
    pub(crate) async fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Err(e) = child.start_kill() {
                tracing::debug!(error = %e, "Process already killed or failed to kill");
            }
            if let Err(e) = child.wait().await {
                tracing::debug!(error = %e, "Error waiting for child process on termination");
            }
        }
    }

    /// Send kill signal to the child process without waiting.
    pub(crate) fn start_kill(&mut self) {
        if let Some(ref mut child) = self.child
            && let Err(e) = child.start_kill()
        {
            tracing::debug!(error = %e, "Process already killed or failed to kill");
        }
    }
}

impl Drop for HarnessProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Err(e) = child.start_kill() {
                tracing::debug!(error = %e, "HarnessProcess::drop: child already exited");
            }
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    super::native_bg_runtime().handle().spawn(async move {
                        if let Err(e) = child.wait().await {
                            tracing::debug!(error = %e, "Error reaping dropped localharness child");
                        }
                    });
                }
                Err(e) => {
                    tracing::debug!(error = %e, "HarnessProcess::drop: try_wait error");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handshake_config_and_response_roundtrip() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let save_dir = Path::new("/tmp/agy-test");
        send_handshake_config(&mut client, save_dir, None)
            .await
            .expect("send_handshake_config succeeds");

        let mut len_buf = [0u8; 4];
        server.read_exact(&mut len_buf).await.unwrap();
        let req_len = u32::from_le_bytes(len_buf) as usize;
        let mut req_buf = vec![0u8; req_len];
        server.read_exact(&mut req_buf).await.unwrap();
        let decoded = proto::localharness::InputConfig::decode(&req_buf[..]).unwrap();
        assert_eq!(decoded.storage_directory, "/tmp/agy-test");

        let resp = proto::localharness::OutputConfig {
            port: 12345,
            api_key: "secret-key".to_string(),
        };
        let mut resp_bytes = Vec::new();
        resp.encode(&mut resp_bytes).unwrap();
        let resp_len = u32::try_from(resp_bytes.len()).unwrap();
        server.write_all(&resp_len.to_le_bytes()).await.unwrap();
        server.write_all(&resp_bytes).await.unwrap();
        server.flush().await.unwrap();

        let parsed = read_handshake_response(&mut client)
            .await
            .expect("read_handshake_response succeeds");
        assert_eq!(parsed.port, 12345);
        assert_eq!(parsed.api_key, "secret-key");
    }

    #[tokio::test]
    async fn read_handshake_response_rejects_oversized_frame() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let oversized = u32::try_from(MAX_HANDSHAKE_BYTES + 1).unwrap();
        server.write_all(&oversized.to_le_bytes()).await.unwrap();
        server.flush().await.unwrap();

        let err = read_handshake_response(&mut client)
            .await
            .expect_err("oversized handshake frame must be rejected");
        assert!(err.to_string().contains("exceeds 10 MiB limit"));
    }

    #[tokio::test]
    async fn read_handshake_response_handles_unexpected_eof() {
        let (mut client, server) = tokio::io::duplex(64);
        drop(server);

        let err = read_handshake_response(&mut client)
            .await
            .expect_err("EOF on stdout must fail cleanly");
        assert!(err.to_string().contains("Failed to read OutputConfig"));
    }

    #[test]
    fn env_allowlist_permits_system_vars_and_blocks_ambient_secrets() {
        assert!(is_allowlisted_host_env_key("PATH"));
        assert!(is_allowlisted_host_env_key("HOME"));
        assert!(is_allowlisted_host_env_key("LC_ALL"));
        assert!(is_allowlisted_host_env_key("SSL_CERT_FILE"));
        assert!(!is_allowlisted_host_env_key("AWS_SECRET_ACCESS_KEY"));
        assert!(!is_allowlisted_host_env_key("GITHUB_TOKEN"));
        assert!(!is_allowlisted_host_env_key("SSH_AUTH_SOCK"));
        // Case-insensitive matching (used on Windows where `PATH` is commonly `Path`).
        assert!(is_allowlisted_host_env_key_with_casing("Path", true));
        assert!(is_allowlisted_host_env_key_with_casing("Temp", true));
        assert!(is_allowlisted_host_env_key_with_casing("UserProfile", true));
        assert!(is_allowlisted_host_env_key_with_casing(
            "ssl_cert_file",
            true
        ));
        assert!(is_allowlisted_host_env_key_with_casing("lc_all", true));
        assert!(!is_allowlisted_host_env_key_with_casing("ld_preload", true));
        assert!(!is_allowlisted_host_env_key_with_casing("pythonpath", true));
        assert!(!is_allowlisted_host_env_key_with_casing(
            "aws_secret_access_key",
            true
        ));
    }
}
