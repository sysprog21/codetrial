//! A recorded Repeat and the comment that adds to it, split out of
//! `tests/agent/framework.rs`, which is at its size ceiling; like it, this
//! compiles as a module of the `tests/agent.rs` binary.

use super::framework::{judge, judged};
use super::*;

/// A Repeat judgment. Each call gives its own summary, so a refusal is never
/// the duplicate check's.
fn repeat(quote: &str, summary: &str) -> Value {
    json!([{"step": "repeat", "quote": quote, "summary": summary}])
}

/// An editor interview whose spoken restatement the judge has recorded.
fn restated() -> RuntimeState {
    let said = "I get a sorted list of intervals and one new interval and return them merged";
    let mut state = RuntimeState {
        transcript: vec![format!("Candidate: {said}")],
        code_templates: [("python".to_string(), String::new())].into(),
        ..RuntimeState::default()
    };
    assert_eq!(judge(&mut state, repeat(said, "Restated.")).len(), 1);
    state
}

fn repeat_rows(state: &RuntimeState) -> usize {
    let rows = state.framework_evidence.iter();
    rows.filter(|row| row.phase == FrameworkPhase::Repeat)
        .count()
}

/// The report credits a rule written as a comment only when a `repeat` phase
/// row quotes it, so a recorded Repeat stays open to a comment that adds to it
/// until another step is recorded. Speech, and a comment already quoted, add
/// nothing; neither does a whiteboard, which has no comments. A turn still
/// calls the judge for the steps speech can record, and a skipped row is not a
/// step recorded.
#[test]
fn a_comment_typed_before_moving_on_adds_to_a_recorded_restatement() {
    let rule = "the merged list stays sorted by start";
    let mut state = restated();
    let skip = json!({"phase": "situation", "source": "session_timing", "kind": "skipped",
        "confidence": 100, "summary": "Not reached."});
    state.behavioral_round_started = true;
    record_framework_evidence(&mut state, &skip).unwrap();
    state.behavioral_round_started = false;
    assert!(phase_judge_open(&state).contains(&"repeat"));
    let spoken = "and the merged intervals never overlap one another";
    state.transcript.push(format!("Candidate: {spoken}"));
    assert_eq!(phase_judge_need(&state), PhaseJudgeDue::Spoken);
    assert!(judge(&mut state, repeat(spoken, "Said.")).is_empty());

    state.code = format!("# {rule}\n");
    assert_eq!(phase_judge_need(&state), PhaseJudgeDue::EditorOnly);
    let window = take_phase_judge_window(&mut state, get_problem(Some("insert-interval")));
    assert!(window.contains("Repeat is already recorded"), "{window}");

    // The judge's summary is not a quote, so a comment repeating its words
    // still adds to the restatement.
    let other = "the intervals never overlap one another";
    let summary = format!("Typed that {other}.");
    let recorded = apply_phase_judgment(&mut state, &judged(repeat(rule, &summary)));
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].source, EvidenceSource::EditorSnapshot);
    assert!(judge(&mut state, repeat(rule, "Again.")).is_empty());
    state.code.push_str(&format!("# {other}\n"));
    assert_eq!(judge(&mut state, repeat(other, "Other.")).len(), 1);
    assert_eq!(repeat_rows(&state), 3);

    state.interview_mode = InterviewMode::Whiteboard;
    assert!(!phase_judge_open(&state).contains(&"repeat"));
}

/// Once another step is recorded the candidate has moved on, so a comment
/// typed after it does not reopen Repeat.
#[test]
fn a_comment_typed_after_the_next_step_does_not_reopen_repeat() {
    let mut state = restated();
    let example = json!({"phase": "example", "source": "candidate_speech",
        "kind": "observed", "confidence": 90, "summary": "Worked a case."});
    record_framework_evidence(&mut state, &example).unwrap();
    assert!(!phase_judge_open(&state).contains(&"repeat"));

    let rule = "the merged list stays sorted by start";
    state.code = format!("# {rule}\n");
    let window = take_phase_judge_window(&mut state, get_problem(Some("insert-interval")));
    assert!(!window.contains("Repeat is already recorded"), "{window}");
    assert!(apply_phase_judgment(&mut state, &judged(repeat(rule, "Typed."))).is_empty());
    assert_eq!(repeat_rows(&state), 1);
}

/// Another step recorded from the same answer has moved the candidate on, so
/// a comment in it is not recorded for Repeat, whichever the judge listed
/// first.
#[test]
fn another_step_in_the_answer_closes_repeat_to_a_comment() {
    let case = "for one three and two five it returns one five";
    let rule = "the merged list stays sorted by start";
    let example = json!({"step": "example", "quote": case, "summary": "Case."});
    let typed = json!({"step": "repeat", "quote": rule, "summary": "Typed."});
    for steps in [[&example, &typed], [&typed, &example]] {
        let mut state = restated();
        state.transcript.push(format!("Candidate: {case}"));
        state.code = format!("# {rule}\n");
        assert_eq!(judge(&mut state, json!(steps)).len(), 1);
        assert_eq!(repeat_rows(&state), 1, "{steps:?}");
    }

    // A first restatement recorded with another step is closed by it too, in
    // either order; a comment listed beside them is not taken first, and an
    // entry the server refuses does not stand in for the first restatement.
    let said = "I get a sorted list of intervals and one new interval and return them merged";
    let first = json!({"step": "repeat", "quote": said, "summary": "Said."});
    let refused = json!({"step": "repeat", "quote": "it combines them", "summary": "Short."});
    for steps in [
        vec![&first, &example],
        vec![&example, &first],
        vec![&first, &typed, &example],
        vec![&refused, &first, &example],
    ] {
        let mut state = RuntimeState {
            transcript: vec![format!("Candidate: {said}"), format!("Candidate: {case}")],
            code_templates: [("python".to_string(), String::new())].into(),
            code: format!("# {rule}\n"),
            ..RuntimeState::default()
        };
        assert_eq!(judge(&mut state, json!(steps)).len(), 2, "{steps:?}");
        assert!(!phase_judge_open(&state).contains(&"repeat"), "{steps:?}");
        assert!(
            judge(&mut state, repeat(rule, "Later.")).is_empty(),
            "{steps:?}"
        );
        assert_eq!(repeat_rows(&state), 1, "{steps:?}");
    }
}

/// Every order of one answer holding a refused Repeat, the spoken
/// restatement, a typed one and an example, from no Repeat and from a
/// reopened one: the example closes Repeat, and only the first Repeat entry
/// actually recorded joins it, so a permutation that records more, or leaves
/// Repeat open, is listed.
#[test]
fn every_order_of_one_answer_closes_repeat_with_its_example() {
    let said = "I get a sorted list of intervals and one new interval and return them merged";
    let case = "for one three and two five it returns one five";
    let rule = "the merged list stays sorted by start";
    let entries = [
        ("refused", repeat("it combines them", "Short.")),
        ("said", repeat(said, "Said.")),
        ("typed", repeat(rule, "Typed.")),
        (
            "example",
            json!([{"step": "example", "quote": case, "summary": "Case."}]),
        ),
    ];
    let mut wrong = Vec::new();
    for reopened in [false, true] {
        for a in 0..4 {
            for b in (0..4).filter(|b| *b != a) {
                for c in (0..4).filter(|c| ![a, b].contains(c)) {
                    let order = [a, b, c, 6 - a - b - c];
                    let mut state = if reopened {
                        restated()
                    } else {
                        RuntimeState {
                            transcript: vec![format!("Candidate: {said}")],
                            code_templates: [("python".to_string(), String::new())].into(),
                            ..RuntimeState::default()
                        }
                    };
                    state.transcript.push(format!("Candidate: {case}"));
                    state.code = format!("# {rule}\n");
                    let steps = order.map(|i| entries[i].1[0].clone());
                    let rows = judge(&mut state, json!(steps));
                    let got = rows
                        .iter()
                        .map(|row| (row.phase, row.source))
                        .collect::<Vec<_>>();

                    // From no Repeat, the first of said and typed in the order
                    // is the restatement; from a reopened one, only the
                    // example.
                    let first = order.into_iter().find(|i| matches!(i, 1 | 2));
                    let expected = match (reopened, first) {
                        (false, Some(1)) => vec![
                            (FrameworkPhase::Repeat, EvidenceSource::CandidateSpeech),
                            (FrameworkPhase::Example, EvidenceSource::CandidateSpeech),
                        ],
                        (false, _) => vec![
                            (FrameworkPhase::Repeat, EvidenceSource::EditorSnapshot),
                            (FrameworkPhase::Example, EvidenceSource::CandidateSpeech),
                        ],
                        (true, _) => {
                            vec![(FrameworkPhase::Example, EvidenceSource::CandidateSpeech)]
                        }
                    };
                    if got != expected || phase_judge_open(&state).contains(&"repeat") {
                        let names = order.map(|i| entries[i].0);
                        let open = phase_judge_open(&state).contains(&"repeat");
                        wrong.push(format!(
                            "reopened={reopened} {names:?}: {got:?}, Repeat open: {open}"
                        ));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The row keeps the quote cut to its bound, so a long comment is matched as
/// the row stores it. Otherwise the judge, shown nothing of what was recorded,
/// could quote it again on every call while Repeat is the latest step.
#[test]
fn a_long_comment_recorded_for_repeat_is_not_recorded_again() {
    let rule = "the merged list stays sorted by start, no two intervals in it overlap, and \
                touching intervals such as one two and two three merge into one";
    assert!(rule.chars().count() > 110, "longer than the row keeps");
    let mut state = restated();
    state.code = format!("# {rule}\n");
    assert_eq!(judge(&mut state, repeat(rule, "Typed.")).len(), 1);
    assert!(judge(&mut state, repeat(rule, "Again.")).is_empty());
    assert_eq!(repeat_rows(&state), 2);
}

/// The six-word floor is the quote's, not one comment's: the comment lines are
/// read joined, so a short rule comment is recorded when the quote runs into it
/// from the line above, though never on its own.
#[test]
fn a_quote_spanning_a_short_rule_comment_is_recorded_for_repeat() {
    let mut state = restated();
    state.code = "# input is the list of prices\n# sell after buy\n".to_string();
    assert!(judge(&mut state, repeat("sell after buy", "Rule.")).is_empty());
    let quote = "the list of prices sell after buy";
    let recorded = judge(&mut state, repeat(quote, "Spanned."));
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].source, EvidenceSource::EditorSnapshot);
}

/// Speech cannot record a reopened Repeat, so it neither calls the judge nor
/// holds a clue for it: with an example and an approach explained before the
/// restatement and no code, nothing spoken is left to observe.
#[test]
fn a_reopened_repeat_waits_for_a_comment_not_speech() {
    let mut state = RuntimeState {
        candidate_led_enabled: true,
        hint_ladder: get_problem(Some("3sum")).variant().hints,
        code_templates: [("python".to_string(), String::new())].into(),
        ..RuntimeState::default()
    };
    for phase in ["example", "algorithm", "repeat"] {
        let said = json!({"phase": phase, "source": "candidate_speech", "kind": "observed",
            "confidence": 90, "summary": "Said it."});
        record_framework_evidence(&mut state, &said).unwrap();
    }
    assert_eq!(phase_judge_open(&state), ["repeat"]);
    state
        .transcript
        .push("Candidate: I am still thinking.".to_string());
    assert_eq!(phase_judge_need(&state), PhaseJudgeDue::No);
    assert!(!record_hint(&mut state, true).contains("Not counted"));
    let held = record_hint(&mut state, true);
    assert!(held.contains("in the code, a run or the board"), "{held}");
}
