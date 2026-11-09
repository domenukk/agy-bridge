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
    sync::{Arc, Mutex, RwLock},
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

/// Running local harness instance with reference counting of active agents.
struct SharedHarnessEntry {
    process: HarnessProcess,
    active_agents: HashSet<AgentId>,
}

/// Per-`save_dir` harness slot protected by its own async mutex so concurrent
/// agent creations for different directories never block each other.
type HarnessSlot = Arc<tokio::sync::Mutex<Option<SharedHarnessEntry>>>;

fn native_bg_runtime() -> &'static tokio::runtime::Runtime {
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
    harnesses: Arc<Mutex<HashMap<PathBuf, HarnessSlot>>>,
    runtime_handle: tokio::runtime::Handle,
    config: RuntimeConfig,
}

impl NativeRuntime {
    /// Create a new native runtime instance.
    ///
    /// # Panics
    ///
    /// Panics if the shared multi-threaded Tokio runtime cannot be initialized.
    #[must_use]
    pub fn new(config: RuntimeConfig) -> Self {
        let runtime_handle = native_bg_runtime().handle().clone();
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            harnesses: Arc::new(Mutex::new(HashMap::new())),
            runtime_handle,
            config,
        }
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

    async fn deregister_agent_from_harnesses_map(
        harnesses_map: &Mutex<HashMap<PathBuf, HarnessSlot>>,
        save_dir: &Path,
        agent_id: AgentId,
    ) {
        let key = Self::canonical_save_dir(save_dir);
        let slot_opt = {
            match harnesses_map.lock() {
                Ok(map) => map.get(&key).cloned(),
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
                    save_dir = %key.display(),
                    "Last active agent shut down; terminating shared localharness process"
                );
                entry.process.kill().await;
            }
        }
    }

    fn get_or_create_harness_slot(&self, key: &Path) -> Result<HarnessSlot, Error> {
        let mut map = self.harnesses.lock().map_err(|e| Error::BackendError {
            message: format!("Poisoned harnesses mutex: {e}"),
        })?;
        Ok(map
            .entry(key.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
            .clone())
    }

    async fn get_or_spawn_harness(
        &self,
        save_dir: &Path,
        custom_binary_path: Option<&Path>,
        agent_id: AgentId,
        force_new: bool,
    ) -> Result<(u16, String, bool), Error> {
        let key = Self::canonical_save_dir(save_dir);
        let slot = self.get_or_create_harness_slot(&key)?;
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
                key.display()
            );
            return Ok((entry.process.port, entry.process.api_key.clone(), true));
        }

        if let Some(mut old_entry) = slot_guard.take() {
            tracing::warn!(
                "Terminating stale localharness process for save_dir={} before respawning",
                key.display()
            );
            old_entry.process.start_kill();
        }

        // Spawn child process on NativeRuntime's dedicated long-lived Tokio runtime
        // so that dropping a caller's temporary Tokio runtime never kills localharness.
        let save_dir_owned = key.clone();
        let custom_owned = custom_binary_path.map(Path::to_path_buf);
        let rt_handle = self.runtime_handle.clone();
        let process = rt_handle
            .spawn(async move {
                HarnessProcess::spawn(&save_dir_owned, None, custom_owned.as_deref()).await
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
        save_dir: &Path,
        custom_binary_path: Option<&Path>,
        agent_id: AgentId,
        config: &AgentConfig,
        hook_runner: Option<&Arc<Hooks>>,
        policies: &PolicySet,
    ) -> Result<(HarnessWsStream, InitialHarnessState), Error> {
        let (mut port, mut api_key, was_reused) = self
            .get_or_spawn_harness(save_dir, custom_binary_path, agent_id, false)
            .await?;

        let mut ws = match connect_to_harness(port, &api_key).await {
            Ok(ws) => ws,
            Err(_e) if was_reused => {
                tracing::warn!(
                    port,
                    "Failed to connect to reused localharness process; respawning a fresh instance"
                );
                let (new_port, new_key, _) = self
                    .get_or_spawn_harness(save_dir, custom_binary_path, agent_id, true)
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
        Ok(())
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        let mut map = match self.harnesses.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for (_path, slot) in map.drain() {
            match slot.try_lock() {
                Ok(mut guard) => {
                    if let Some(mut entry) = guard.take() {
                        entry.process.start_kill();
                        self.runtime_handle.spawn(async move {
                            entry.process.kill().await;
                        });
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "NativeRuntime::drop: harness slot lock contended");
                }
            }
        }
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new(RuntimeConfig::default())
    }
}

#[cfg(test)]
mod tests;
