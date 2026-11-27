//! The `tests` module of `src/agent/key_moments.rs`, which declares this file
//! by path, so the picking and the quote check are tested without a room.

use super::*;
use serde_json::json;

/// When the synthetic session began on the ledger's clock.
const STARTED: u64 = 1_000_000;

/// A session that says `lines` in order, each turn closing at its time.
fn session(lines: &[(&str, &str, u64)]) -> RuntimeState {
    let mut state = RuntimeState::default();
    for (speaker, words, at_ms) in lines {
        state.transcript.push(format!("{speaker}: {words}"));
        let speaker = if *speaker == CANDIDATE_SPEAKER {
            "candidate"
        } else {
            "interviewer"
        };
        state
            .evidence_ledger
            .record_conversation_turn(STARTED + at_ms, speaker);
    }
    state
}

fn observed(phase: FrameworkPhase, quote: &str, at_ms: u64) -> FrameworkEvidence {
    FrameworkEvidence {
        at_ms,
        phase,
        source: EvidenceSource::CandidateSpeech,
        kind: EvidenceKind::Observed,
        confidence: 70,
        summary: format!("Stated it. Quote: \"{quote}\""),
        framework_version: FRAMEWORK_VERSION,
    }
}

fn python(code: &str) -> TestedCode {
    TestedCode {
        language: "python".to_string(),
        code: code.to_string(),
    }
}

/// A line is timed by its own speaker's turns, so a candidate turn that
/// closed after the interviewer's next one still gets its own time.
#[test]
fn lines_are_timed_by_their_speakers_turns() {
    let mut state = RuntimeState {
        transcript: vec![
            "Interviewer: Tell me your plan.".to_string(),
            "Candidate: I would use a hash map".to_string(),
            "Interviewer: Go on.".to_string(),
        ],
        ..RuntimeState::default()
    };
    let ledger = &mut state.evidence_ledger;
    ledger.record_conversation_turn(STARTED + 1_000, "interviewer");
    ledger.record_conversation_turn(STARTED + 9_000, "interviewer");
    ledger.record_conversation_turn(STARTED + 10_000, "candidate");
    assert_eq!(
        transcript_line_times(&state, STARTED),
        vec![Some(1_000), Some(10_000), Some(9_000)]
    );
}

/// A turn the ledger has let go of leaves its line untimed rather than
/// lending it the time of a later turn, and a line still open has no time.
#[test]
fn a_dropped_or_open_turn_leaves_its_line_untimed() {
    let mut state = session(&[
        (CANDIDATE_SPEAKER, "first answer here", 1_000),
        (CANDIDATE_SPEAKER, "second answer here", 2_000),
    ]);
    state.evidence_ledger.entries.remove(0);
    state
        .transcript
        .push("Candidate: still talking".to_string());
    assert_eq!(
        transcript_line_times(&state, STARTED),
        vec![None, Some(2_000), None]
    );
}

/// The approach is timed by the line it was said on, not by when the row was
/// written, and a hint and a fixed run become moments of their own.
#[test]
fn the_rules_pick_moments_at_event_time() {
    let mut state = session(&[
        (INTERVIEWER_SPEAKER, "How would you start?", 10_000),
        (
            CANDIDATE_SPEAKER,
            "I would keep a dictionary of the amounts seen so far",
            40_000,
        ),
        (CANDIDATE_SPEAKER, "can I have a hint please", 100_000),
    ]);
    state.framework_evidence.push(observed(
        FrameworkPhase::Algorithm,
        "keep a dictionary of the amounts seen so far",
        300_000,
    ));
    state.key_moments.events = vec![
        MomentEvent {
            at_ms: 101_000,
            kind: MomentEventKind::HintRequested { given: true },
        },
        MomentEvent {
            at_ms: 200_000,
            kind: MomentEventKind::FixedRun {
                failing: python("seen[x] = i\nif target - x in seen:"),
                fixed_at_ms: 230_000,
            },
        },
    ];
    let moments = pick_moments(&state, STARTED);
    let picked = moments
        .iter()
        .map(|moment| (moment.id.as_str(), moment.kind, moment.at_ms))
        .collect::<Vec<_>>();
    assert_eq!(
        picked,
        vec![
            ("m1", MomentKind::Approach, 40_000),
            ("m2", MomentKind::Hint, 101_000),
            ("m3", MomentKind::FixedRun, 200_000),
        ]
    );
    assert_eq!(moments[0].candidate_lines, vec![1]);
    assert_eq!(moments[1].candidate_lines, vec![2]);
    assert!(moments[2].code.is_some());
}

/// Past the cap the hints give way, the later ones first.
#[test]
fn hints_give_way_past_the_cap() {
    let mut state = session(&[(
        CANDIDATE_SPEAKER,
        "I would keep a dictionary of the amounts seen so far",
        5_000,
    )]);
    state.framework_evidence.push(observed(
        FrameworkPhase::Algorithm,
        "keep a dictionary of the amounts seen so far",
        5_000,
    ));
    state.key_moments.events = (0..8)
        .map(|index| MomentEvent {
            at_ms: 10_000 * (index + 1),
            kind: MomentEventKind::HintRequested {
                given: index % 2 == 0,
            },
        })
        .collect();
    let moments = pick_moments(&state, STARTED);
    assert_eq!(moments.len(), MAX_KEY_MOMENTS);
    assert_eq!(moments[0].kind, MomentKind::Approach);
    assert_eq!(moments.last().map(|moment| moment.at_ms), Some(50_000));
}

/// A failing run is held until a run passes; the first fix is the moment and
/// later runs add none.
#[test]
fn only_the_first_fixed_run_is_a_moment() {
    let mut state = RuntimeState::default();
    state.record_run_event(&python("broken"), false);
    state.record_run_event(&python("still broken"), false);
    assert!(state.key_moments.events.is_empty());
    state.record_run_event(&python("fixed"), true);
    state.record_run_event(&python("broken again"), false);
    state.record_run_event(&python("fixed again"), true);
    let fixes = state
        .key_moments
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MomentEventKind::FixedRun { failing, .. } => Some(failing.code.as_str()),
            MomentEventKind::HintRequested { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(fixes, vec!["broken"]);
}

/// Only a request is a moment; a clue the interviewer volunteered is not.
#[test]
fn a_volunteered_hint_is_not_a_moment() {
    let mut state = RuntimeState::default();
    state.record_hint_event(STARTED, false, true);
    assert!(state.key_moments.events.is_empty());
    state.record_hint_event(STARTED, true, false);
    assert_eq!(
        state.key_moments.events.first().map(|event| &event.kind),
        Some(&MomentEventKind::HintRequested { given: false })
    );
}

fn request() -> KeyMomentsRequest {
    let mut state = session(&[
        (
            INTERVIEWER_SPEAKER,
            "What happens when the target is twice one amount?",
            190_000,
        ),
        (
            CANDIDATE_SPEAKER,
            "oh three matched itself because I inserted before the lookup",
            205_000,
        ),
        (
            CANDIDATE_SPEAKER,
            "much later I talked about something else entirely here",
            400_000,
        ),
    ]);
    state.key_moments.events = vec![MomentEvent {
        at_ms: 200_000,
        kind: MomentEventKind::FixedRun {
            failing: python("for i, x in enumerate(nums):\n    seen[x] = i\n"),
            fixed_at_ms: 230_000,
        },
    }];
    key_moments_request(&state, "Chargeback Pair Match", "Two Sum", STARTED).expect("one moment")
}

/// The prompt names the moment, its time and the fenced lines and code it
/// may quote, and nothing from outside its window.
#[test]
fn the_prompt_shows_each_moment_and_its_window() {
    let prompt = request().prompt;
    assert!(prompt.starts_with("Problem: Chargeback Pair Match\n"));
    assert!(prompt.contains("Moment m1 at 03:20: A test run failed"));
    assert!(prompt.contains("The passing run was at 03:50."));
    assert!(prompt.contains("BEGIN UNTRUSTED TRANSCRIPT"));
    assert!(prompt.contains("three matched itself"));
    assert!(prompt.contains("BEGIN UNTRUSTED CODE (python, as the failing run ran it)"));
    assert!(!prompt.contains("something else entirely"));
}

/// A note is kept when it quotes the candidate's line in the window or the
/// failing code; one quoting the interviewer, another window, too few words,
/// code with its operator changed, or a moment the rules did not pick is
/// dropped.
#[test]
fn only_grounded_notes_are_kept() {
    let request = request();
    let answer = |quote: &str, moment: &str| {
        json!({"notes": [{"moment": moment, "quote": quote, "note": "Check the lookup first."}]})
            .to_string()
    };
    for (quote, moment) in [
        ("three matched itself because I inserted", "m1"),
        ("for i, x in enumerate(nums)", "m1"),
    ] {
        assert!(request.apply(&answer(quote, moment)).is_some(), "{quote}");
    }
    for (quote, moment) in [
        ("when the target is twice one amount", "m1"),
        ("I talked about something else", "m1"),
        ("matched itself", "m1"),
        ("for i, x in enumerate(nums)!", "m1"),
        ("for i x in enumerate nums", "m1"),
        ("three matched itself because I inserted", "m2"),
    ] {
        assert!(request.apply(&answer(quote, moment)).is_none(), "{quote}");
    }
    assert!(request.apply("not json").is_none());
}

/// What the report carries: the moment's time and kind beside the checked
/// quote and the note, one note per moment.
#[test]
fn kept_notes_take_the_moments_time_and_kind() {
    let answer = json!({"notes": [
        {"moment": "m1", "quote": "three matched itself because I inserted", "note": "Look up before you insert."},
        {"moment": "m1", "quote": "three matched itself because I inserted", "note": "A second note."},
    ]})
    .to_string();
    assert_eq!(
        request().apply(&answer),
        Some(json!([{
            "atMs": 200_000,
            "kind": "fixed_run",
            "quote": "three matched itself because I inserted",
            "note": "Look up before you insert.",
        }]))
    );
}

/// No moment means no call: a session with nothing the rules pick asks for
/// no notes.
#[test]
fn a_session_without_moments_makes_no_request() {
    let state = session(&[(CANDIDATE_SPEAKER, "hello there", 1_000)]);
    assert!(key_moments_request(&state, "Any", "", STARTED).is_none());
}

/// A pass in another language is a different program: it does not fix the
/// failure, which stays waiting for a pass in its own language.
#[test]
fn a_pass_in_another_language_is_not_the_fix() {
    let mut state = RuntimeState::default();
    state.record_run_event(&python("broken"), false);
    let java = TestedCode {
        language: "java".to_string(),
        code: "class Solution {}".to_string(),
    };
    state.record_run_event(&java, true);
    assert!(state.key_moments.events.is_empty());
    assert!(state.key_moments.failing_run.is_some());
    state.record_run_event(&python("fixed"), true);
    assert_eq!(state.key_moments.events.len(), 1);
}

/// Hints fill the event cap; the one fixed run is still kept.
#[test]
fn hints_cannot_crowd_out_the_fixed_run() {
    let mut state = RuntimeState::default();
    for _ in 0..MAX_MOMENT_EVENTS + 4 {
        state.record_hint_event(STARTED, true, true);
    }
    assert_eq!(state.key_moments.events.len(), MAX_MOMENT_EVENTS);
    state.record_run_event(&python("broken"), false);
    state.record_run_event(&python("fixed"), true);
    assert!(
        state
            .key_moments
            .events
            .iter()
            .any(|event| matches!(event.kind, MomentEventKind::FixedRun { .. }))
    );
}

/// A note naming the published problem, in its text or its quote, is dropped
/// like any other candidate-facing text that does, and so is a quote too long
/// to show whole.
#[test]
fn a_note_naming_the_published_problem_or_quoting_too_much_is_dropped() {
    let request = request();
    let note = |quote: &str, text: &str| {
        json!({"notes": [{"moment": "m1", "quote": quote, "note": text}]}).to_string()
    };
    let grounded = "three matched itself because I inserted";
    assert!(
        request
            .apply(&note(grounded, "This is the classic Two Sum trap."))
            .is_none()
    );
    let long = format!("{grounded} {}", "x ".repeat(MAX_NOTE_QUOTE_CHARS));
    assert!(request.apply(&note(&long, "Check first.")).is_none());
    assert!(request.apply(&note(grounded, "Check first.")).is_some());
}

/// The browser keeps a moment only of a kind it knows and only up to the cap,
/// so both have to agree with what this module sends.
#[test]
fn the_browser_reads_the_same_cap_and_kinds() {
    let browser = std::fs::read_to_string("web/lib.js").expect("web/lib.js is readable");
    let after = |declaration: &str, end: char| {
        let start = browser
            .find(declaration)
            .unwrap_or_else(|| panic!("web/lib.js declares {declaration}"))
            + declaration.len();
        let rest = &browser[start..];
        rest[..rest.find(end).expect("the declaration ends")].to_string()
    };
    let cap: usize = after("export const MAX_KEY_MOMENTS = ", ';')
        .trim()
        .parse()
        .expect("MAX_KEY_MOMENTS is a number");
    assert_eq!(cap, MAX_KEY_MOMENTS);
    let kinds = after("export const KEY_MOMENT_KINDS = [", ']');
    let kinds = kinds
        .split(',')
        .map(|kind| kind.trim().trim_matches('"'))
        .filter(|kind| !kind.is_empty())
        .collect::<Vec<_>>();

    // Every kind, by a match that stops compiling when one is added.
    let all = [
        MomentKind::Hint,
        MomentKind::HintWithheld,
        MomentKind::FixedRun,
        MomentKind::Approach,
        MomentKind::Complexity,
    ];
    for kind in all {
        match kind {
            MomentKind::Hint
            | MomentKind::HintWithheld
            | MomentKind::FixedRun
            | MomentKind::Approach
            | MomentKind::Complexity => {}
        }
    }
    assert_eq!(kinds, all.iter().map(|kind| kind.id()).collect::<Vec<_>>());
}

/// A cut interviewer line reaches the prompt as the marker every other model
/// reader sees, not as the words that may never have been heard.
#[test]
fn a_cut_interviewer_line_is_masked_in_the_prompt() {
    let mut state = session(&[
        (
            INTERVIEWER_SPEAKER,
            "Try storing the complement first",
            190_000,
        ),
        (
            CANDIDATE_SPEAKER,
            "oh three matched itself because I inserted before the lookup",
            205_000,
        ),
    ]);
    state.interviewer_delivery.uncertain.insert(0);
    state.key_moments.events = vec![MomentEvent {
        at_ms: 200_000,
        kind: MomentEventKind::HintRequested { given: true },
    }];
    let prompt = key_moments_request(&state, "Any", "", STARTED)
        .expect("one moment")
        .prompt;
    assert!(!prompt.contains("storing the complement"));
    assert!(prompt.contains(CANCELLED_INTERVIEWER_OUTPUT));
    assert!(prompt.contains("three matched itself"));
}
