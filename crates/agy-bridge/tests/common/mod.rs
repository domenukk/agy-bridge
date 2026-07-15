//! Shared test helpers for agy-bridge integration tests.
//
// Each integration test file is a separate crate that includes this module.
// Not all test crates use every helper, so some items may appear unused.
// `if cond { panic!(...) }` is intentional in the retry harness — the format
// strings are multi-line and read better than `assert!`.

use std::sync::{Condvar, Mutex};

use fast_rands::Rand;

/// Returns the `GEMINI_API_KEY`, checking the environment first and then
/// falling back to a `.env` file in the project root.
///
/// # Panics
///
/// Panics if the key is not found in either location.
pub fn api_key() -> String {
    // NOLINT: env var not set is expected — falls through to .env file below
    if let Ok(key) = std::env::var("GEMINI_API_KEY")
        && !key.is_empty()
    {
        return key.trim_matches('"').to_string();
    }
    // Try loading from .env
    let env_map = agy_bridge::load_dotenv();
    if let Some(key) = env_map.get("GEMINI_API_KEY")
        && !key.is_empty()
    {
        return key.trim_matches('"').to_string();
    }
    // NOLINT: test helper — cwd default is fine, path is only used in the panic message
    let dotenv_path = std::env::current_dir().unwrap_or_default().join(".env");
    panic!(
        "GEMINI_API_KEY not set in environment or in {dotenv}",
        dotenv = dotenv_path.display(),
    );
}

pub fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

pub fn create_bridge() -> agy_bridge::AgyBridge {
    agy_bridge::AgyBridge::builder()
        .build()
        .expect("Failed to create bridge")
}

// ── Bounded-concurrency gate ─────────────────────────────────────────────────

/// Default number of live tests allowed to run concurrently.
///
/// This is high enough to exercise real concurrency, but low enough to stay
/// within Gemini API tokens-per-minute (TPM) limits.  Override with the
/// `AGY_BRIDGE_MAX_CONCURRENT_TESTS` environment variable.
const DEFAULT_MAX_CONCURRENT: usize = 3;

/// Maximum random stagger delay (in milliseconds) added before each test
/// starts its first API call.  Spreads initial bursts to avoid TPM spikes.
const STAGGER_MAX_MS: u64 = 2000;

/// A simple counting semaphore built on `Mutex` + `Condvar`.
///
/// Used instead of `tokio::sync::Semaphore` because the live tests use
/// synchronous `#[test]` functions (not `#[tokio::test]`).
struct CountingSemaphore {
    state: Mutex<usize>,
    cvar: Condvar,
    max_permits: usize,
}

impl CountingSemaphore {
    const fn new(max_permits: usize) -> Self {
        Self {
            state: Mutex::new(0),
            cvar: Condvar::new(),
            max_permits,
        }
    }

    /// Blocks until a permit is available, then returns a guard that releases
    /// it on drop.
    fn acquire(&self) -> SemaphoreGuard<'_> {
        let mut active = self.state.lock().unwrap_or_else(|poisoned| {
            // Recover from a poisoned lock (prior test panic) so remaining
            // tests still get a chance to run.
            poisoned.into_inner()
        });
        while *active >= self.max_permits {
            active = self
                .cvar
                .wait(active)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *active += 1;
        SemaphoreGuard { sem: self }
    }
}

struct SemaphoreGuard<'a> {
    sem: &'a CountingSemaphore,
}

impl Drop for SemaphoreGuard<'_> {
    fn drop(&mut self) {
        let mut active = self
            .sem
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active -= 1;
        self.sem.cvar.notify_one();
    }
}

/// Read the concurrency limit from the environment, falling back to the
/// compiled-in default.
fn max_concurrent_tests() -> usize {
    std::env::var("AGY_BRIDGE_MAX_CONCURRENT_TESTS")
        // NOLINT: env var not set is expected — falls through to default
        .ok()
        // NOLINT: invalid integer falls through to default concurrency limit
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(DEFAULT_MAX_CONCURRENT, |n| n.max(1))
}

/// Global concurrency gate for live integration tests.
///
/// Limits — but does **not** serialize — the number of tests hitting the
/// Gemini API concurrently.  This keeps us within tokens-per-minute (TPM)
/// quotas while still exercising real concurrent agent execution.
///
/// The static is initialised with `DEFAULT_MAX_CONCURRENT` permits; the
/// env-var override is applied at runtime via `live_gate()`.
static LIVE_GATE: CountingSemaphore = CountingSemaphore::new(DEFAULT_MAX_CONCURRENT);

/// Lazily initialised gate that respects the `AGY_BRIDGE_MAX_CONCURRENT_TESTS`
/// env-var.  Falls back to the compile-time `LIVE_GATE` when the env-var is
/// absent or equals the default.
fn live_gate() -> &'static CountingSemaphore {
    use std::sync::OnceLock;
    static CUSTOM_GATE: OnceLock<Option<CountingSemaphore>> = OnceLock::new();

    let custom = CUSTOM_GATE.get_or_init(|| {
        let n = max_concurrent_tests();
        if n == DEFAULT_MAX_CONCURRENT {
            None // use the compile-time static
        } else {
            eprintln!("[GATE] Using custom concurrency limit: {n}");
            Some(CountingSemaphore::new(n))
        }
    });

    match custom {
        Some(gate) => gate,
        None => &LIVE_GATE,
    }
}

/// A retry harness for live tests that uses the **structured [`Error`]** enum
/// instead of scraping panic messages.
///
/// Test functions return `Result<(), Error>`. The harness retries on
/// [`Error::is_retryable()`] errors (connection, quota, 503 backend) plus
/// timeouts and transient lifecycle errors (`event loop is closed`,
/// `CancelledError`).
///
/// Concurrency is bounded (not serialised) by a counting semaphore so that
/// multiple tests can run in parallel without overwhelming the API's
/// tokens-per-minute quota.  A small random stagger delay is added before
/// each test's first API call to spread initial bursts.
///
/// # Panics
///
/// Panics if the test function returns a non-retryable error, or if the
/// retry budget is exhausted while retryable errors keep occurring.
pub fn run_live_test<F>(test_name: &str, f: F)
where
    F: Fn() -> Result<(), agy_bridge::error::Error>,
{
    if std::env::var("AGY_BRIDGE_SKIP_LIVE_TESTS").is_ok() {
        eprintln!("[SKIP] '{test_name}' skipped (AGY_BRIDGE_SKIP_LIVE_TESTS is set)");
        return;
    }

    // Acquire a concurrency permit *before* the budget timer starts so that
    // queued tests do not exhaust their retry window while waiting.
    let _permit = live_gate().acquire();

    eprintln!("[GATE] '{test_name}' acquired live-test permit");

    // Stagger: sleep a small random duration to spread API bursts across
    // concurrent tests, reducing the chance of hitting TPM limits.
    let max_stagger =
        usize::try_from(STAGGER_MAX_MS).expect("STAGGER_MAX_MS constant fits in usize");
    let stagger =
        std::time::Duration::from_millis(fast_rands::StdRand::new().between(0, max_stagger) as u64);
    eprintln!(
        "[STAGGER] '{test_name}' waiting {}ms before starting",
        stagger.as_millis()
    );
    std::thread::sleep(stagger);

    let start = std::time::Instant::now();
    let budget = std::time::Duration::from_mins(5);
    let mut sleep_duration = std::time::Duration::from_secs(5);
    let mut attempt = 1;

    loop {
        match f() {
            Ok(()) => return,
            Err(ref e) if is_retryable_error(e) => {
                let elapsed = start.elapsed();
                assert!(
                    elapsed < budget,
                    "Test '{test_name}' failed: budget exhausted on attempt {attempt} \
                     with retryable error: {e}"
                );

                let remaining = budget.saturating_sub(elapsed);
                assert!(
                    !remaining.is_zero(),
                    "Test '{test_name}' failed: budget exhausted on attempt {attempt} \
                     with retryable error: {e}"
                );

                let current_sleep = std::cmp::min(sleep_duration, remaining);
                eprintln!(
                    "[RETRY] Test '{test_name}' failed on attempt {attempt} with retryable error: {e}\n\
                     Waiting {}s before retry... (Elapsed: {elapsed:?}, Remaining budget: {remaining:?})",
                    current_sleep.as_secs()
                );

                std::thread::sleep(current_sleep);

                // Update sleep duration for the next attempt: double up to 15s max
                sleep_duration =
                    std::cmp::min(sleep_duration * 2, std::time::Duration::from_secs(15));
                attempt += 1;
            }
            Err(e) => {
                // Non-retryable error: fail immediately
                panic!("[FAILURE] Test '{test_name}' failed with non-retryable error: {e}");
            }
        }
    }
}

/// Determine whether an error is retryable in the context of live tests.
///
/// Uses the structured [`Error`] enum instead of string-matching:
/// - [`Error::is_retryable()`] covers `ConnectionError`, `QuotaExceeded`, 503 backend errors.
/// - [`Error::Timeout`] is retryable (transient API slowness).
/// - [`Error::Stream`] is retryable (WebSocket/connection drops).
/// - Backend errors mentioning event-loop lifecycle issues are retryable
///   (race between parallel test runtime teardown/creation).
fn is_retryable_error(err: &agy_bridge::error::Error) -> bool {
    use agy_bridge::error::Error;

    if err.is_retryable() {
        return true;
    }

    match err {
        Error::Timeout { .. } | Error::Stream(_) => true,
        Error::BackendError { message } => {
            let msg_lower = message.to_lowercase();
            // Transient Python runtime lifecycle races when multiple test
            // runtimes are created/destroyed in the same process.
            msg_lower.contains("event loop is closed") || msg_lower.contains("cancellederror")
        }
        _ => false,
    }
}
