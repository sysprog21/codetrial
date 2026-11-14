//! What the hint ladder reads about the candidate's work, and how it holds a
//! rung until they act on the one before.
//!
//! Split out of `tests/agent.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

/// A judgment as the room makes one: the window is taken, then the answer is
/// applied to the state as it is then.
fn judge(state: &mut RuntimeState, steps: Value) -> Vec<FrameworkEvidence> {
    take_phase_judge_window(state, get_problem(Some("insert-interval")));
    apply_phase_judgment(state, &json!({ "steps": steps }).to_string())
}

/// A Run click that executed nothing is not a test the candidate ran.
#[test]
fn only_a_run_that_executed_cases_is_a_real_test_run() {
    let mut state = with_written_code(RuntimeState::default());
    let (code, language) = (state.code.clone(), state.language.clone());
    for run in [
        json!({"total": 0, "setupError": "javac: not found", "code": code, "language": language}),
        json!({"total": 0, "setupError": "runner unavailable", "runnerUnavailable": true,
            "code": code, "language": language}),
        json!({"passed": 0, "total": 0, "code": code, "language": language}),
    ] {
        apply_data_event(&mut state, TOPIC_TEST_RESULTS, &run, 99.0);
    }
    assert_eq!((state.test_runs, state.hint_attempt.real_test_runs), (3, 0));
    receive_test_run(&mut state);
    assert_eq!((state.test_runs, state.hint_attempt.real_test_runs), (4, 1));
}

/// A judged spoken step marks the turn that first said its quote, so words
/// repeated in a later turn do not move it, and only a step the candidate said
/// moves it: the Test row a received run books is not speech.
#[test]
fn the_judge_marks_the_first_turn_that_said_a_quote() {
    let mut state = RuntimeState {
        transcript: vec![
            "Interviewer: What is the problem asking?".to_string(),
            "Candidate: I need to merge any overlapping intervals and keep them sorted."
                .to_string(),
            "Interviewer: How would you approach it?".to_string(),
            "Candidate: I need to merge any overlapping intervals, so I would sort them by start \
             time first."
                .to_string(),
        ],
        ..RuntimeState::default()
    };
    let repeat = json!([{"step": "repeat", "quote": "I need to merge any overlapping intervals"}]);
    assert_eq!(judge(&mut state, repeat).len(), 1);
    assert_eq!(state.hint_attempt.quoted_turn_start, Some(1));
    let algorithm =
        json!([{"step": "algorithm", "quote": "I would sort them by start time first"}]);
    assert_eq!(judge(&mut state, algorithm).len(), 1);
    assert_eq!(state.hint_attempt.quoted_turn_start, Some(3));

    let mut state = with_written_code(state);
    receive_test_run(&mut state);
    assert!(framework_progress(&state).contains(&"test"));
    assert_eq!(state.hint_attempt.quoted_turn_start, Some(3));
}
