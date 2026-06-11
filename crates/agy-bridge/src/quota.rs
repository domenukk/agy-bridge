//! Quota tracking and exponential backoff with jitter.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use fast_rands::Rand;

/// Maximum number of per-key quota entries before pruning idle ones.
const MAX_QUOTA_ENTRIES: usize = 100;

/// Minimum per-agent jitter added to backoff, in milliseconds.
const MIN_PER_AGENT_JITTER_MS: u64 = 500;
/// Maximum per-agent jitter added to backoff, in milliseconds.
const MAX_PER_AGENT_JITTER_MS: u64 = 2000;

/// Per-runtime registry of per-API-key [`QuotaState`] instances.
///
/// Each [`PythonRuntime`](crate::runtime::PythonRuntime) owns its own
/// `QuotaRegistry`, so different runtime instances have fully independent
/// quota tracking. Agents within the same runtime that share an API key
/// share a single [`QuotaState`] — a 429 on key A only backs off agents
/// using key A within that runtime.
#[derive(Debug, Default)]
pub struct QuotaRegistry {
    inner: RwLock<HashMap<String, Arc<QuotaState>>>,
}

impl QuotaRegistry {
    /// Create a new empty quota registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a [`QuotaState`] for the given API key.
    ///
    /// Agents sharing the same key share backoff state. Agents with different
    /// keys are fully independent.
    ///
    /// When the registry exceeds `MAX_QUOTA_ENTRIES`, idle entries (those with
    /// zero consecutive 429s) are pruned to prevent unbounded growth from API
    /// key rotation.
    #[must_use]
    pub fn state_for_key(&self, key: &str) -> Arc<QuotaState> {
        // Fast path: read lock.
        if let Ok(map) = self.inner.read()
            && let Some(state) = map.get(key)
        {
            return Arc::clone(state);
        }
        // Slow path: write lock.
        let mut map = self.inner.write().unwrap_or_else(|e| {
            tracing::error!("QuotaRegistry poisoned: {e}");
            e.into_inner()
        });

        // Prune idle entries if the map is growing too large.
        if map.len() >= MAX_QUOTA_ENTRIES {
            let idle_keys: Vec<String> = map
                .iter()
                .filter(|(_, v)| v.consecutive_429_count() == 0)
                .map(|(k, _)| k.clone())
                .collect();
            for k in idle_keys {
                map.remove(&k);
            }
            if map.len() >= MAX_QUOTA_ENTRIES {
                tracing::warn!(
                    entries = map.len(),
                    "Quota registry still at capacity after pruning idle entries"
                );
            }
        }

        Arc::clone(
            map.entry(key.to_owned())
                .or_insert_with(|| Arc::new(QuotaState::new())),
        )
    }
}

/// Inner state protected by a single mutex to ensure atomicity of
/// counter increment + deadline computation.
///
/// Uses `std::sync::Mutex` (not `tokio::sync::Mutex`) because the lock is
/// held only for brief counter reads/writes and is always dropped before
/// any `.await` point (see `wait_for_quota`). This avoids async mutex
/// overhead and is deadlock-free.
#[derive(Debug)]
struct QuotaInner {
    /// Number of consecutive 429 responses observed.
    consecutive_429s: u32,
    /// Don't issue new requests until this instant.
    backoff_until: Option<tokio::time::Instant>,
}

/// Tracks consecutive HTTP 429 (quota exceeded) events and computes backoff.
#[derive(Debug)]
pub struct QuotaState {
    inner: Mutex<QuotaInner>,
}

impl Default for QuotaState {
    fn default() -> Self {
        Self::new()
    }
}

impl QuotaState {
    /// Create a new `QuotaState` with no recorded errors.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(QuotaInner {
                consecutive_429s: 0,
                backoff_until: None,
            }),
        }
    }

    /// Record a quota hit and compute the next backoff deadline.
    ///
    /// `retry_after` is the server-suggested wait duration. The actual backoff
    /// is `max(retry_after, exponential_backoff(consecutive_count))` plus jitter.
    ///
    /// If the internal mutex is poisoned (e.g. a previous panic while the lock
    /// was held), the hit is logged but silently dropped — this is best-effort.
    pub fn record_quota_hit(&self, retry_after: Duration) {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "QuotaState mutex poisoned in record_quota_hit — skipping"
                );
                return;
            }
        };
        inner.consecutive_429s += 1;
        let count = inner.consecutive_429s;
        let now = tokio::time::Instant::now();

        let exp_backoff = exponential_backoff_with_jitter(count);
        let effective_backoff = retry_after.max(exp_backoff);

        tracing::warn!(
            consecutive_429s = count,
            retry_after_ms = u64::try_from(retry_after.as_millis()).unwrap_or_else(|e| {
                tracing::warn!("Int conversion failed: {}", e);
                u64::MAX
            }),
            effective_backoff_ms =
                u64::try_from(effective_backoff.as_millis()).unwrap_or_else(|e| {
                    tracing::warn!("Int conversion failed: {}", e);
                    u64::MAX
                }),
            "Quota hit — backing off"
        );

        inner.backoff_until = Some(now + effective_backoff);
    }

    /// Wait until any active backoff period has elapsed.
    ///
    /// All operations should call this before proceeding to respect shared
    /// quota state across agents.
    ///
    /// If the internal mutex is poisoned, logs an error and falls through
    /// immediately (operations proceed without backoff rather than panicking).
    pub async fn wait_for_quota(&self) {
        let (deadline, count) = {
            let inner = match self.inner.lock() {
                Ok(guard) => guard,
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "QuotaState mutex poisoned in wait_for_quota — proceeding without backoff"
                    );
                    return;
                }
            };
            (inner.backoff_until, inner.consecutive_429s)
        };
        if let Some(until) = deadline {
            let now = tokio::time::Instant::now();
            if until > now {
                let wait = until - now;
                // Per-call jitter so agents sharing the same deadline don't
                // wake simultaneously (thundering-herd mitigation).
                let min_val = usize::try_from(MIN_PER_AGENT_JITTER_MS).unwrap_or(usize::MAX);
                let max_val = usize::try_from(MAX_PER_AGENT_JITTER_MS).unwrap_or(usize::MAX);
                let per_agent_jitter = Duration::from_millis(
                    fast_rands::StdRand::new().between(min_val, max_val) as u64,
                );
                let total_wait = wait + per_agent_jitter;
                tracing::warn!(
                    wait_ms = u64::try_from(total_wait.as_millis()).unwrap_or_else(|e| {
                        tracing::warn!("Int conversion failed: {}", e);
                        u64::MAX
                    }),
                    consecutive_429s = count,
                    "Quota backoff — waiting"
                );
                tokio::time::sleep(total_wait).await;
                tracing::info!("Quota backoff complete — resuming operations");
            }
        }
    }

    /// Record a successful operation, resetting the consecutive 429 counter.
    ///
    /// If the internal mutex is poisoned, logs an error and skips the reset
    /// (best-effort — the counter may remain stale but we do not panic).
    pub fn record_success(&self) {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "QuotaState mutex poisoned in record_success — skipping reset"
                );
                return;
            }
        };
        if inner.consecutive_429s > 0 {
            tracing::info!(
                previous_consecutive_429s = inner.consecutive_429s,
                "Quota state reset after successful operation"
            );
            inner.consecutive_429s = 0;
            inner.backoff_until = None;
        }
    }

    /// Return the current number of consecutive 429 errors.
    ///
    /// Returns `0` if the internal mutex is poisoned (logged as an error).
    #[must_use]
    pub fn consecutive_429_count(&self) -> u32 {
        match self.inner.lock() {
            Ok(guard) => guard.consecutive_429s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "QuotaState mutex poisoned in consecutive_429_count — returning 0"
                );
                0
            }
        }
    }
}

use crate::error::MAX_BACKOFF_SECS;
/// Maximum jitter added to backoff, in milliseconds.
const MAX_JITTER_MS: u64 = 5000;

/// Return a random jitter value in the range `[0, MAX_JITTER_MS)`.
///
/// Uses `fastrand` for non-deterministic jitter, preventing thundering-herd
/// effects when multiple clients back off from the same attempt count.
fn jitter_ms() -> u64 {
    let max_val = usize::try_from(MAX_JITTER_MS - 1).unwrap_or(usize::MAX);
    fast_rands::StdRand::new().between(0, max_val) as u64
}

/// Compute exponential backoff with jitter: `min(2^n, MAX_BACKOFF_SECS)` seconds + jitter.
///
/// Progression: attempt 1→2s, 2→4s, 3→8s, 4→16s, 5→32s, 6→64s, 7+→120s.
/// Jitter is a non-deterministic random value in `[0, MAX_JITTER_MS)` ms,
/// added on top of the capped exponential base to avoid thundering herd.
fn exponential_backoff_with_jitter(attempt: u32) -> Duration {
    let base_secs = 2u64
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(MAX_BACKOFF_SECS);
    let capped_secs = base_secs.min(MAX_BACKOFF_SECS);
    Duration::from_millis(capped_secs * 1000 + jitter_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_quota_state_records_and_resets() {
        let state = QuotaState::new();
        assert_eq!(state.consecutive_429_count(), 0);

        state.record_quota_hit(Duration::from_millis(10));
        assert_eq!(state.consecutive_429_count(), 1);

        state.record_quota_hit(Duration::from_millis(10));
        assert_eq!(state.consecutive_429_count(), 2);

        state.record_success();
        assert_eq!(state.consecutive_429_count(), 0);
    }

    #[tokio::test]
    async fn test_wait_for_quota_returns_immediately_when_no_backoff() {
        let state = QuotaState::new();
        // Should return immediately — no backoff recorded.
        let start = tokio::time::Instant::now();
        state.wait_for_quota().await;
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_backoff_timing() {
        tokio::time::pause();
        let state = QuotaState::new();

        // Record a hit with a very short retry_after. The exponential backoff
        // for attempt 1 is 2s + jitter, so backoff_until will be ~2.5s from now.
        state.record_quota_hit(Duration::from_millis(1));

        // Verify backoff_until is set.
        assert!(state.inner.lock().unwrap().backoff_until.is_some());

        // Advance time past the backoff (base 2s + up to 5s jitter + up to 3s per-agent jitter).
        tokio::time::advance(Duration::from_secs(11)).await;
        state.wait_for_quota().await;
        // Should have completed without hanging.
    }

    #[tokio::test]
    async fn test_exponential_backoff_progression() {
        // Verify the exponential progression: 2→4→8→16→32→64→120 (capped)
        let d1 = exponential_backoff_with_jitter(1);
        let d2 = exponential_backoff_with_jitter(2);
        let d3 = exponential_backoff_with_jitter(3);
        let d7 = exponential_backoff_with_jitter(7);

        // Base: 2s, 4s, 8s, ... capped at 120s. Plus jitter (< 5000ms).
        assert!(d1 >= Duration::from_secs(2) && d1 < Duration::from_secs(7));
        assert!(d2 >= Duration::from_secs(4) && d2 < Duration::from_secs(9));
        assert!(d3 >= Duration::from_secs(8) && d3 < Duration::from_secs(13));
        assert!(d7 >= Duration::from_mins(2) && d7 < Duration::from_secs(125));
    }

    #[tokio::test]
    async fn test_multiple_agents_respect_shared_quota_state() {
        tokio::time::pause();
        let state = Arc::new(QuotaState::new());

        // Agent 1 records a quota hit.
        state.record_quota_hit(Duration::from_millis(100));
        assert_eq!(state.consecutive_429_count(), 1);

        // Agent 2 sees the same state — should observe backoff.
        assert!(state.inner.lock().unwrap().backoff_until.is_some());

        // Agent 2 records another hit — counter should increment.
        state.record_quota_hit(Duration::from_millis(100));
        assert_eq!(state.consecutive_429_count(), 2);

        // Advance past all backoffs
        tokio::time::advance(Duration::from_mins(2)).await;

        // Agent 1 records success — resets for everyone.
        state.record_success();
        assert_eq!(state.consecutive_429_count(), 0);
    }

    #[test]
    fn test_quota_state_default() {
        let state = QuotaState::default();
        assert_eq!(state.consecutive_429_count(), 0);
    }

    #[tokio::test]
    async fn test_double_success_reset_is_idempotent() {
        let state = QuotaState::new();
        state.record_quota_hit(Duration::from_millis(10));
        assert_eq!(state.consecutive_429_count(), 1);

        state.record_success();
        assert_eq!(state.consecutive_429_count(), 0);

        // Second success should be a no-op
        state.record_success();
        assert_eq!(state.consecutive_429_count(), 0);
    }

    #[test]
    fn test_jitter_is_nondeterministic_for_same_attempt() {
        // Call jitter_ms 10 times with the same context; at least 2 distinct
        // values should appear (statistical: probability of all-same ≈ 0).
        let values: Vec<u64> = (0..10).map(|_i| jitter_ms()).collect();
        let distinct: std::collections::HashSet<u64> = values.iter().copied().collect();
        assert!(
            distinct.len() >= 2,
            "Expected at least 2 distinct jitter values, got {values:?}"
        );
    }

    #[test]
    fn test_jitter_bounded() {
        // Every jitter value must be in [0, MAX_JITTER_MS)
        for _ in 0..100 {
            let j = jitter_ms();
            assert!(j < MAX_JITTER_MS, "jitter {j} should be < {MAX_JITTER_MS}");
        }
    }

    #[test]
    fn test_backoff_with_jitter_bounded() {
        // Full backoff duration must be within [base, base + MAX_JITTER_MS)
        for attempt in 1..=20 {
            let d = exponential_backoff_with_jitter(attempt);
            let base_secs = 2u64
                .checked_shl(attempt.saturating_sub(1))
                .unwrap_or(MAX_BACKOFF_SECS)
                .min(MAX_BACKOFF_SECS);
            let base = Duration::from_secs(base_secs);
            let max_with_jitter = Duration::from_millis(base_secs * 1000 + MAX_JITTER_MS);
            assert!(
                d >= base && d < max_with_jitter,
                "attempt {attempt}: {d:?} not in [{base:?}, {max_with_jitter:?})"
            );
        }
    }

    #[tokio::test]
    async fn test_quota_counter_increases_monotonically() {
        let state = QuotaState::new();
        for expected in 1..=5 {
            state.record_quota_hit(Duration::from_millis(1));
            assert_eq!(state.consecutive_429_count(), expected);
        }
    }

    #[tokio::test]
    async fn test_record_quota_hit_uses_max_of_retry_and_exponential() {
        tokio::time::pause();
        let state = QuotaState::new();

        // With a very large retry_after, it should dominate
        let large_retry = Duration::from_mins(5);
        state.record_quota_hit(large_retry);

        let until = {
            let inner = state.inner.lock().unwrap();
            assert!(inner.backoff_until.is_some());
            inner.backoff_until.unwrap()
        };
        let now = tokio::time::Instant::now();
        // The backoff should be at least the large retry_after
        assert!(until >= now + Duration::from_secs(290));
    }

    /// Stress test §7.3.2: 10 concurrent tasks each fire 5 quota hits,
    /// verify the counter reaches 50 exactly, backoff deadlines advance,
    /// and a single `record_success` resets the entire shared state.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stress_quota_storm_concurrent_429s() {
        let state = Arc::new(QuotaState::new());
        let tasks: u32 = 10;
        let hits_per_task: u32 = 5;

        let mut handles = Vec::new();
        for _ in 0..tasks {
            let qs = Arc::clone(&state);
            handles.push(tokio::spawn(async move {
                for _ in 0..hits_per_task {
                    qs.record_quota_hit(Duration::from_millis(10));
                }
            }));
        }

        for h in handles {
            h.await.expect("task should complete");
        }

        // All tasks recorded their hits — counter should be exactly tasks * hits_per_task.
        assert_eq!(state.consecutive_429_count(), tasks * hits_per_task);

        // Backoff deadline should be set.
        assert!(
            state.inner.lock().unwrap().backoff_until.is_some(),
            "backoff_until should be set after storm"
        );

        // A single success resets the global counter for all agents.
        state.record_success();
        assert_eq!(state.consecutive_429_count(), 0);

        assert!(
            state.inner.lock().unwrap().backoff_until.is_none(),
            "backoff_until should be cleared after success"
        );
    }

    /// Verify behavior when quota is exhausted (many consecutive 429s):
    /// backoff duration caps at `MAX_BACKOFF_SECS` and does not overflow.
    #[tokio::test]
    async fn test_quota_exhaustion_backoff_caps() {
        tokio::time::pause();
        let state = QuotaState::new();

        // Simulate 20 consecutive 429 hits — well past the cap.
        let hit_count = 20u32;
        for _ in 0..hit_count {
            state.record_quota_hit(Duration::from_millis(1));
        }

        assert_eq!(state.consecutive_429_count(), hit_count);

        // The backoff should be set and capped at MAX_BACKOFF_SECS + jitter.
        let until = {
            let inner = state.inner.lock().unwrap();
            assert!(
                inner.backoff_until.is_some(),
                "backoff_until should be set after many hits"
            );
            inner.backoff_until.unwrap()
        };

        let now = tokio::time::Instant::now();
        let backoff_duration = until - now;
        // Backoff should be capped: at most MAX_BACKOFF_SECS + MAX_JITTER_MS.
        let max_allowed = Duration::from_millis(MAX_BACKOFF_SECS * 1000 + MAX_JITTER_MS);
        assert!(
            backoff_duration <= max_allowed,
            "backoff {backoff_duration:?} exceeds cap {max_allowed:?}"
        );

        // Advance past the backoff and verify wait_for_quota completes.
        tokio::time::advance(Duration::from_secs(MAX_BACKOFF_SECS + 1)).await;
        state.wait_for_quota().await;
        // Should not hang.

        // Success resets the counter even after exhaustion.
        state.record_success();
        assert_eq!(state.consecutive_429_count(), 0);
    }

    // ── QuotaRegistry tests ────────────────────────────────────────

    #[test]
    fn same_key_returns_same_quota_state() {
        let registry = QuotaRegistry::new();
        let a = registry.state_for_key("test-key-same");
        let b = registry.state_for_key("test-key-same");
        assert!(Arc::ptr_eq(&a, &b), "Same key should return the same Arc");
    }

    #[test]
    fn different_keys_return_independent_quota_states() {
        let registry = QuotaRegistry::new();
        let a = registry.state_for_key("test-key-alpha");
        let b = registry.state_for_key("test-key-beta");
        assert!(
            !Arc::ptr_eq(&a, &b),
            "Different keys should return different Arcs"
        );
    }

    #[test]
    fn different_keys_have_independent_backoff() {
        let registry = QuotaRegistry::new();
        let a = registry.state_for_key("test-key-independent-a");
        let b = registry.state_for_key("test-key-independent-b");

        // Hit quota on key A.
        a.record_quota_hit(Duration::from_secs(10));
        assert!(a.consecutive_429_count() > 0);

        // Key B should be unaffected.
        assert_eq!(b.consecutive_429_count(), 0);
    }

    #[test]
    fn different_registries_are_fully_independent() {
        let registry_a = QuotaRegistry::new();
        let registry_b = QuotaRegistry::new();

        let state_a = registry_a.state_for_key("shared-key");
        let state_b = registry_b.state_for_key("shared-key");

        // Same key, different registries → different Arc instances.
        assert!(
            !Arc::ptr_eq(&state_a, &state_b),
            "Different registries should not share state even for the same key"
        );

        // 429 on registry A should not affect registry B.
        state_a.record_quota_hit(Duration::from_secs(10));
        assert!(state_a.consecutive_429_count() > 0);
        assert_eq!(state_b.consecutive_429_count(), 0);
    }
}
