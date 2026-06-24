use std::{sync::Arc, thread};

use agy_bridge::{
    error::Error,
    safety::{Confidence, detect_safety_interference},
    types::Step,
};

#[test]
fn test_stress_malformed_and_unusual_errors() {
    // Unusual casing
    let mut step = Step::default();
    step.error = "bLoCkEd bY SaFeTy fIlTeR".to_string();
    let verdict = detect_safety_interference(Some(&step), None);
    assert_eq!(verdict.confidence, Confidence::Certain);
    assert!(verdict.warrants_recovery);

    // Weird symbols / unicode but containing keyword
    let mut step2 = Step::default();
    step2.error = "🔥 request blocked by safety filter 🔥\x00\x01".to_string();
    let verdict2 = detect_safety_interference(Some(&step2), None);
    assert_eq!(verdict2.confidence, Confidence::Certain);
    assert!(verdict2.warrants_recovery);

    // Huge error string (stress testing performance/robustness with 1MB string)
    let mut huge_error = "A".repeat(1_000_000);
    huge_error.push_str(" blocked by safety ");
    huge_error.push_str(&"B".repeat(1_000_000));
    let mut step3 = Step::default();
    step3.error = huge_error;
    let verdict3 = detect_safety_interference(Some(&step3), None);
    assert_eq!(verdict3.confidence, Confidence::Certain);
    assert!(verdict3.warrants_recovery);
}

#[test]
fn test_stress_concurrent_safety_triggers() {
    // Verify thread safety and absence of data races under concurrent evaluation
    let mut step_val = Step::default();
    step_val.content =
        "I cannot fulfill this request because it goes against my safety policies.".to_string();
    let step = Arc::new(step_val);

    let mut handles = vec![];
    for _ in 0..100 {
        let step_clone = Arc::clone(&step);
        handles.push(thread::spawn(move || {
            let verdict = detect_safety_interference(Some(&step_clone), None);
            assert_eq!(verdict.confidence, Confidence::MediumHigh);
            assert!(verdict.warrants_recovery);
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_stress_ambiguous_combinations() {
    // A step with a suspiciously short response (Low) combined with an explicit backend error (High)
    let mut step = Step::default();
    step.content = "No.".to_string();
    let err = Error::BackendError {
        message: "HTTP 400: finish_reason: SAFETY".to_string(),
    };
    let verdict = detect_safety_interference(Some(&step), Some(&err));
    // High must win over Low
    assert_eq!(verdict.confidence, Confidence::High);
    assert!(verdict.warrants_recovery);
}

#[test]
fn test_stress_whitespace_variations() {
    // Step content with tabs, newlines, carriage returns -> should be MediumLow (empty response)
    let mut step = Step::default();
    step.content = "   \t \n \r  ".to_string();
    let verdict = detect_safety_interference(Some(&step), None);
    assert_eq!(verdict.confidence, Confidence::MediumLow);
    assert!(!verdict.warrants_recovery);
}
