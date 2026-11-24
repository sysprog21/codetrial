//! A recorded Repeat and the comment that adds to it, kept apart from
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

/// The report credits a rule written as a comment only when a `repeat` row
/// from `editor_snapshot` quotes it, so a recorded Repeat stays open to a
/// comment that adds to it
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
    let summary = format!("Typed that \"{other}\".");
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

/// With another step recorded and Repeat not yet, the candidate has moved on:
/// a comment is not taken as the first restatement, a late spoken one still
/// is and is not reopened, and a comment explaining another step still counts.
#[test]
fn a_comment_typed_after_another_step_is_not_a_first_restatement() {
    let said = "I get a sorted list of intervals and one new interval and return them merged";
    let rule = "the merged list stays sorted by start";
    let mut state = RuntimeState {
        transcript: vec![format!("Candidate: {said}")],
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {rule}\n"),
        ..RuntimeState::default()
    };
    let example = json!({"phase": "example", "source": "candidate_speech",
        "kind": "observed", "confidence": 90, "summary": "Worked a case."});
    record_framework_evidence(&mut state, &example).unwrap();
    assert!(judge(&mut state, repeat(rule, "Typed.")).is_empty());

    // Said while it is typed, it is a late spoken restatement all the same.
    let mut spoken = state.clone();
    spoken.transcript.push(format!("Candidate: {rule}"));
    let both = judge(&mut spoken, repeat(rule, "Said and typed."));
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].source, EvidenceSource::CandidateSpeech);
    let late = judge(&mut state, repeat(said, "Said."));
    assert_eq!(late.len(), 1);
    assert_eq!(late[0].source, EvidenceSource::CandidateSpeech);
    assert!(!phase_judge_open(&state).contains(&"repeat"));

    let approach = "merge every interval that overlaps the new one";
    state.code.push_str(&format!("# {approach}\n"));
    let algorithm = json!([{"step": "algorithm", "quote": approach, "summary": "Typed."}]);
    assert_eq!(judge(&mut state, algorithm).len(), 1);
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

/// The open steps are read once, as the judge was asked. Repeat entries are
/// tried first while Repeat is unrecorded, so a restatement listed after
/// Coding no longer shows it; an example does: with Optimizations recorded,
/// Coding closes every spoken step, and only a read after it would refuse one
/// listed after it in the same answer.
#[test]
fn coding_in_an_answer_does_not_close_an_example_listed_after_it() {
    let worked = "for one two three and target five it returns one and two";
    let mut state = with_written_code(RuntimeState {
        transcript: vec![format!("Candidate: {worked}")],
        ..RuntimeState::default()
    });
    let analysed = json!({"phase": "optimizations", "source": "editor_snapshot",
        "kind": "observed", "confidence": 90, "summary": "Done."});
    record_framework_evidence(&mut state, &analysed).unwrap();
    let steps = json!([
        {"step": "coding", "quote": "def solve(nums): return sorted(nums)", "summary": "Wrote it."},
        {"step": "example", "quote": worked, "summary": "Worked a case."}
    ]);
    let phases = judge(&mut state, steps)
        .iter()
        .map(|row| row.phase)
        .collect::<Vec<_>>();
    assert_eq!(phases, [FrameworkPhase::Coding, FrameworkPhase::Example]);
    assert!(phase_judge_open(&state).is_empty());
}

/// Every order of `n` entries.
fn orders(n: usize) -> Vec<Vec<usize>> {
    let Some(last) = n.checked_sub(1) else {
        return vec![Vec::new()];
    };
    let shorter = orders(last);
    let insert = |order: &Vec<usize>, at| {
        let mut order = order.clone();
        order.insert(at, last);
        order
    };
    let all = shorter
        .iter()
        .flat_map(|order| (0..n).map(move |at| insert(order, at)));
    all.collect()
}

/// Every order of one answer holding a refused Repeat, the spoken
/// restatement, a typed one, one both said and typed, and an example, from no
/// Repeat, from a reopened one and from an example recorded first: Repeat ends
/// closed. From no Repeat the first Repeat entry actually recorded joins the
/// example, and when it is a comment every other comment entry of the answer
/// joins too, ahead of the example, as the parts of one comment would; a
/// reopened Repeat, a second and separate restatement, takes none, and after
/// an example only the first spoken one is recorded. A permutation that
/// records more or another entry, or leaves Repeat open, is listed.
#[test]
fn every_order_of_one_answer_closes_repeat_with_its_example() {
    let said = "I get a sorted list of intervals and one new interval and return them merged";
    let case = "for one three and two five it returns one five";
    let rule = "the merged list stays sorted by start";
    let both = "no two intervals in the result overlap";
    let entries = [
        ("refused", repeat("it combines them", "Short.")),
        ("said", repeat(said, "Said.")),
        ("typed", repeat(rule, "Typed.")),
        ("both", repeat(both, "Both.")),
        (
            "example",
            json!([{"step": "example", "quote": case, "summary": "Case."}]),
        ),
    ];
    let mut wrong = Vec::new();
    for start in ["no Repeat", "reopened", "example first"] {
        for order in orders(entries.len()) {
            let mut state = if start == "reopened" {
                restated()
            } else {
                RuntimeState {
                    transcript: vec![format!("Candidate: {said}")],
                    code_templates: [("python".to_string(), String::new())].into(),
                    ..RuntimeState::default()
                }
            };
            if start == "example first" {
                let example = json!({"phase": "example", "source": "candidate_speech",
                            "kind": "observed", "confidence": 90, "summary": "Worked a case."});
                record_framework_evidence(&mut state, &example).unwrap();
            }
            state.transcript.push(format!("Candidate: {case}"));
            state.transcript.push(format!("Candidate: {both}"));
            state.code = format!("# {rule}\n# {both}\n");
            let steps = order.iter().map(|i| entries[*i].1[0].clone());
            let rows = judge(&mut state, json!(steps.collect::<Vec<_>>()));
            let got = rows
                .iter()
                .map(|row| (row.phase, row.source, row.summary.split(' ').next()))
                .collect::<Vec<_>>();

            // From no Repeat, the first of said, typed and both in the order is
            // the restatement, a comment while nothing else is recorded, and
            // when it is a comment typed and both are both recorded, in their
            // order, before the example; from a reopened one, only the example;
            // after an example, only the first of said and both, as speech.
            let spoken = |i: &&usize| matches!(i, 1 | 3);
            let first = order
                .iter()
                .find(|i| spoken(i) || (start == "no Repeat" && **i == 2));
            let source = match first {
                Some(1) => EvidenceSource::CandidateSpeech,
                _ if start == "no Repeat" => EvidenceSource::EditorSnapshot,
                _ => EvidenceSource::CandidateSpeech,
            };
            let restated = first.map(|i| {
                (
                    FrameworkPhase::Repeat,
                    source,
                    entries[*i].1[0]["summary"].as_str(),
                )
            });
            let example = (
                FrameworkPhase::Example,
                EvidenceSource::CandidateSpeech,
                Some("Case."),
            );
            let parts = order.iter().filter(|i| matches!(**i, 2 | 3)).map(|i| {
                (
                    FrameworkPhase::Repeat,
                    EvidenceSource::EditorSnapshot,
                    entries[*i].1[0]["summary"].as_str(),
                )
            });
            let expected = match start {
                "no Repeat" if source == EvidenceSource::EditorSnapshot => {
                    parts.chain([example]).collect()
                }
                "no Repeat" => vec![restated.unwrap(), example],
                "reopened" => vec![example],
                _ => Vec::from_iter(restated),
            };
            if got != expected || phase_judge_open(&state).contains(&"repeat") {
                let names = order.iter().map(|i| entries[*i].0).collect::<Vec<_>>();
                let open = phase_judge_open(&state).contains(&"repeat");
                wrong.push(format!("{start} {names:?}: {got:?}, Repeat open: {open}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Every cell of Repeat's matrix, as the bundle 49 row reads it: a new
/// restatement said, typed or both, coming alone before any other step, alone
/// after one, or in one answer with another step in either order, while Repeat
/// is unrecorded, recorded and open, or recorded and closed. Open with another
/// step recorded, and closed with none, cannot happen. Before moving on a
/// comment is the restatement; after, only speech is, and only a first one;
/// once Repeat is recorded only a comment adds to it, while it is open.
#[test]
fn every_cell_of_the_repeat_matrix_records_what_the_contract_says() {
    use EvidenceSource::{CandidateSpeech as Said, EditorSnapshot as Typed};
    let rule = "the merged list stays sorted by start";
    let approach = "merge every interval that overlaps the new one";
    let example = json!({"phase": "example", "source": "candidate_speech",
        "kind": "observed", "confidence": 90, "summary": "Worked a case."});
    let mut wrong = Vec::new();
    for repeat_state in ["unrecorded", "open", "closed"] {
        for timing in ["before", "after", "same, Repeat first", "same, Repeat last"] {
            if (repeat_state, timing) == ("open", "after")
                || (repeat_state, timing) == ("closed", "before")
            {
                continue;
            }
            for source in ["said", "typed", "said and typed"] {
                let mut state = if repeat_state == "unrecorded" {
                    RuntimeState {
                        code_templates: [("python".to_string(), String::new())].into(),
                        ..RuntimeState::default()
                    }
                } else {
                    restated()
                };
                if repeat_state == "closed" || timing == "after" {
                    record_framework_evidence(&mut state, &example).unwrap();
                }
                if source != "typed" {
                    state.transcript.push(format!("Candidate: {rule}"));
                }
                if source != "said" {
                    state.code = format!("# {rule}\n");
                }
                let mut steps = vec![json!({"step": "repeat", "quote": rule, "summary": "New."})];
                if timing.starts_with("same") {
                    state.transcript.push(format!("Candidate: {approach}"));
                    let other = json!({"step": "algorithm", "quote": approach, "summary": "Plan."});
                    let at = if timing.ends_with("first") { 1 } else { 0 };
                    steps.insert(at, other);
                }
                let rows = judge(&mut state, json!(steps));
                let got = rows
                    .iter()
                    .map(|row| (row.phase, row.source))
                    .collect::<Vec<_>>();
                let repeat = match (repeat_state, timing, source) {
                    ("unrecorded", "after", "typed") | ("open", "before", "said") => None,
                    ("unrecorded", "after", _) | ("unrecorded", _, "said") => Some(Said),
                    ("unrecorded", ..) | ("open", "before", _) => Some(Typed),
                    _ => None,
                };
                let mut expected =
                    Vec::from_iter(repeat.map(|source| (FrameworkPhase::Repeat, source)));
                if timing.starts_with("same") {
                    expected.push((FrameworkPhase::Algorithm, Said));
                }
                if got != expected {
                    wrong.push(format!(
                        "{repeat_state}, {timing}, {source}: {got:?}, expected {expected:?}"
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The report reads a comment only through an `editor_snapshot` Repeat row's
/// quote, and the row keeps 110 characters, so a comment quote longer than
/// that is refused for Repeat rather than recorded without a rule past the cut.
/// Quoted in parts,
/// each its own entry, the rule reaches a row whole even when the same answer
/// also records an example, since the parts are tried before it, and a part
/// quoted again is not recorded twice.
#[test]
fn a_rule_past_the_110th_character_of_a_comment_reaches_a_row() {
    let restatement = "I get a sorted list of intervals and one new interval, and I return the whole \
                       list with the new one merged in";
    let rule = "touching intervals such as one two and two three merge into one";
    let case = "for one three and two five it returns one five";
    let comment = format!("{restatement}, and {rule}");
    assert!(
        comment.find(rule).unwrap() > 110,
        "the rule starts past the cut"
    );
    let mut state = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {comment}\n"),
        transcript: vec![format!("Candidate: {case}")],
        ..RuntimeState::default()
    };
    assert!(judge(&mut state, repeat(&comment, "Whole.")).is_empty());
    let parts = json!([
        {"step": "repeat", "quote": restatement, "summary": "Restated."},
        {"step": "repeat", "quote": rule, "summary": "Rule."},
        {"step": "example", "quote": case, "summary": "Case."},
    ]);
    let rows = judge(&mut state, parts);
    let got = rows
        .iter()
        .map(|row| (row.phase, row.source))
        .collect::<Vec<_>>();
    let typed = (FrameworkPhase::Repeat, EvidenceSource::EditorSnapshot);
    let said = (FrameworkPhase::Example, EvidenceSource::CandidateSpeech);
    assert_eq!(got, [typed, typed, said]);
    assert!(
        rows[1].summary.ends_with(&format!("\"{rule}\"")),
        "{}",
        rows[1].summary
    );
    assert!(judge(&mut state, repeat(rule, "Again.")).is_empty());
    assert_eq!(repeat_rows(&state), 2);
}

/// A rule in a short tail past the 110th character is under six words on its
/// own, so the judge quotes the last part overlapping the one before; the
/// duplicate check takes it, since it adds the tail, and refuses it quoted
/// again.
#[test]
fn a_rule_in_a_short_tail_is_quoted_in_an_overlapping_part() {
    let restatement = "I get a sorted list of intervals and one new interval, and I return the whole \
                       list with the new one merged in";
    let tail = "touching ones merge";
    let comment = format!("{restatement}, and {tail}");
    assert!(
        comment.find(tail).unwrap() > 110,
        "the rule is past the cut"
    );
    let mut state = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {comment}\n"),
        ..RuntimeState::default()
    };
    assert!(judge(&mut state, repeat(tail, "Tail.")).is_empty());
    let overlap = "the new one merged in, and touching ones merge";
    let parts = json!([
        {"step": "repeat", "quote": restatement, "summary": "Restated."},
        {"step": "repeat", "quote": overlap, "summary": "Overlap."},
    ]);
    let rows = judge(&mut state, parts);
    assert_eq!(rows.len(), 2);
    assert!(
        rows[1].summary.ends_with(&format!("\"{overlap}\"")),
        "{}",
        rows[1].summary
    );
    assert!(judge(&mut state, repeat(overlap, "Again.")).is_empty());
    assert_eq!(repeat_rows(&state), 2);
}

/// The comment words are read from the head of the editor the judge is shown,
/// so a rule typed past it, which the judge never saw, is not a comment it can
/// quote, and the cut never reads a buffer longer than that head.
#[test]
fn a_comment_past_the_head_the_judge_is_shown_is_not_quoted() {
    let rule = "the merged list stays sorted by start";
    let filler = "x = 1\n".repeat(7 * 1024 / 6);
    let mut past = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("{filler}# {rule}\n"),
        ..RuntimeState::default()
    };
    let mut ahead = RuntimeState {
        code: format!("# {rule}\n{filler}"),
        ..past.clone()
    };
    assert!(judge(&mut past, repeat(rule, "Typed late in the file.")).is_empty());
    assert_eq!(judge(&mut ahead, repeat(rule, "Typed first.")).len(), 1);
}

/// The bound is the row's own: a comment quote of 110 characters, a line break
/// counted as the space the row keeps for it and surrounding spaces not counted
/// as the row trims them, is recorded, and one of 111 is not. Only Repeat reads
/// a comment through its row, so another step's long comment is still
/// recorded, and a long restatement also said is recorded as speech, which
/// reaches the report whole in the transcript.
#[test]
fn a_repeat_comment_quote_is_bounded_by_what_its_row_keeps() {
    let typed = |quote: &str, steps: Value| {
        let mut state = RuntimeState {
            code_templates: [("python".to_string(), String::new())].into(),
            code: format!("# {}\n", quote.replace('\n', "\n# ")),
            ..RuntimeState::default()
        };
        judge(&mut state, steps)
            .iter()
            .map(|row| (row.phase, row.source))
            .collect::<Vec<_>>()
    };
    let sized = |chars: usize| {
        let frame = "the sorted list holds  intervals".len();
        format!(
            "the sorted list holds\n{} intervals",
            "x".repeat(chars - frame)
        )
    };
    assert_eq!(sized(110).chars().count(), 110);
    let fits = sized(110);
    let over = sized(111);
    let typed_repeat = vec![(FrameworkPhase::Repeat, EvidenceSource::EditorSnapshot)];
    assert_eq!(typed(&fits, repeat(&fits, "Fits.")), typed_repeat);
    let padded = format!(" {fits} ");
    assert_eq!(typed(&fits, repeat(&padded, "Padded.")), typed_repeat);
    assert!(typed(&over, repeat(&over, "Over.")).is_empty());
    let example = json!([{"step": "example", "quote": over, "summary": "Case."}]);
    assert_eq!(
        typed(&over, example),
        [(FrameworkPhase::Example, EvidenceSource::EditorSnapshot)]
    );

    let long = over.replace('\n', " ");
    let mut state = RuntimeState {
        transcript: vec![format!("Candidate: {long}")],
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {long}\n"),
        ..RuntimeState::default()
    };
    let rows = judge(&mut state, repeat(&long, "Said."));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source, EvidenceSource::CandidateSpeech);
}

/// The row's quote is found by the double quotes around it, not by the
/// "Quote: " marker, which a comment may hold too, so a comment holding it is
/// not recorded again under another summary.
#[test]
fn a_comment_holding_the_quote_marker_is_not_recorded_again() {
    let rule = "Quote: the \"sale\" must follow the buy day";
    let mut state = restated();
    state.code = format!("# {rule}\n");
    assert_eq!(judge(&mut state, repeat(rule, "Typed.")).len(), 1);
    assert!(judge(&mut state, repeat(rule, "Again.")).is_empty());
    assert_eq!(repeat_rows(&state), 2);
}

/// The starter's comment lines are cut out before the comment lines are
/// joined, so its words never bring a short rule comment to the six-word floor,
/// even typed twice on one line.
#[test]
fn the_starter_comment_does_not_bring_a_rule_to_six_words() {
    let page = std::fs::read_dir("web/problems")
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .map(|page| serde_json::from_str::<Value>(&page).unwrap())
        .find(|page| page["page"] == "single-resale-window")
        .expect("the single-resale-window page");
    let starter = page["starterCode"]["python"].as_str().unwrap();
    assert!(starter.contains("        # Think out loud as you go!\n        pass\n"));
    let mut state = restated();
    state
        .code_templates
        .insert("python".to_string(), starter.to_string());
    state.code = starter.replace("pass", "# sell after buy\n        pass");
    assert!(judge(&mut state, repeat("as you go sell after buy", "Joined.")).is_empty());
    let twice = "# Think out loud as you go! Think out loud as you go! sell after buy";
    state.code = starter.replace("# Think out loud as you go!", twice);
    assert!(judge(&mut state, repeat("as you go sell after buy", "Twice.")).is_empty());
    let around = "# you must sell Think out loud as you go! later than you buy";
    state.code = starter.replace("# Think out loud as you go!", around);
    let across = "you must sell later than you buy";
    assert!(judge(&mut state, repeat(across, "Across.")).is_empty());
    state.code = starter.replace("pass", "# sell after buy\n        pass");
    let rule = "you must sell on a later day than you buy";
    state.code = state.code.replace("sell after buy", rule);
    assert_eq!(judge(&mut state, repeat(rule, "Typed.")).len(), 1);
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

/// A quote copied across two comment lines keeps a space where the line broke,
/// so the row the report reads does not glue "prices" to "sell", and the same
/// quote is then found in it and not recorded twice.
#[test]
fn a_quote_across_two_comment_lines_keeps_a_space_in_the_row() {
    let mut state = restated();
    state.code = "# input is the list of prices\n# sell after buy\n".to_string();
    let quote = "the list of prices\nsell after buy";
    let recorded = judge(&mut state, repeat(quote, "Two lines."));
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].summary,
        "Two lines. Quote: \"the list of prices sell after buy\""
    );
    assert!(judge(&mut state, repeat(quote, "Again.")).is_empty());
}

/// A blank comment line between two the candidate typed does not break a quote
/// across them, though the starter has a blank comment line too: it holds no
/// words, so it cuts nothing out.
#[test]
fn a_blank_comment_line_does_not_break_a_quote() {
    let mut state = restated();
    state
        .code_templates
        .insert("python".to_string(), "#\n".to_string());
    state.code = "#\n# input is the list of prices\n#\n# sell after buy\n".to_string();
    let quote = "the list of prices sell after buy";
    assert_eq!(judge(&mut state, repeat(quote, "Blank.")).len(), 1);
}

/// A restatement recorded after an example and an approach is not reopened to a
/// comment, so with no code nothing is left for the judge to record and a held
/// clue asks for work it can see, not for more speech.
#[test]
fn a_restatement_after_other_steps_leaves_nothing_to_hear() {
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
    assert!(phase_judge_open(&state).is_empty());
    state
        .transcript
        .push("Candidate: I am still thinking.".to_string());
    assert_eq!(phase_judge_need(&state), PhaseJudgeDue::No);
    assert!(!record_hint(&mut state, true).contains("Not counted"));
    let held = record_hint(&mut state, true);
    assert!(held.contains("in the code, a run or the board"), "{held}");
}

/// The C starter of canary-host-selection, whose comment states the output.
fn canary_c_starter() -> String {
    let page = std::fs::read_dir("web/problems")
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .map(|page| serde_json::from_str::<Value>(&page).unwrap())
        .find(|page| page["page"] == "canary-host-selection")
        .expect("the canary-host-selection page");
    page["starterCode"]["c"].as_str().unwrap().to_string()
}

/// The starter's words stay the starter's when the candidate edits its comment
/// line: neither the starter's sentence nor a quote it pads to six words is
/// theirs, while words they added to the line still are.
#[test]
fn starter_words_on_an_edited_starter_line_are_not_the_candidates() {
    let starter = canary_c_starter();
    let line = "of size *returnSize.";
    assert!(starter.contains(line));
    let edited = |added: &str| RuntimeState {
        language: "c".to_string(),
        code_templates: [("c".to_string(), starter.clone())].into(),
        code: starter.replace(line, &format!("of size *returnSize, {added}")),
        ..RuntimeState::default()
    };
    let mut state = edited("k per group.");
    let given = "Return an array of arrays of size returnSize";
    assert!(judge(&mut state, repeat(given, "Output.")).is_empty());
    let padded = "arrays of size returnSize k per group";
    assert!(judge(&mut state, repeat(padded, "Padded.")).is_empty());
    let mut state = edited("with k hosts in every group.");
    let own = "with k hosts in every group";
    assert_eq!(judge(&mut state, repeat(own, "Theirs.")).len(), 1);
}

/// Before any restatement Repeat is open but unrecorded, so the judge is not
/// told it is already recorded, which would have it pass over a first spoken
/// restatement.
#[test]
fn an_unrecorded_repeat_is_not_called_recorded() {
    let mut state = RuntimeState {
        transcript: vec!["Candidate: I get a sorted list of intervals".to_string()],
        code_templates: [("python".to_string(), String::new())].into(),
        ..RuntimeState::default()
    };
    assert!(phase_judge_open(&state).contains(&"repeat"));
    let window = take_phase_judge_window(&mut state, get_problem(Some("insert-interval")));
    assert!(!window.contains("Repeat is already recorded"), "{window}");
}

/// With another step recorded and Repeat not, a comment no longer records it,
/// so the judge is told that only speech does, and a spoken restatement is
/// still recorded while the comment is refused.
#[test]
fn the_judge_is_told_a_comment_no_longer_counts_for_repeat() {
    let rule = "you must sell on a later day than you buy";
    let said = "I get the daily prices and return the best profit from one sale";
    let mut state = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {rule}\n"),
        transcript: vec![format!("Candidate: {said}")],
        ..RuntimeState::default()
    };
    let example = json!({"phase": "example", "source": "candidate_speech",
        "kind": "observed", "confidence": 90, "summary": "Worked a case."});
    record_framework_evidence(&mut state, &example).unwrap();
    let window = take_phase_judge_window(&mut state, get_problem(Some("insert-interval")));
    assert!(
        window.contains("record repeat only from the candidate's speech"),
        "{window}"
    );
    assert!(apply_phase_judgment(&mut state, &judged(repeat(rule, "Typed."))).is_empty());
    let rows = apply_phase_judgment(&mut state, &judged(repeat(said, "Said.")));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source, EvidenceSource::CandidateSpeech);

    let mut fresh = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        transcript: vec![format!("Candidate: {said}")],
        ..RuntimeState::default()
    };
    let window = take_phase_judge_window(&mut fresh, get_problem(Some("insert-interval")));
    assert!(
        !window.contains("only from the candidate's speech"),
        "{window}"
    );

    // A whiteboard has no comment, so it is not told about one.
    let mut board = RuntimeState {
        interview_mode: InterviewMode::Whiteboard,
        transcript: vec![format!("Candidate: {said}")],
        ..RuntimeState::default()
    };
    let drawn = json!({"phase": "example", "source": "board_snapshot",
        "kind": "observed", "confidence": 90, "summary": "Drew a case."});
    record_framework_evidence(&mut board, &drawn).unwrap();
    assert!(phase_judge_open(&board).contains(&"repeat"));
    let window = take_phase_judge_window(&mut board, get_problem(Some("insert-interval")));
    assert!(
        !window.contains("only from the candidate's speech"),
        "{window}"
    );
}

/// A comment quoted in parts, one row each, is not recorded again as one quote
/// that spans the parts, since every word of it is already quoted.
#[test]
fn a_quote_two_rows_already_hold_is_not_recorded_again() {
    let one = "the merged list stays sorted by start";
    let two = "no two intervals in the result overlap";
    let mut state = restated();
    state.code = format!("# {one}\n# {two}\n");
    assert_eq!(judge(&mut state, repeat(one, "One.")).len(), 1);
    assert_eq!(judge(&mut state, repeat(two, "Two.")).len(), 1);
    let both = format!("{one} {two}");
    assert!(judge(&mut state, repeat(&both, "Both.")).is_empty());
    assert_eq!(repeat_rows(&state), 3);
}

/// A starter comment line edited inside, a word added or taken out, still
/// holds the starter's words, which ground no quote.
#[test]
fn starter_words_on_a_line_edited_inside_are_not_the_candidates() {
    let starter = canary_c_starter();
    let cases = [
        (
            "The sizes of the arrays are returned",
            "The sizes of the k arrays are returned",
            "arrays are returned as *returnColumnSizes array",
        ),
        (
            "Note: Both returned array",
            "Both returned array",
            "Both returned array and *columnSizes array must be malloced",
        ),
    ];
    for (from, to, quote) in cases {
        assert!(starter.contains(from), "{from}");
        let mut state = RuntimeState {
            language: "c".to_string(),
            code_templates: [("c".to_string(), starter.clone())].into(),
            code: starter.replace(from, to),
            ..RuntimeState::default()
        };
        assert!(
            judge(&mut state, repeat(quote, "Starter.")).is_empty(),
            "{quote}"
        );
    }
}

/// A line the candidate typed is theirs though it holds the words of a short
/// starter line, here "@return {number}", which a restatement of the output
/// says in passing.
#[test]
fn a_candidate_line_holding_a_short_starter_line_is_theirs() {
    let page = std::fs::read_dir("web/problems")
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .map(|page| serde_json::from_str::<Value>(&page).unwrap())
        .find(|page| page["page"] == "address-block-shared-bits")
        .expect("the address-block-shared-bits page");
    let starter = page["starterCode"]["javascript"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(starter.contains(" * @return {number}\n"));
    let own = "I return number of bits shared by every address in the block";
    let mut state = RuntimeState {
        language: "javascript".to_string(),
        code_templates: [("javascript".to_string(), starter.clone())].into(),
        code: starter.replace("// Think out loud as you go!", &format!("// {own}")),
        ..RuntimeState::default()
    };
    let quote = "I return number of bits shared by every address";
    assert_eq!(judge(&mut state, repeat(quote, "Output.")).len(), 1);

    // A sentence that keeps six of the eight words of a starter line says the
    // output in the candidate's own words, and is not cut as an edit of it.
    let starter = canary_c_starter();
    let own = "I return an array of host groups, each of size k";
    let mut state = RuntimeState {
        language: "c".to_string(),
        code_templates: [("c".to_string(), starter.clone())].into(),
        code: format!("{starter}// {own}\n"),
        ..RuntimeState::default()
    };
    assert_eq!(judge(&mut state, repeat(own, "Output.")).len(), 1);
}

/// A sentence that holds a starter line's words in order but spreads them
/// across itself is the candidate's: here "for a binary tree" and "node" of
/// the Python tree starter's "# Definition for a binary tree node.", five of
/// its six words, with five of the candidate's own between them.
#[test]
fn a_sentence_spreading_a_starter_lines_words_is_theirs() {
    let page = std::fs::read_dir("web/problems")
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .map(|page| serde_json::from_str::<Value>(&page).unwrap())
        .find(|page| page["page"] == "fraud-rule-worst-case")
        .expect("the fraud-rule-worst-case page");
    let starter = page["starterCode"]["python"].as_str().unwrap().to_string();
    assert!(starter.starts_with("# Definition for a binary tree node.\n"));
    let own = "For a binary tree, return the label of each node seen from the left";
    let mut state = RuntimeState {
        code_templates: [("python".to_string(), starter.clone())].into(),
        code: starter.replace("# Think out loud as you go!", &format!("# {own}")),
        ..RuntimeState::default()
    };
    assert_eq!(judge(&mut state, repeat(own, "Output.")).len(), 1);
}

/// A known limit: parts are kept ahead of the answer's other steps only once a
/// part is recorded from that answer. A first part recorded by an earlier
/// call leaves the rest to an answer like any comment added to a recorded
/// Repeat, so an answer that also records an example records only the example.
#[test]
fn a_part_after_one_recorded_earlier_is_closed_by_an_example() {
    let restatement = "I get a sorted list of intervals and one new interval, and I return the whole \
                       list with the new one merged in";
    let rule = "touching intervals such as one two and two three merge into one";
    let case = "for one three and two five it returns one five";
    let mut state = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {restatement},\n"),
        ..RuntimeState::default()
    };
    assert_eq!(judge(&mut state, repeat(restatement, "Restated.")).len(), 1);
    state.code = format!("# {restatement},\n# and {rule}\n");
    state.transcript.push(format!("Candidate: {case}"));
    let parts = json!([
        {"step": "repeat", "quote": restatement, "summary": "Restated again."},
        {"step": "repeat", "quote": rule, "summary": "Rule."},
        {"step": "example", "quote": case, "summary": "Case."},
    ]);
    let got = judge(&mut state, parts)
        .iter()
        .map(|row| row.phase)
        .collect::<Vec<_>>();
    assert_eq!(got, [FrameworkPhase::Example]);
}

/// A comment quoted in parts may need more repeat entries than there are
/// steps, so repeat entries are counted apart from the others and every part
/// reaches a row, here twelve with an example and an approach listed first.
#[test]
fn every_part_reaches_a_row_past_the_step_count() {
    let case = "for one three and two five it returns one five";
    let plan = "merge every interval that overlaps the new one";
    let lines = [
        "the input is a sorted list of intervals",
        "each interval holds a start and an end",
        "the second input is one new interval",
        "the output is the list with it merged",
        "the output stays sorted by start value",
        "no two output intervals overlap one another",
        "touching intervals such as one two and two three merge",
        "an empty list returns just the new interval",
        "the new interval may cover every other interval",
        "an interval that ends before the new one stays",
        "an interval that starts after the new one stays",
        "the merged interval spans the widest start and end",
    ];
    let mut state = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: lines.iter().map(|l| format!("# {l}\n")).collect(),
        transcript: vec![format!("Candidate: {case}"), format!("Candidate: {plan}")],
        ..RuntimeState::default()
    };
    let mut steps = vec![
        json!({"step": "example", "quote": case, "summary": "Case."}),
        json!({"step": "algorithm", "quote": plan, "summary": "Plan."}),
    ];
    for (i, line) in lines.iter().enumerate() {
        steps.push(json!({"step": "repeat", "quote": line, "summary": format!("Part {i}.")}));
    }
    judge(&mut state, json!(steps));
    assert_eq!(repeat_rows(&state), lines.len(), "a part was dropped");
}

/// A comment correcting the spoken restatement is matched against the rows as
/// whole words, so "sorted" is not found inside "unsorted" and it adds to
/// Repeat.
#[test]
fn a_correction_inside_a_longer_word_still_adds_to_repeat() {
    let said = "the input comes unsorted by start and I return the intervals merged";
    let mut state = RuntimeState {
        transcript: vec![format!("Candidate: {said}")],
        code_templates: [("python".to_string(), String::new())].into(),
        ..RuntimeState::default()
    };
    assert_eq!(judge(&mut state, repeat(said, "Said.")).len(), 1);
    state.code =
        "# correction: input is sorted by start, and I return the intervals merged\n".to_string();
    let fixed = "sorted by start, and I return the intervals merged";
    assert_eq!(judge(&mut state, repeat(fixed, "Fixed.")).len(), 1);
}

/// The cells of Repeat's matrix with a comment over 110 characters quoted in
/// two
/// parts as the restatement: both parts are recorded wherever one comment would
/// be, the other step of the same answer listed before, between or after them,
/// and none where a comment would not.
#[test]
fn every_cell_of_the_repeat_matrix_holds_for_a_comment_in_parts() {
    use EvidenceSource::{CandidateSpeech as Said, EditorSnapshot as Typed};
    let one = "I get a sorted list of intervals and one new interval, and I return the whole \
               list with the new one merged in";
    let two = "touching intervals such as one two and two three merge into one";
    let approach = "merge every interval that overlaps the new one";
    let example = json!({"phase": "example", "source": "candidate_speech",
        "kind": "observed", "confidence": 90, "summary": "Worked a case."});
    let mut wrong = Vec::new();
    for repeat_state in ["unrecorded", "open", "closed"] {
        for timing in [
            "before",
            "after",
            "same, parts first",
            "same, between",
            "same, parts last",
        ] {
            if (repeat_state, timing) == ("open", "after")
                || (repeat_state, timing) == ("closed", "before")
            {
                continue;
            }
            let mut state = if repeat_state == "unrecorded" {
                RuntimeState {
                    code_templates: [("python".to_string(), String::new())].into(),
                    ..RuntimeState::default()
                }
            } else {
                restated()
            };
            if repeat_state == "closed" || timing == "after" {
                record_framework_evidence(&mut state, &example).unwrap();
            }
            state.code = format!("# {one}, and {two}\n");
            let mut steps = vec![
                json!({"step": "repeat", "quote": one, "summary": "One."}),
                json!({"step": "repeat", "quote": two, "summary": "Two."}),
            ];
            if timing.starts_with("same") {
                state.transcript.push(format!("Candidate: {approach}"));
                let other = json!({"step": "algorithm", "quote": approach, "summary": "Plan."});
                let at = match timing {
                    "same, parts first" => 2,
                    "same, between" => 1,
                    _ => 0,
                };
                steps.insert(at, other);
            }
            let got = judge(&mut state, json!(steps))
                .iter()
                .map(|row| (row.phase, row.source))
                .collect::<Vec<_>>();
            let parts = match (repeat_state, timing) {
                ("unrecorded", "after") => 0,
                ("unrecorded", _) | ("open", "before") => 2,
                _ => 0,
            };
            let mut expected = vec![(FrameworkPhase::Repeat, Typed); parts];
            if timing.starts_with("same") {
                expected.push((FrameworkPhase::Algorithm, Said));
            }
            if got != expected {
                wrong.push(format!(
                    "{repeat_state}, {timing}: {got:?}, expected {expected:?}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A starter line counts as edited when the candidate's line keeps at least
/// three of its words, and four in five of them, in order: a three-word line
/// left as it was pads no quote, and a five-word one keeping exactly four of
/// its words is cut as well.
#[test]
fn a_starter_line_kept_to_the_bound_is_cut() {
    let typed = |starter: &str, code: &str, quote: &str| {
        let mut state = RuntimeState {
            code_templates: [("python".to_string(), starter.to_string())].into(),
            code: code.to_string(),
            ..RuntimeState::default()
        };
        judge(&mut state, repeat(quote, "Typed.")).len()
    };
    let three = "# the sorted list\n";
    let padded = format!("{three}# you must merge them\n");
    assert_eq!(typed(three, &padded, "the sorted list you must merge"), 0);
    let five = "# the input list is sorted\n";
    let kept = "# the input list stays sorted by start\n";
    assert_eq!(typed(five, kept, "the input list stays sorted by start"), 0);
    let theirs = "# every interval keeps its start and end\n";
    assert_eq!(
        typed(five, theirs, "every interval keeps its start and end"),
        1
    );
}

#[test]
fn a_quote_across_an_overlap_seam_is_not_recorded_again() {
    let restatement = "I get a sorted list of intervals and one new interval, and I return the whole \
                       list with the new one merged in";
    let mut state = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {restatement}, and touching ones merge\n"),
        ..RuntimeState::default()
    };
    let parts = json!([
        {"step": "repeat", "quote": restatement, "summary": "Restated."},
        {"step": "repeat", "quote": "the new one merged in, and touching ones merge", "summary": "Overlap."},
    ]);
    assert_eq!(judge(&mut state, parts).len(), 2);
    let across = "the whole list with the new one merged in, and touching ones merge";
    assert!(judge(&mut state, repeat(across, "Across.")).is_empty());
    assert_eq!(repeat_rows(&state), 2);
}

/// The rows are placed where they fall in the comment, not read in the order
/// they were recorded: a comment whose later part was quoted first, by an
/// earlier answer, is covered as well, and a quote across where the two parts
/// overlap adds nothing.
#[test]
fn parts_recorded_out_of_order_still_cover_a_quote_across_them() {
    let restatement = "I get a sorted list of intervals and one new interval, and I return the whole \
                       list with the new one merged in";
    let overlap = "the new one merged in, and touching ones merge";
    let mut state = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {restatement}, and touching ones merge\n"),
        ..RuntimeState::default()
    };
    assert_eq!(judge(&mut state, repeat(overlap, "Tail first.")).len(), 1);
    assert_eq!(judge(&mut state, repeat(restatement, "Head.")).len(), 1);
    let across = "the whole list with the new one merged in, and touching ones merge";
    assert!(judge(&mut state, repeat(across, "Across.")).is_empty());
    assert_eq!(repeat_rows(&state), 2);

    // A quote reaching words no row placed is still taken.
    let mut wider = RuntimeState {
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {restatement}, and touching ones merge\n"),
        ..RuntimeState::default()
    };
    assert_eq!(judge(&mut wider, repeat(overlap, "Tail first.")).len(), 1);
    assert_eq!(judge(&mut wider, repeat(across, "Wider.")).len(), 1);
    assert_eq!(repeat_rows(&wider), 2);
}
