//! Child process lifecycle and Length-Delimited Protobuf handshake for `localharness`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
};

use prost::Message as _;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use crate::{error::Error, proto};

#[cfg(windows)]
pub(crate) const HARNESS_BIN_NAME: &str = "localharness.exe";
#[cfg(not(windows))]
pub(crate) const HARNESS_BIN_NAME: &str = "localharness";

const ENV_HARNESS_PATH: &str = "ANTIGRAVITY_HARNESS_PATH";
const ENV_PATH: &str = "PATH";
const ENV_HOME: &str = "HOME";
const GEMINI_CACHE_DIR: &str = ".gemini";
const ANTIGRAVITY_DIR: &str = "antigravity";
const BIN_DIR: &str = "bin";
const CLIENT_LANGUAGE: &str = "rust";
const RUST_LANGUAGE_VERSION: &str = "1.95";
const MAX_SPAWN_ATTEMPTS: u32 = 10;
const SPAWN_RETRY_BACKOFF_MS: u64 = 15;

/// Running local harness process with active stdin/stdout and parsed port.
pub(crate) struct HarnessProcess {
    pub(crate) child: Child,
    pub(crate) port: u16,
    pub(crate) api_key: String,
    pub(crate) _stdin: ChildStdin,
}

/// Locate the `localharness` binary on the current system.
pub(crate) fn find_harness_binary() -> Result<PathBuf, Error> {
    // 1. Explicit ANTIGRAVITY_HARNESS_PATH environment variable
    // NOLINT: environment variable is optional
    if let Ok(custom) = std::env::var(ENV_HARNESS_PATH) {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return Ok(p);
        }
    }

    // 2. Embedded path from build.rs
    if let Some(embedded) = option_env!("NATIVE_HARNESS_PATH") {
        let p = PathBuf::from(embedded);
        if p.is_file() {
            return Ok(p);
        }
    }

    // 3. User home cache (~/.gemini/antigravity/bin/localharness)
    if let Some(home) = std::env::var_os(ENV_HOME)
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        let candidate = home
            .join(GEMINI_CACHE_DIR)
            .join(ANTIGRAVITY_DIR)
            .join(BIN_DIR)
            .join(HARNESS_BIN_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // 4. PATH lookup
    // NOLINT: environment variable is optional
    if let Ok(path_var) = std::env::var(ENV_PATH) {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(HARNESS_BIN_NAME);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(Error::BackendError {
        message: format!(
            "Could not find {HARNESS_BIN_NAME} binary. Set {ENV_HARNESS_PATH} or build with the 'native' feature."
        ),
    })
}

async fn send_handshake_config(
    stdin: &mut ChildStdin,
    save_dir: &Path,
    env_vars: Option<&HashMap<String, String>>,
) -> Result<(), Error> {
    let input_config = proto::localharness::InputConfig {
        storage_directory: save_dir.to_string_lossy().to_string(),
        port: 0,
        bind_address: String::new(),
        client_info: Some(proto::localharness::ClientInfo {
            language: CLIENT_LANGUAGE.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            language_version: RUST_LANGUAGE_VERSION.to_string(),
            os: std::env::consts::OS.to_string(),
            os_version: String::new(),
        }),
        // NOLINT: empty environment map fallback when not provided
        env: env_vars.cloned().unwrap_or_default(),
        use_interactions_api: false,
    };

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

async fn read_handshake_response(
    stdout: &mut ChildStdout,
) -> Result<proto::localharness::OutputConfig, Error> {
    const MAX_HANDSHAKE_BYTES: usize = 10 * 1024 * 1024;

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

impl HarnessProcess {
    /// Spawn the local harness binary and complete the length-delimited protobuf handshake.
    ///
    /// # Errors
    ///
    /// Returns an error if binary cannot be found, spawned, or handshake fails.
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

        if !binary_path.exists() {
            return Err(Error::BackendError {
                message: format!(
                    "Local harness binary not found at {}",
                    binary_path.display()
                ),
            });
        }

        let mut cmd = Command::new(&binary_path);
        for (k, v) in crate::load_dotenv() {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        if std::env::var_os("USERPROFILE").is_none() {
            let fallback =
                std::env::var_os(ENV_HOME).map_or_else(std::env::temp_dir, PathBuf::from);
            cmd.env("USERPROFILE", fallback);
        }
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
                    tokio::time::sleep(std::time::Duration::from_millis(
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

        let mut stdin = child.stdin.take().ok_or_else(|| Error::BackendError {
            message: "Failed to open child stdin".to_string(),
        })?;
        let mut stdout = child.stdout.take().ok_or_else(|| Error::BackendError {
            message: "Failed to open child stdout".to_string(),
        })?;

        send_handshake_config(&mut stdin, save_dir, env_vars).await?;
        let output_config = read_handshake_response(&mut stdout).await?;

        tracing::info!(
            port = output_config.port,
            "Localharness process started successfully"
        );

        let port = u16::try_from(output_config.port).map_err(|e| Error::BackendError {
            message: format!("Invalid port {}: {e}", output_config.port),
        })?;

        Ok(Self {
            child,
            port,
            api_key: output_config.api_key,
            _stdin: stdin,
        })
    }

    /// Check whether the child process is still running.
    pub(crate) fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
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
        if let Err(e) = self.child.kill().await {
            tracing::debug!(error = %e, "Process already terminated");
        }
        if let Err(e) = self.child.wait().await {
            tracing::debug!(error = %e, "Error waiting for child process on termination");
        }
    }

    /// Send kill signal to the child process without waiting.
    pub(crate) fn start_kill(&mut self) {
        if let Err(e) = self.child.start_kill() {
            tracing::debug!(error = %e, "Process already killed or failed to kill");
        }
    }
}

impl Drop for HarnessProcess {
    fn drop(&mut self) {
        self.start_kill();
    }
}
