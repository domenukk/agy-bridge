use std::sync::atomic::Ordering;

use super::{AgentId, dedup_model_text, format_error_message, forward_step_to_writer};
use crate::types::{Step, StepSource, StepStatus};

/// Helper: create a step with given status and error field.
fn step_with(status: StepStatus, error: &str, content: &str) -> Step {
    Step {
        status,
        error: error.to_string(),
        content: content.to_string(),
        ..Step::default()
    }
}

// ── model-text de-duplication tests ──────────────────────────────
//
// The SDK emits, per turn, streaming delta steps *and* a consolidated
// "complete response" step that repeats the full text. Forwarding both
// doubled the response under concurrency (a load-dependent flake). These
// tests pin the reconciliation done by `forward_text` / `dedup_model_text`.

/// Build a MODEL step carrying an incremental delta.
fn model_delta(delta: &str) -> Step {
    Step {
        source: StepSource::Model,
        target: crate::types::StepTarget::User,
        content_delta: delta.to_string(),
        ..Step::default()
    }
}

/// Build a MODEL "complete response" step carrying full content.
fn model_complete(content: &str) -> Step {
    Step {
        source: StepSource::Model,
        target: crate::types::StepTarget::User,
        content: content.to_string(),
        is_complete_response: Some(true),
        ..Step::default()
    }
}

/// Forward all steps through one accumulator and drain the text channel.
async fn text_of(steps: Vec<Step>) -> String {
    let (writer, handle) = crate::streaming::channel();
    writer.subs.text.store(true, Ordering::Release);
    let mut streamed = String::new();
    for step in steps {
        forward_step_to_writer(&writer, step, AgentId(1), &mut streamed).await;
    }
    drop(writer);
    handle.text().await.expect("text drains cleanly").into()
}

/// Regression: a delta step followed by a consolidated complete-response
/// step repeating the same text must yield the text exactly once — this is
/// the exact doubling observed under concurrent load.
#[tokio::test]
async fn consolidated_complete_response_is_not_double_emitted() {
    let text = text_of(vec![
        model_delta("Healthy mock response"),
        model_complete("Healthy mock response"),
    ])
    .await;
    assert_eq!(text, "Healthy mock response");
}

/// Incremental deltas concatenate, and the trailing consolidation that
/// repeats their sum is dropped.
#[tokio::test]
async fn incremental_deltas_concatenate_once() {
    let text = text_of(vec![
        model_delta("Heal"),
        model_delta("thy "),
        model_delta("mock "),
        model_delta("response"),
        model_complete("Healthy mock response"),
    ])
    .await;
    assert_eq!(text, "Healthy mock response");
}

/// A non-streaming turn (content only, no deltas) is emitted once.
#[tokio::test]
async fn non_streaming_single_content_step_emitted_once() {
    let text = text_of(vec![model_complete("Only once")]).await;
    assert_eq!(text, "Only once");
}

/// Two separate model messages, each delta + consolidation, must each be
/// emitted once — the accumulator resets at the message boundary.
#[tokio::test]
async fn two_messages_each_emitted_once() {
    let text = text_of(vec![
        model_delta("one"),
        model_complete("one"),
        model_delta("two"),
        model_complete("two"),
    ])
    .await;
    assert_eq!(text, "onetwo");
}

/// SDK fidelity (`receive_chunks`): model text NOT targeted at the user
/// (e.g. a subagent responding to the orchestrator, target != USER) must
/// never appear in the primary text stream.
#[tokio::test]
async fn non_user_target_model_text_is_not_streamed() {
    let subagent_step = Step {
        source: StepSource::Model,
        target: crate::types::StepTarget::Model,
        content: "internal subagent chatter".to_string(),
        is_complete_response: Some(true),
        ..Step::default()
    };
    let text = text_of(vec![subagent_step]).await;
    assert_eq!(text, "", "non-user-target model text must be filtered out");
}

/// SDK fidelity (`receive_chunks`): text from a non-model source (user
/// echo, system message) is not part of the primary text stream.
#[tokio::test]
async fn non_model_source_text_is_not_streamed() {
    let system_step = Step {
        source: StepSource::System,
        target: crate::types::StepTarget::User,
        content: "system notice".to_string(),
        ..Step::default()
    };
    let text = text_of(vec![system_step]).await;
    assert_eq!(text, "", "non-model text must be filtered out");
}

#[test]
fn dedup_model_text_skips_exact_consolidation() {
    let mut s = String::new();
    assert_eq!(
        dedup_model_text("abc".to_owned(), true, &mut s),
        Some("abc".to_owned())
    );
    assert_eq!(dedup_model_text("abc".to_owned(), false, &mut s), None);
}

#[test]
fn dedup_model_text_trims_grown_snapshot() {
    let mut s = String::new();
    assert_eq!(
        dedup_model_text("ab".to_owned(), true, &mut s),
        Some("ab".to_owned())
    );
    assert_eq!(
        dedup_model_text("abcd".to_owned(), false, &mut s),
        Some("cd".to_owned())
    );
}

#[test]
fn dedup_model_text_non_streaming_emits_content() {
    let mut s = String::new();
    assert_eq!(
        dedup_model_text("full".to_owned(), false, &mut s),
        Some("full".to_owned())
    );
}

// ── Error detection tests ────────────────────────────────────────

#[test]
fn error_status_is_detected() {
    let step = step_with(StepStatus::Error, "", "some content");
    assert_eq!(step.status, StepStatus::Error);
    assert!(step.error.is_empty());
    let has_error_status = step.status == StepStatus::Error;
    let has_error_field = !step.error.is_empty();
    assert!(has_error_status || has_error_field);
}

#[test]
fn error_field_is_detected() {
    let step = step_with(StepStatus::Done, "quota exceeded", "");
    let has_error_status = step.status == StepStatus::Error;
    let has_error_field = !step.error.is_empty();
    assert!(has_error_status || has_error_field);
}

#[test]
fn both_error_signals_detected() {
    let step = step_with(StepStatus::Error, "model not found", "error text");
    let has_error_status = step.status == StepStatus::Error;
    let has_error_field = !step.error.is_empty();
    assert!(has_error_status && has_error_field);
}

#[test]
fn normal_step_not_treated_as_error() {
    let step = step_with(StepStatus::Done, "", "normal content");
    let has_error_status = step.status == StepStatus::Error;
    let has_error_field = !step.error.is_empty();
    assert!(!has_error_status && !has_error_field);
}

#[test]
fn empty_content_with_done_status_is_not_error() {
    let step = step_with(StepStatus::Done, "", "");
    let has_error_status = step.status == StepStatus::Error;
    let has_error_field = !step.error.is_empty();
    assert!(!has_error_status && !has_error_field);
}

// ── format_error_message tests ──────────────────────────────────

#[test]
fn format_uses_error_field_when_present() {
    let step = step_with(StepStatus::Error, "quota exceeded", "some content");
    assert_eq!(format_error_message(&step), "quota exceeded");
}

#[test]
fn format_falls_back_to_content_when_no_error_field() {
    let step = step_with(StepStatus::Error, "", "agent terminated");
    let msg = format_error_message(&step);
    assert!(msg.contains("agent terminated"), "got: {msg}");
    assert!(msg.contains("Error"), "got: {msg}");
}

#[test]
fn format_uses_content_delta_when_content_empty() {
    let step = Step {
        status: StepStatus::Error,
        content_delta: "delta error text".to_string(),
        ..Step::default()
    };
    let msg = format_error_message(&step);
    assert!(msg.contains("delta error text"), "got: {msg}");
}

// ── output_after_error state machine tests ──────────────────────
//
// These verify the tracking logic used by `stream_steps_to_writer`
// to decide whether to propagate errors at end-of-stream.

/// Simulates the state machine from `stream_steps_to_writer`.
/// Returns (`last_error`, `output_after_error`) after processing events.
fn simulate_stream(events: &[super::StepContent]) -> (Option<String>, bool) {
    // Delegate to the real state machine so these tests exercise production
    // logic rather than a parallel copy. (The early-stop signal is covered
    // separately in the `consecutive model-error` tests below.)
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    for event in events {
        state.observe(event);
    }
    (state.last_error, state.output_after_error)
}

#[test]
fn error_only_propagates() {
    let (last_error, output_after) = simulate_stream(&[err_step("503 unavailable")]);
    assert!(last_error.is_some());
    assert!(!output_after, "No output after error → should propagate");
}

#[test]
fn error_then_output_is_recovered() {
    let (last_error, output_after) =
        simulate_stream(&[err_step("model output empty"), super::StepContent::Output]);
    assert!(last_error.is_some());
    assert!(
        output_after,
        "Output after error → recovered, don't propagate"
    );
}

#[test]
fn error_then_output_then_error_propagates() {
    let (last_error, output_after) = simulate_stream(&[
        err_step("first error"),
        super::StepContent::Output,
        err_step("second error"),
    ]);
    assert_eq!(last_error.as_deref(), Some("second error"));
    assert!(!output_after, "Last error had no output after → propagate");
}

#[test]
fn clean_stream_no_error() {
    let (last_error, output_after) = simulate_stream(&[
        super::StepContent::Output,
        super::StepContent::Empty,
        super::StepContent::Output,
    ]);
    assert!(last_error.is_none());
    assert!(!output_after);
}

#[test]
fn output_before_error_does_not_count_as_recovery() {
    let (last_error, output_after) = simulate_stream(&[
        super::StepContent::Output, // output BEFORE error
        err_step("late error"),
    ]);
    assert!(last_error.is_some());
    assert!(
        !output_after,
        "Output before (not after) error → should propagate"
    );
}

#[test]
fn empty_steps_do_not_affect_recovery() {
    let (last_error, output_after) = simulate_stream(&[
        err_step("error"),
        super::StepContent::Empty,
        super::StepContent::Empty,
    ]);
    assert!(last_error.is_some());
    assert!(!output_after, "Empty steps don't count as recovery");
}

#[test]
fn multiple_errors_then_output_is_recovered() {
    let (last_error, output_after) = simulate_stream(&[
        err_step("first"),
        err_step("second"),
        super::StepContent::Output,
    ]);
    assert_eq!(last_error.as_deref(), Some("second"));
    assert!(output_after, "Output after last error → recovered");
}

// ── consecutive model-error early-stop tests ────────────────────

/// Build an error step carrying `message` and an unknown (`0`) HTTP status.
fn err_step(message: &str) -> super::StepContent {
    super::StepContent::Error {
        message: message.into(),
        http_code: 0,
    }
}

fn model_error() -> super::StepContent {
    err_step("model output must contain either output text or tool calls")
}

#[test]
fn three_consecutive_model_errors_stop_the_stream() {
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    assert!(
        !state.observe(&model_error()),
        "1st model error keeps streaming"
    );
    assert!(
        !state.observe(&model_error()),
        "2nd model error keeps streaming"
    );
    assert!(
        state.observe(&model_error()),
        "3rd consecutive model error must stop the stream"
    );
    assert!(state.last_error.is_some());
    assert!(
        !state.output_after_error,
        "no output followed → the error must propagate"
    );
}

#[test]
fn output_resets_the_model_error_streak() {
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    assert!(!state.observe(&model_error()));
    assert!(!state.observe(&model_error()));
    // A usable step resets the streak...
    assert!(!state.observe(&super::StepContent::Output));
    // ...so two further model errors still do not trip the limit.
    assert!(!state.observe(&model_error()));
    assert!(!state.observe(&model_error()));
    assert_eq!(state.consecutive_model_errors, 2);
}

#[test]
fn transient_errors_do_not_count_toward_the_model_limit() {
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    // The SDK's own backoff handles transport/API errors; they must never
    // trip the model-quality early-stop.
    for _ in 0..5 {
        assert!(!state.observe(&err_step("503 unavailable")));
    }
    assert_eq!(state.consecutive_model_errors, 0);
    assert!(state.last_error.is_some());
}

#[test]
fn a_transient_error_resets_the_model_error_streak() {
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    assert!(!state.observe(&model_error()));
    assert!(!state.observe(&model_error()));
    // A different (non-model) error breaks the consecutive model streak.
    assert!(!state.observe(&err_step("503 unavailable")));
    assert_eq!(state.consecutive_model_errors, 0);
}

#[test]
fn empty_steps_do_not_reset_the_model_error_streak() {
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    assert!(!state.observe(&model_error()));
    assert!(!state.observe(&super::StepContent::Empty));
    // The empty (metadata-only) step neither counts nor resets, so the next
    // model error is the 2nd — not enough to stop yet.
    assert!(!state.observe(&model_error()));
    // The 3rd consecutive model error (ignoring the empty) trips the limit.
    assert!(state.observe(&model_error()));
}

// ── runaway thinking-only (empty step) early-stop tests ─────────

#[test]
fn runaway_thinking_only_stream_is_aborted() {
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    let limit = super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS;
    // All but the last empty step keep the stream going.
    for i in 1..limit {
        assert!(
            !state.observe(&super::StepContent::Empty),
            "empty step {i} should not yet abort"
        );
    }
    // The step that reaches the limit aborts and records a synthetic,
    // model-quality-classified error so the orchestrator recovers.
    assert!(
        state.observe(&super::StepContent::Empty),
        "reaching the empty-step limit must abort the stream"
    );
    let err = state.last_error.expect("synthetic error recorded");
    assert!(
        super::is_model_quality_error(&err),
        "synthetic runaway error must be model-quality so it routes to recovery"
    );
    assert!(!state.output_after_error, "no output → error propagates");
}

#[test]
fn output_resets_the_empty_step_streak() {
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    // Stream thinking-only steps up to just below the limit...
    for _ in 0..(super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS - 1) {
        assert!(!state.observe(&super::StepContent::Empty));
    }
    // ...then a productive step resets the streak.
    assert!(!state.observe(&super::StepContent::Output));
    assert_eq!(state.consecutive_empty_steps, 0);
    // A fresh run of thinking-only steps starts over and does not abort.
    assert!(!state.observe(&super::StepContent::Empty));
}

#[test]
fn interleaved_output_prevents_runaway_abort() {
    let mut state = super::StreamErrorState::new(super::StreamLimits::default());
    // A healthy long turn: many thinking steps punctuated by output never
    // reaches the empty-step ceiling.
    for _ in 0..10 {
        for _ in 0..(super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS - 1) {
            assert!(!state.observe(&super::StepContent::Empty));
        }
        assert!(!state.observe(&super::StepContent::Output));
    }
    assert!(state.last_error.is_none(), "healthy turn records no error");
}

// ── disabled-limit (zero = unlimited) tests ─────────────────────

#[test]
fn zero_model_error_limit_never_aborts() {
    let limits = super::StreamLimits {
        max_model_errors: 0,
        max_empty_steps: super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS,
        channel_buffer: crate::streaming::DEFAULT_CHANNEL_BUFFER,
    };
    let mut state = super::StreamErrorState::new(limits);
    // Far more model errors than the default limit — none should trip.
    for _ in 0..100 {
        assert!(
            !state.observe(&model_error()),
            "zero limit must never abort on model errors"
        );
    }
    // The errors are still recorded for end-of-stream propagation.
    assert!(state.last_error.is_some());
}

#[test]
fn zero_empty_step_limit_never_aborts() {
    let limits = super::StreamLimits {
        max_model_errors: super::DEFAULT_MAX_CONSECUTIVE_MODEL_ERRORS,
        max_empty_steps: 0,
        channel_buffer: crate::streaming::DEFAULT_CHANNEL_BUFFER,
    };
    let mut state = super::StreamErrorState::new(limits);
    // Far more empty steps than the default limit — none should trip.
    for _ in 0..1000 {
        assert!(
            !state.observe(&super::StepContent::Empty),
            "zero limit must never abort on empty steps"
        );
    }
    assert!(state.last_error.is_none(), "no synthetic error recorded");
}

#[test]
fn stream_limits_from_config_uses_overrides() {
    let config = super::super::config::RuntimeConfig {
        max_consecutive_model_errors: Some(10),
        max_consecutive_empty_steps: Some(42),
        ..Default::default()
    };
    let limits = super::StreamLimits::from_config(&config);
    assert_eq!(limits.max_model_errors, 10);
    assert_eq!(limits.max_empty_steps, 42);
}

#[test]
fn stream_limits_from_config_uses_defaults_for_none() {
    let config = super::super::config::RuntimeConfig::default();
    let limits = super::StreamLimits::from_config(&config);
    assert_eq!(
        limits.max_model_errors,
        super::DEFAULT_MAX_CONSECUTIVE_MODEL_ERRORS
    );
    assert_eq!(
        limits.max_empty_steps,
        super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS
    );
}
