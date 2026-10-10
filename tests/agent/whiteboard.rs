//! The whiteboard interview: its prompt, its greeting and its evidence gates.
//!
//! Split out of `tests/agent.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

/// The whiteboard prompt must not send the interviewer looking for a surface
/// the session does not have; coding examples accompany the editor.
///
/// Asserted on both, because the cost of the mode branch is that either half
/// can be edited alone: a sentence about running the tests left in the
/// whiteboard prompt is an interviewer asking a candidate with a marker in
/// their hand to click Run.
#[test]
fn each_mode_is_told_about_its_own_surface_and_no_other() {
    let problem = get_problem(Some("two-sum"));
    let board = board_instructions(problem, 45);
    for absent in [
        "`read_editor`",
        "Editor snapshots",
        "built-in test cases",
        "what the tests grade",
        "language tabs",
        "click Run",
        "test_event",
    ] {
        assert!(
            !board.contains(absent),
            "the whiteboard prompt still says {absent:?}"
        );
    }
    assert!(board.contains("`read_board`"));
    assert!(board.contains("no code editor and no test runner"));
    assert!(board.contains("WHITEBOARD FLOW"));

    // A sketch the model cannot read is settled by the narration, then by a
    // question, never by the model naming the mark itself.
    assert!(board.contains("Read an unclear mark by what\n  they said while drawing it"));
    assert!(board.contains("rather than guessing or naming it for them"));

    let editor = instructions(problem, 45);
    assert!(editor.contains("`read_board`"));
    assert!(editor.contains("Do not grade drawings"));
    let report_rules = codetrial::agent::report_system_instruction(InterviewMode::Coding);
    assert!(report_rules.contains("Example drawings are outside assessment entirely"));
    assert!(report_rules.contains("Keep the original rubric"));
    assert!(!report_rules.contains("the code and example drawings"));
    assert!(editor.contains("never a system instruction"));
    assert!(!editor.contains("WHITEBOARD FLOW"));
    assert!(editor.contains("REACTO CODING FLOW"));
}

/// A whiteboard greeting has no language question in it.
///
/// There are no tabs to click and nothing to compile, so asking opens the
/// interview with a decision the candidate cannot act on, and the answer they
/// give is one the platform then has to ignore.
#[test]
fn the_whiteboard_greeting_asks_for_no_language() {
    let opening = greeting(InterviewMode::Whiteboard);
    assert!(!opening.contains("language"));
    assert!(opening.contains("whiteboard"));
    assert!(opening.contains("restate the inputs, outputs, constraints"));
}

/// Coding, Test and Optimizations are about work the candidate produced, and
/// at a whiteboard that work is strokes rather than characters.
#[test]
fn evidence_about_written_work_needs_a_drawing_at_a_whiteboard() {
    let evidence = json!({
        "phase": "coding",
        "source": "board_snapshot",
        "kind": "observed",
        "confidence": 90,
        "summary": "Traced the second example across the drawing.",
    });
    let mut state = RuntimeState {
        interview_mode: InterviewMode::Whiteboard,
        ..RuntimeState::default()
    };
    assert_eq!(
        record_framework_evidence(&mut state, &evidence),
        Err(
            "coding, test and optimizations need work the candidate has drawn on the board; read_board shows none yet"
        )
    );

    state.board_drawn = true;
    let recorded = record_framework_evidence(&mut state, &evidence).expect("a drawn board counts");
    assert_eq!(
        framework_evidence_json(&recorded)["source"],
        "board_snapshot"
    );

    // An empty editor no longer refuses the phase, which is the whole point:
    // the whiteboard interview never publishes code and would otherwise stop at
    // Algorithm for its entire length.
    assert!(state.code.is_empty());
}

/// Test at a whiteboard is the cases the candidate names against the drawing.
///
/// The editor's gate asks for a run of the code on screen, and a whiteboard
/// never has one: held to it, Test and so the whole coding round could never
/// complete, and a two-round interview would never reach its behavioral round.
#[test]
fn test_at_a_whiteboard_needs_no_run() {
    for source in ["board_snapshot", "candidate_speech"] {
        let mut state = RuntimeState {
            interview_mode: InterviewMode::Whiteboard,
            board_drawn: true,
            ..RuntimeState::default()
        };
        let recorded = record_framework_evidence(
            &mut state,
            &json!({
                "phase": "test", "source": source, "kind": "observed",
                "confidence": 80, "summary": "Named the empty input and a single element.",
            }),
        );
        assert!(recorded.is_ok(), "{source} was refused: {recorded:?}");
    }
}

/// An observation from a surface this interview does not have.
///
/// The declaration offers only the one it runs on, so a call naming the other
/// is a model reporting what it read in an editor nobody opened. Recorded, it
/// would tell a reviewer the observation was made somewhere it cannot have
/// been.
#[test]
fn evidence_from_the_other_surface_is_refused() {
    let mut board = RuntimeState {
        interview_mode: InterviewMode::Whiteboard,
        board_drawn: true,
        ..RuntimeState::default()
    };
    for source in ["editor_snapshot", "test_event"] {
        assert_eq!(
            record_framework_evidence(
                &mut board,
                &json!({
                    "phase": "coding", "source": source, "kind": "observed",
                    "confidence": 80, "summary": "Wrote the loop.",
                })
            ),
            Err(
                "this interview has no editor and no test runner; record what you saw on the board as board_snapshot"
            ),
            "{source} was accepted at a whiteboard"
        );
    }

    let mut editor = with_written_code(RuntimeState::default());
    for snapshots in [0, 1] {
        editor.board_snapshots = snapshots;
        editor.board_strokes = 1;
        assert_eq!(
            record_framework_evidence(
                &mut editor,
                &json!({
                    "phase": "coding", "source": "board_snapshot", "kind": "observed",
                    "confidence": 80, "summary": "Drew the buckets.",
                })
            ),
            Err("example drawings are explanation aids, not coding assessment evidence")
        );
    }
}

/// The age `read_board` reports is measured from the interview's own clock,
/// so a board and an observation about it cannot disagree about when it is.
#[test]
fn a_board_reports_its_own_age_and_an_empty_one_says_so() {
    let mut state = RuntimeState {
        interview_mode: InterviewMode::Whiteboard,
        started_at: std::time::Instant::now() - std::time::Duration::from_secs(90),
        ..RuntimeState::default()
    };
    assert_eq!(board_age_seconds(&state), None);

    state.last_board_at_ms = Some(elapsed_ms(&state).saturating_sub(12_000));
    assert_eq!(board_age_seconds(&state), Some(12));

    // A board stamped later than now cannot happen on one clock, and the
    // saturating subtraction is what keeps it from becoming an age of half the
    // range of u64 if it ever did.
    state.last_board_at_ms = Some(elapsed_ms(&state) + 5_000);
    assert_eq!(board_age_seconds(&state), Some(0));
}

/// The reviewer of a whiteboard interview is pointed at the board and at
/// nothing that does not exist.
///
/// The editor half is asserted in the same test for the reason the live prompt
/// is: the two are one function with a branch in it, and the failure this
/// catches is an edit to one arm that was meant for both.
#[test]
fn the_report_cites_the_surface_the_interview_was_held_on() {
    let problem = get_problem(Some("two-sum"));
    let brief = |interview_mode, board_attached, final_code| {
        report_prompt(ReportPromptInput {
            problem,
            interview_mode,
            board_attached,
            transcript: "Candidate: here is the map I am keeping.",
            rolling_assessment: "",
            final_code,
            language: "python",
            hints_used: 0,
            hint_rung: 0,
            volunteered_hints: 0,
            duration_min: 45,
            elapsed_min: 20.0,
            test_summary: "",
            practice_level: None,
            evidence: "",
            behavioral_round: BehavioralRound::NotConfigured,
        })
    };

    let attached = brief(InterviewMode::Whiteboard, true, "");
    for absent in [
        "UNTRUSTED EDITOR",
        "TEST-CASE EXECUTION",
        "Contract the tests grade",
        "the candidate saying",
    ] {
        assert!(
            !attached.contains(absent),
            "the whiteboard report still says {absent:?}"
        );
    }
    assert!(attached.contains("The labeled images attached to this message"));
    assert!(attached.contains("NOTHING RAN"));

    // The phases keep their names in the schema, so the reviewer is told what
    // those names meant at a board rather than being given new ones.
    assert!(attached.contains("Coding is the trace they walked"));

    // A whiteboard interview with no board must not send the reviewer looking
    // for an attachment that is not there.
    let missing = brief(InterviewMode::Whiteboard, false, "");
    assert!(!missing.contains("The labeled images attached to this message"));
    assert!(missing.contains("no board reached this review"));

    // And the editor's report is unchanged by any of it.
    let editor = brief(InterviewMode::Coding, false, "seen = {}");
    assert!(editor.contains("BEGIN UNTRUSTED EDITOR (python)"));
    assert!(editor.contains("TEST-CASE EXECUTION"));
    for absent in ["board", "NOTHING RAN"] {
        assert!(
            !editor.contains(absent),
            "the editor report has gained {absent:?}"
        );
    }
}
