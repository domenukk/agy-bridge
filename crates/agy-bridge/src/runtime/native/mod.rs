//! Native local harness runtime: communicates with the `localharness` binary
//! via stdio protobuf handshake and WebSocket JSON protocol without Python.

pub(crate) mod config;
pub(crate) mod events;
pub(crate) mod process;
mod runtime_impl;
pub(crate) mod session;
pub(crate) mod tools;
pub(crate) mod ws;

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[cfg(test)]
use self::events::to_proto_user_input;
use self::{
    process::HarnessProcess,
    session::{InitialHarnessState, NativeAgentSession},
    ws::{HarnessWsStream, connect_to_harness, initialize_harness},
};
#[cfg(test)]
use crate::agent::Runtime;
use crate::{
    agent::AgentId, config::AgentConfig, error::Error, hooks::Hooks, policies::PolicySet, proto,
    runtime::config::RuntimeConfig,
};

const WS_CONNECT_RETRY_COUNT: usize = 10;
const WS_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);
const EVENT_CHANNEL_BUFFER_SIZE: usize = 64;

static BRIDGE_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_random_128_hex() -> String {
    let mut bytes = [0u8; 16];
    let urandom_ok = {
        #[cfg(unix)]
        {
            use std::io::Read as _;
            match std::fs::File::open("/dev/urandom") {
                Ok(mut f) => match f.read_exact(&mut bytes) {
                    Ok(()) => true,
                    Err(err) => {
                        tracing::debug!(error = %err, "Failed reading /dev/urandom for save_dir nonce; falling back to RandomState");
                        false
                    }
                },
                Err(err) => {
                    tracing::debug!(error = %err, "Failed opening /dev/urandom for save_dir nonce; falling back to RandomState");
                    false
                }
            }
        }
        #[cfg(not(unix))]
        {
            false
        }
    };
    if !urandom_ok {
        use std::{collections::hash_map::RandomState, hash::BuildHasher as _};
        let seq = BRIDGE_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now();
        let hi = RandomState::new().hash_one((std::process::id(), seq, now));
        let lo = RandomState::new().hash_one((seq, std::process::id(), now));
        bytes[..8].copy_from_slice(&hi.to_le_bytes());
        bytes[8..].copy_from_slice(&lo.to_le_bytes());
    }
    let mut hex = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        if let Err(err) = write!(&mut hex, "{b:02x}") {
            tracing::error!(error = %err, "Failed formatting save_dir hex byte");
        }
    }
    hex
}

fn allocate_isolated_bridge_save_dir() -> PathBuf {
    let rand_hex = generate_random_128_hex();
    std::env::temp_dir().join(format!("agy-bridge-{}-{rand_hex}", std::process::id()))
}

fn create_strict_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn verify_real_dir(path: &Path, require_mode_0700: bool) -> Result<(), Error> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| Error::BackendError {
        message: format!("Failed to inspect save_dir {}: {e}", path.display()),
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(Error::BackendError {
            message: format!(
                "Refusing save_dir {}: path is a symlink or non-directory",
                path.display()
            ),
        });
    }
    #[cfg(unix)]
    if require_mode_0700 {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o700 {
            return Err(Error::BackendError {
                message: format!(
                    "Refusing default save_dir {}: expected mode 0700, found {mode:04o}",
                    path.display()
                ),
            });
        }
    }
    #[cfg(not(unix))]
    if require_mode_0700 {
        tracing::trace!(path = %path.display(), "Private save_dir verified on non-Unix platform");
    }
    Ok(())
}

/// Running local harness instance with reference counting of active agents.
struct SharedHarnessEntry {
    process: HarnessProcess,
    active_agents: HashSet<AgentId>,
}

/// Per-`(save_dir, sorted_env)` harness slot protected by its own async mutex so concurrent
/// agent creations for different directories or environment sets never block or collide.
pub(crate) type HarnessCacheKey = (PathBuf, Vec<(String, String)>);
type HarnessSlot = Arc<tokio::sync::Mutex<Option<SharedHarnessEntry>>>;

pub(crate) fn native_bg_runtime() -> &'static tokio::runtime::Runtime {
    static NATIVE_BG_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> =
        std::sync::OnceLock::new();
    NATIVE_BG_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("agy-bridge-native-runtime")
            .build()
            .expect("Failed to initialize shared NativeRuntime Tokio runtime")
    })
}

/// Native local harness runtime communicating natively via WebSocket and Protobuf.
pub struct NativeRuntime {
    sessions: Arc<RwLock<HashMap<AgentId, Arc<NativeAgentSession>>>>,
    harnesses: Arc<Mutex<HashMap<HarnessCacheKey, HarnessSlot>>>,
    runtime_handle: tokio::runtime::Handle,
    default_save_dir: PathBuf,
    default_save_dir_initialized: Arc<Mutex<bool>>,
    config: RuntimeConfig,
}

impl NativeRuntime {
    /// Create a new native runtime instance with an isolated default storage directory.
    ///
    /// # Panics
    ///
    /// Panics if the shared multi-threaded Tokio runtime cannot be initialized.
    #[must_use]
    pub fn new(config: RuntimeConfig) -> Self {
        let runtime_handle = native_bg_runtime().handle().clone();
        let default_save_dir = allocate_isolated_bridge_save_dir();
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            harnesses: Arc::new(Mutex::new(HashMap::new())),
            runtime_handle,
            default_save_dir,
            default_save_dir_initialized: Arc::new(Mutex::new(false)),
            config,
        }
    }

    pub(crate) fn prepare_save_dir(
        &self,
        explicit_save_dir: Option<&Path>,
    ) -> Result<PathBuf, Error> {
        if let Some(user_dir) = explicit_save_dir {
            match std::fs::symlink_metadata(user_dir) {
                Ok(meta) => {
                    if meta.file_type().is_symlink() || !meta.is_dir() {
                        return Err(Error::BackendError {
                            message: format!(
                                "Refusing save_dir {}: path is a symlink or non-directory",
                                user_dir.display()
                            ),
                        });
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir_all(user_dir).map_err(|err| Error::BackendError {
                        message: format!("Failed to create save_dir {}: {err}", user_dir.display()),
                    })?;
                    verify_real_dir(user_dir, false)?;
                }
                Err(e) => {
                    return Err(Error::BackendError {
                        message: format!("Failed to inspect save_dir {}: {e}", user_dir.display()),
                    });
                }
            }
            return Ok(user_dir.to_path_buf());
        }

        let mut initialized =
            self.default_save_dir_initialized
                .lock()
                .map_err(|e| Error::BackendError {
                    message: format!("Poisoned default_save_dir_initialized lock: {e}"),
                })?;
        if !*initialized {
            create_strict_private_dir(&self.default_save_dir).map_err(|e| Error::BackendError {
                message: format!(
                    "Failed to create isolated default save_dir {}: {e}",
                    self.default_save_dir.display()
                ),
            })?;
            verify_real_dir(&self.default_save_dir, true)?;
            *initialized = true;
            return Ok(self.default_save_dir.clone());
        }

        match std::fs::symlink_metadata(&self.default_save_dir) {
            Ok(_) => verify_real_dir(&self.default_save_dir, true)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                create_strict_private_dir(&self.default_save_dir).map_err(|err| {
                    Error::BackendError {
                        message: format!(
                            "Failed to recreate isolated default save_dir {}: {err}",
                            self.default_save_dir.display()
                        ),
                    }
                })?;
                verify_real_dir(&self.default_save_dir, true)?;
            }
            Err(e) => {
                return Err(Error::BackendError {
                    message: format!(
                        "Failed to inspect isolated default save_dir {}: {e}",
                        self.default_save_dir.display()
                    ),
                });
            }
        }
        Ok(self.default_save_dir.clone())
    }

    /// Returns a reference to the runtime configuration.
    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Return the number of agents currently live in this runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal session lock is poisoned.
    pub fn active_agent_count(&self) -> impl Future<Output = Result<usize, Error>> + Send {
        let result = self
            .sessions
            .read()
            .map(|sessions| sessions.len())
            .map_err(|e| Error::BackendError {
                message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
            });
        std::future::ready(result)
    }

    /// Return the number of active `localharness` child processes managed by this runtime.
    pub async fn active_harness_count(&self) -> usize {
        let slots: Vec<HarnessSlot> = self
            .harnesses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        let mut count = 0;
        for slot in slots {
            let mut guard = slot.lock().await;
            if let Some(ref mut entry) = *guard {
                if entry.process.is_alive() {
                    count += 1;
                } else {
                    *guard = None;
                }
            }
        }
        count
    }

    fn canonical_save_dir(save_dir: &Path) -> PathBuf {
        crate::policies::path::canonicalize_path(save_dir).unwrap_or_else(|e| {
            tracing::trace!(
                error = %e,
                path = %save_dir.display(),
                "Failed to canonicalize save_dir"
            );
            save_dir.to_path_buf()
        })
    }

    pub(crate) fn harness_cache_key(
        save_dir: &Path,
        env: &HashMap<String, String>,
    ) -> HarnessCacheKey {
        let canonical = Self::canonical_save_dir(save_dir);
        let mut sorted_env: Vec<(String, String)> =
            env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        sorted_env.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        (canonical, sorted_env)
    }

    async fn deregister_agent_from_harnesses_map(
        harnesses_map: &Mutex<HashMap<HarnessCacheKey, HarnessSlot>>,
        key: &HarnessCacheKey,
        agent_id: AgentId,
    ) {
        let slot_opt = {
            match harnesses_map.lock() {
                Ok(map) => map.get(key).cloned(),
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "Poisoned harnesses mutex in deregister_agent_from_harnesses_map"
                    );
                    None
                }
            }
        };
        if let Some(slot) = slot_opt {
            let mut guard = slot.lock().await;
            let should_kill = if let Some(ref mut entry) = *guard {
                entry.active_agents.remove(&agent_id);
                tracing::debug!(
                    agent_id = %agent_id,
                    remaining = entry.active_agents.len(),
                    "Deregistered agent from shared localharness"
                );
                entry.active_agents.is_empty()
            } else {
                false
            };
            if should_kill && let Some(mut entry) = guard.take() {
                tracing::info!(
                    agent_id = %agent_id,
                    save_dir = %key.0.display(),
                    "Last active agent shut down; terminating shared localharness process"
                );
                entry.process.kill().await;
            }
        }
    }

    fn get_or_create_harness_slot(&self, key: &HarnessCacheKey) -> Result<HarnessSlot, Error> {
        let mut map = self.harnesses.lock().map_err(|e| Error::BackendError {
            message: format!("Poisoned harnesses mutex: {e}"),
        })?;
        Ok(map
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
            .clone())
    }

    async fn get_or_spawn_harness(
        &self,
        key: &HarnessCacheKey,
        env_vars: &HashMap<String, String>,
        custom_binary_path: Option<&Path>,
        agent_id: AgentId,
        force_new: bool,
    ) -> Result<(u16, String, bool), Error> {
        let slot = self.get_or_create_harness_slot(key)?;
        let mut slot_guard = slot.lock().await;

        if !force_new
            && let Some(entry) = slot_guard.as_mut()
            && entry.process.is_alive()
        {
            entry.active_agents.insert(agent_id);
            tracing::debug!(
                agent_id = %agent_id,
                port = entry.process.port,
                active_agents = entry.active_agents.len(),
                "Reusing existing running localharness process for save_dir={}",
                key.0.display()
            );
            return Ok((entry.process.port, entry.process.api_key.clone(), true));
        }

        if let Some(mut old_entry) = slot_guard.take() {
            tracing::warn!(
                "Terminating stale localharness process for save_dir={} before respawning",
                key.0.display()
            );
            old_entry.process.kill().await;
        }

        let save_dir_owned = key.0.clone();
        let env_owned = (!env_vars.is_empty()).then(|| env_vars.clone());
        let custom_owned = custom_binary_path.map(Path::to_path_buf);
        let rt_handle = self.runtime_handle.clone();
        let process = rt_handle
            .spawn(async move {
                HarnessProcess::spawn(&save_dir_owned, env_owned.as_ref(), custom_owned.as_deref())
                    .await
            })
            .await
            .map_err(|e| Error::BackendError {
                message: format!("Harness spawn task failed: {e}"),
            })??;

        let port = process.port;
        let api_key = process.api_key.clone();
        let mut active_agents = HashSet::new();
        active_agents.insert(agent_id);
        let entry = SharedHarnessEntry {
            process,
            active_agents,
        };
        *slot_guard = Some(entry);
        Ok((port, api_key, false))
    }

    async fn connect_and_init_harness(
        &self,
        key: &HarnessCacheKey,
        custom_binary_path: Option<&Path>,
        agent_id: AgentId,
        config: &AgentConfig,
        hook_runner: Option<&Arc<Hooks>>,
        policies: &PolicySet,
    ) -> Result<(HarnessWsStream, InitialHarnessState), Error> {
        let (mut port, mut api_key, was_reused) = self
            .get_or_spawn_harness(key, &config.env, custom_binary_path, agent_id, false)
            .await?;

        let mut ws = match connect_to_harness(port, &api_key).await {
            Ok(ws) => ws,
            Err(_e) if was_reused => {
                tracing::warn!(
                    port,
                    "Failed to connect to reused localharness process; respawning a fresh instance"
                );
                let (new_port, new_key, _) = self
                    .get_or_spawn_harness(key, &config.env, custom_binary_path, agent_id, true)
                    .await?;
                port = new_port;
                api_key = new_key;
                connect_to_harness(port, &api_key).await?
            }
            Err(e) => return Err(e),
        };

        let init_state = initialize_harness(&mut ws, config, hook_runner, policies).await?;
        Ok((ws, init_state))
    }

    /// Terminate all managed localharness processes and wait for exit.
    ///
    /// # Errors
    ///
    /// Returns an error if process termination fails.
    pub async fn shutdown(&self) -> Result<(), Error> {
        let slots: Vec<HarnessSlot> = {
            let mut map = self.harnesses.lock().map_err(|e| Error::BackendError {
                message: format!("Poisoned harnesses mutex in shutdown: {e}"),
            })?;
            map.drain().map(|(_, s)| s).collect()
        };
        for slot in slots {
            let mut guard = slot.lock().await;
            if let Some(mut entry) = guard.take() {
                entry.process.kill().await;
            }
        }
        match self.default_save_dir_initialized.lock() {
            Ok(mut init_guard) => *init_guard = false,
            Err(poisoned) => *poisoned.into_inner() = false,
        }
        match std::fs::symlink_metadata(&self.default_save_dir) {
            Ok(meta) => {
                if meta.file_type().is_dir()
                    && !meta.file_type().is_symlink()
                    && let Err(e) = std::fs::remove_dir_all(&self.default_save_dir)
                {
                    tracing::debug!(
                        path = %self.default_save_dir.display(),
                        error = %e,
                        "Failed removing isolated default_save_dir during shutdown"
                    );
                }
            }
            Err(e) => {
                tracing::trace!(
                    path = %self.default_save_dir.display(),
                    error = %e,
                    "default_save_dir not present during shutdown"
                );
            }
        }
        Ok(())
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        let mut map = match self.harnesses.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut unlocked_entries = Vec::new();
        let mut contended_slots = Vec::new();
        for (_path, slot) in map.drain() {
            match slot.try_lock() {
                Ok(mut guard) => {
                    if let Some(mut entry) = guard.take() {
                        entry.process.start_kill();
                        unlocked_entries.push(entry);
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "NativeRuntime::drop: harness slot lock contended; queuing async reap"
                    );
                    contended_slots.push(Arc::clone(&slot));
                }
            }
        }
        let default_dir = self.default_save_dir.clone();
        self.runtime_handle.spawn(async move {
            for mut entry in unlocked_entries {
                entry.process.kill().await;
            }
            for slot in contended_slots {
                let mut guard = slot.lock().await;
                if let Some(mut entry) = guard.take() {
                    entry.process.kill().await;
                }
            }
            match std::fs::symlink_metadata(&default_dir) {
                Ok(meta) => {
                    if meta.file_type().is_dir()
                        && !meta.file_type().is_symlink()
                        && let Err(e) = tokio::fs::remove_dir_all(&default_dir).await
                    {
                        tracing::debug!(
                            path = %default_dir.display(),
                            error = %e,
                            "Failed removing isolated default_save_dir on NativeRuntime drop"
                        );
                    }
                }
                Err(e) => {
                    tracing::trace!(
                        path = %default_dir.display(),
                        error = %e,
                        "default_save_dir not present on NativeRuntime drop"
                    );
                }
            }
        });
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new(RuntimeConfig::default())
    }
}

#[cfg(test)]
mod tests;
