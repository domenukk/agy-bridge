//! Shared test helpers for agy-bridge integration tests.
//
// Each integration test file is a separate crate that includes this module.
// Not all test crates use every helper, so suppress dead_code warnings.
#![allow(dead_code)]
#![allow(clippy::if_then_some_else_none, clippy::missing_panics_doc)]
// `if cond { panic!(...) }` is intentional in the retry harness — the format
// strings are multi-line and read better than `assert!`.

/// Returns the `GEMINI_API_KEY`, checking the environment first and then
/// falling back to a `.env` file in the project root.
///
/// # Panics
///
/// Panics if the key is not found in either location.
pub fn api_key() -> String {
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
    let dotenv_path = std::env::current_dir().unwrap_or_default().join(".env");
    panic!(
        "GEMINI_API_KEY not set in environment or in {dotenv}",
        dotenv = dotenv_path.display(),
    );
}

/// A panic-safe exponential backoff retry harness for live tests.
///
/// It catches panics, detects if they are caused by transient Gemini API states,
/// retries with exponential backoff (5s initial, doubling up to 15s max sleep),
/// within a total try window budget of 60 seconds.
///
/// If the budget is exhausted, it panics with a diagnostic message.
///
/// # Panics
///
/// Panics if the test function `f` panics with a non-transient error, or if the
/// retry budget is exhausted while transient errors keep occurring.
pub fn run_live_test<F>(test_name: &str, f: F)
where
    F: Fn() + std::panic::RefUnwindSafe + std::panic::UnwindSafe,
{
    let start = std::time::Instant::now();
    let budget = std::time::Duration::from_mins(3);
    let mut sleep_duration = std::time::Duration::from_secs(5);
    let mut attempt = 1;

    loop {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&f));
        match result {
            Ok(()) => return,
            Err(e) => {
                let elapsed = start.elapsed();
                let panic_msg = if let Some(s) = e.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "<non-string panic payload>".to_string()
                };

                if is_transient_error(&panic_msg) {
                    assert!(
                        elapsed < budget,
                        "Test '{test_name}' failed: budget exhausted on attempt {attempt} with transient error: {panic_msg}"
                    );

                    let remaining = budget.saturating_sub(elapsed);
                    assert!(
                        !remaining.is_zero(),
                        "Test '{test_name}' failed: budget exhausted on attempt {attempt} with transient error: {panic_msg}"
                    );

                    let current_sleep = std::cmp::min(sleep_duration, remaining);
                    eprintln!(
                        "[RETRY] Test '{test_name}' failed on attempt {attempt} with transient error: {panic_msg}\n\
                         Waiting {}s before retry... (Elapsed: {elapsed:?}, Remaining budget: {remaining:?})",
                        current_sleep.as_secs()
                    );

                    std::thread::sleep(current_sleep);

                    // Update sleep duration for the next attempt: double up to 15s max
                    sleep_duration =
                        std::cmp::min(sleep_duration * 2, std::time::Duration::from_secs(15));
                    attempt += 1;
                } else {
                    // Non-transient panic: propagate immediately
                    eprintln!(
                        "[FAILURE] Test '{test_name}' failed with non-transient panic: {panic_msg}"
                    );
                    std::panic::resume_unwind(e);
                }
            }
        }
    }
}

fn is_transient_error(msg: &str) -> bool {
    let msg_lower = msg.to_lowercase();
    msg_lower.contains("quotaexceeded")
        || msg_lower.contains("quota exceeded")
        || msg_lower.contains("quota")
        || msg_lower.contains("connectionerror")
        || msg_lower.contains("503")
        || msg_lower.contains("429")
        || msg_lower.contains("resource_exhausted")
        || msg_lower.contains("rate-limit")
        || msg_lower.contains("rate limit")
        || msg_lower.contains("overloaded")
        || msg_lower.contains("timeout")
        || msg_lower.contains("timed out")
        || msg_lower.contains("timedout")
        || msg_lower.contains("deadline")
        || msg_lower.contains("streamerror")
        || msg_lower.contains("stream error")
        || msg_lower.contains("unreachable")
        || msg_lower.contains("unavailable")
        || msg_lower.contains("expected usage metadata")
}
