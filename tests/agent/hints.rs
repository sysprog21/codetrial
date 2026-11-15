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

const HELD: &str = "what they tried with the last clue";
const OPENING: &str = "Candidate: I am stuck.\nInterviewer: What would you try first?";
const OPTIMIZATION: &str = "sorting first costs n log n so the scan stays quadratic";
const CLUE: &str = "Interviewer: Think on it.\nCandidate: Two pointers move inward from both ends.";

/// What releases the next rung and what does not, as `act` performs them.
const RELEASING: &str = "an edit; an executed run; a turn after the clue; a changed board; \
    an optimization said after the algorithm";
const NOT_RELEASING: &str = "nothing; a setup error; no cases run; a tab switch; \
    switching back to a tested tab; an inferred step; a cleared board; the late request";

/// A Candidate-led interview holding the first rung; board scenarios run at the
/// whiteboard.
fn led(scenario: &str) -> RuntimeState {
    let whiteboard = scenario.ends_with("board");
    let mut state = with_written_code(RuntimeState {
        interview_mode: [InterviewMode::Coding, InterviewMode::Whiteboard][usize::from(whiteboard)],
        candidate_led_enabled: true,
        hint_ladder: get_problem(Some("3sum")).variant().hints,
        board_strokes: 3,
        transcript: OPENING.lines().map(String::from).collect(),
        ..RuntimeState::default()
    });
    if scenario == "switching back to a tested tab" {
        let run = json!({"passed": 1, "total": 1, "code": state.code, "language": "python"});
        type_code(&mut state, "c", "");
        apply_data_event(&mut state, TOPIC_TEST_RESULTS, &run, 99.0);
    }
    record_hint(&mut state, true);
    state
}

fn drawn(state: &mut RuntimeState, strokes: u32) {
    (state.hint_attempt.board_revisions, state.board_strokes) = (1, strokes);
}

fn algorithm(state: &mut RuntimeState, quote: &str) {
    let recorded = judge(state, json!([{"step": "algorithm", "quote": quote}]));
    assert_eq!(recorded.len(), 1, "{quote}");
}

/// What the candidate does after the rung.
fn act(state: &mut RuntimeState, scenario: &str) {
    let run = |state: &mut RuntimeState, run: Value| {
        apply_data_event(state, TOPIC_TEST_RESULTS, &run, 99.0);
    };
    let inferred = json!({"phase": "algorithm", "source": "candidate_speech", "kind": "inferred",
        "confidence": 90, "summary": "Candidate described their next step."});
    let late = "Candidate: I would sort the numbers and fix one, a hint?".to_string();
    match scenario {
        "nothing" => {}
        "an edit" => drop(type_code(state, "python", "nums.sort()\n")),
        "an executed run" => receive_test_run(state),
        "a setup error" => run(state, json!({"total": 0, "setupError": "no javac"})),
        "no cases run" => run(state, json!({"passed": 0, "total": 0})),
        "a tab switch" => drop(type_code(state, "java", "class Solution {}\n")),
        "switching back to a tested tab" => {
            let code = state.tested_code.clone().unwrap().code;
            type_code(state, "python", &code);
            assert!(framework_progress(state).contains(&"test"));
        }
        "a turn after the clue" => {
            state.transcript.extend(CLUE.lines().map(String::from));
            algorithm(state, "two pointers move inward from both ends");
        }
        "the late request" => {
            state.transcript.push(late);
            algorithm(state, "I would sort the numbers and fix one");
            state.transcript.extend(CLUE.lines().map(String::from));
        }
        "an optimization said after the algorithm" => {
            act(state, "the late request");
            state.transcript.push(format!("Candidate: {OPTIMIZATION}."));
            let said = judge(
                state,
                json!([{"step": "optimizations", "quote": OPTIMIZATION}]),
            );
            assert_eq!(said.len(), 1);
        }
        "an inferred step" => drop(record_framework_evidence(state, &inferred).unwrap()),
        "a changed board" => drawn(state, 4),
        "a cleared board" => drawn(state, 0),
        _ => unreachable!("{scenario}"),
    }
}

/// Candidate-led, a rung waits for work seen since the last one, and the key
/// step for an approach.
#[test]
fn candidate_led_log_hint_holds_the_next_rung_until_the_candidate_acts() {
    let hints = get_problem(Some("3sum")).variant().hints;
    let counts = |s: &RuntimeState| (s.hints_used, s.hint_rungs_given);
    for (scenarios, releases) in [(RELEASING, true), (NOT_RELEASING, false)] {
        for scenario in scenarios.split("; ") {
            let mut state = led(scenario);
            let reply = record_hint(&mut state, true);
            assert!(reply.contains(HELD), "{scenario}: {reply}");
            assert!(!reply.contains(hints[1]), "{scenario}");
            assert_eq!(counts(&state), (1, 1), "{scenario}");
            act(&mut state, scenario);
            let next = record_hint(&mut state, true);
            assert_eq!(next.contains(hints[1]), releases, "{scenario}: {next}");
            let given = (1 + u32::from(releases), 1 + usize::from(releases));
            assert_eq!(counts(&state), given, "{scenario}");
            let withheld = 2 - u32::from(releases);
            assert_eq!(state.evidence_ledger.hints.withheld, withheld, "{scenario}");
        }
    }

    let mut state = led("an executed run");
    act(&mut state, "an executed run");
    assert!(record_hint(&mut state, true).contains(hints[1]));
    act(&mut state, "an executed run");
    assert!(record_hint(&mut state, true).contains("stays withheld"));
    act(&mut state, "a turn after the clue");
    assert!(record_hint(&mut state, true).contains(hints[2]));

    // With no spoken step left for the phase judge to find, Coding aside, the
    // held reply asks for work it can see instead.
    let mut state = led("nothing");
    for phase in ["repeat", "example", "algorithm", "optimizations"] {
        let said = json!({"phase": phase, "source": "candidate_speech", "kind": "observed",
            "confidence": 90, "summary": "Said it."});
        record_framework_evidence(&mut state, &said).unwrap();
    }
    assert_eq!(phase_judge_open(&state), ["coding"]);
    assert!(record_hint(&mut state, true).contains("in the code, a run or the board"));
}
