//! The `tests` module of `src/livekit/session.rs`, which declares this file by
//! path.
//!
//! These moved here with the code they cover: they reach private items of the
//! session half, which a test module attached to the parent can no longer see.
//! Everything reaches in through `super`, so this is a unit test and not an
//! integration one.

use super::*;

// The room half's test module owns the audio fixture, because the room half
// owns the track it is a stand-in for.
use crate::livekit::tests::test_output_audio;

/// Closing a turn yields what to publish, once, and only for an open one.
///
/// The pair is the line and the segment it replaces, so an empty text or a
/// borrowed id publishes a turn that says nothing or overwrites another
/// one. Closing a turn nobody opened would publish a blank line for a
/// speaker who has not spoken.
#[test]
fn a_turn_closes_once_and_only_when_it_was_open() {
    let mut unopened = SpeakerTurn::default();
    assert!(
        close_turn(&mut unopened, "Candidate").is_none(),
        "a speaker who has not spoken has no line to publish"
    );

    let mut turn = SpeakerTurn::default();
    turn.record(&mut Vec::new(), "Candidate", "I will use a hash map.");
    let open_segment = turn.segment_id("Candidate");
    let (text, segment) = close_turn(&mut turn, "Candidate").expect("an open turn closes");
    assert_eq!(text, "I will use a hash map.");
    assert_eq!(
        segment, open_segment,
        "the id names the segment being replaced, not the one after it"
    );
    assert_ne!(
        turn.segment_id("Candidate"),
        open_segment,
        "and the next line is a new segment, not an overwrite of this one"
    );

    assert!(
        close_turn(&mut turn, "Candidate").is_none(),
        "a closed turn closes once; publishing it again repeats the line"
    );
}
/// The checklist is redrawn for a change the candidate can see, and for
/// nothing else.
///
/// Evidence for a phase they never reached is recorded but never shown, so
/// keying the publish on the evidence count sends a message whose phase
/// list is identical to the one already on screen. The browser unhides the
/// checklist on every `framework_state` it receives, so the first such
/// message reveals an empty, wholly unticked list before the candidate has
/// banked anything.
#[test]
fn the_checklist_is_republished_only_when_it_would_look_different() {
    let mut state = RuntimeState::default();
    let skip = serde_json::json!({
        "phase": "optimizations",
        "source": "session_timing",
        "kind": "skipped",
        "confidence": 0,
        "summary": "the session ended before optimizations",
    });

    let shown_before = framework_progress(&state);
    record_framework_evidence(&mut state, &skip).expect("evidence should record");
    assert_eq!(
        state.framework_evidence.len(),
        1,
        "the skip is still recorded for the report"
    );
    assert_eq!(
        framework_progress(&state),
        shown_before,
        "a skip changes nothing on screen, so it must not trigger a redraw"
    );

    // And the rule the publish is keyed on, which the assertion above cannot
    // see: same phases means no redraw, a new phase means one.
    assert!(
        !checklist_changed(&shown_before, &state),
        "a skip leaves the checklist looking exactly as it did"
    );
    record_framework_evidence(
        &mut state,
        &serde_json::json!({
            "phase": "repeat",
            "source": "candidate_speech",
            "kind": "observed",
            "confidence": 80,
            "summary": "restated the inputs and outputs",
        }),
    )
    .expect("evidence should record");
    assert!(
        checklist_changed(&shown_before, &state),
        "a phase the candidate reached is a new tick and has to be sent"
    );
}
/// A pause silences output for as long as it lasts, and nothing about the
/// event changes that.
#[test]
fn a_pause_drops_output_and_passes_everything_else_through() {
    let audio = GeminiEvent::Audio {
        bytes: vec![0],
        mime_type: "audio/pcm".to_string(),
    };

    assert_eq!(
        output_disposition(&audio, false, true),
        OutputDisposition::Drop
    );
    assert_eq!(
        output_disposition(
            &GeminiEvent::OutputTranscript("hi".to_string()),
            false,
            true
        ),
        OutputDisposition::Drop
    );

    // Not output, so never the thing a pause silences: a tool call still has to
    // be answered or Gemini waits on a response that is never sent.
    assert_eq!(
        output_disposition(&GeminiEvent::ToolCall(Vec::new()), false, true),
        OutputDisposition::Deliver
    );
    assert_eq!(
        output_disposition(&GeminiEvent::InputTranscript("hi".to_string()), false, true),
        OutputDisposition::Deliver
    );
}
/// A discard is one turn's sentence, not a standing condition, and the turn's
/// own end is what serves it.
#[test]
fn a_discard_lasts_exactly_one_turn() {
    let audio = GeminiEvent::Audio {
        bytes: vec![0],
        mime_type: "audio/pcm".to_string(),
    };

    assert_eq!(
        output_disposition(&audio, true, false),
        OutputDisposition::Drop
    );
    assert_eq!(
        output_disposition(&GeminiEvent::TurnComplete, true, false),
        OutputDisposition::EndsTheDiscard
    );
    assert_eq!(
        output_disposition(&GeminiEvent::Interrupted, true, false),
        OutputDisposition::EndsTheDiscard
    );

    // Nothing to serve once it is spent.
    assert_eq!(
        output_disposition(&audio, false, false),
        OutputDisposition::Deliver
    );
}
/// The ordering the two rules are checked in. A turn that ends while the
/// interview is still paused has to serve the discard, or the sentence outlives
/// the turn it belonged to and the next reply is dropped as well.
#[test]
fn a_turn_ending_under_a_pause_still_ends_the_discard() {
    assert_eq!(
        output_disposition(&GeminiEvent::TurnComplete, true, true),
        OutputDisposition::EndsTheDiscard
    );
}
/// The browser patches one row per segment id, so an in-progress turn has
/// to carry the same id it will carry when it closes.
#[test]
fn transcript_stream_options_carry_a_segment_id_and_final_flag() {
    let open = transcript_stream_options("interviewer-0".to_string(), false, None);

    assert_eq!(open.topic, TOPIC_TRANSCRIPTION);
    assert_eq!(
        open.attributes.get("lk.segment_id"),
        Some(&"interviewer-0".to_string())
    );
    assert_eq!(
        open.attributes.get("lk.transcription_final"),
        Some(&"false".to_string())
    );
    assert_eq!(open.sender_identity, None);

    let closed = transcript_stream_options("interviewer-0".to_string(), true, None);

    assert_eq!(
        closed.attributes.get("lk.segment_id"),
        Some(&"interviewer-0".to_string()),
        "closing a turn must not change the row it patches"
    );
    assert_eq!(
        closed.attributes.get("lk.transcription_final"),
        Some(&"true".to_string())
    );
}
#[test]
fn transcript_stream_options_can_preserve_candidate_identity() {
    let options =
        transcript_stream_options("candidate-0".to_string(), true, Some("candidate-fixed"));

    assert_eq!(
        options.sender_identity.as_ref().map(ToString::to_string),
        Some("candidate-fixed".to_string())
    );
}
#[test]
fn transcript_text_trims_and_drops_empty_events() {
    assert_eq!(transcript_text("  hello  "), Some("hello"));
    assert_eq!(transcript_text("  \n\t  "), None);
}
#[test]
fn agent_state_attributes_preserve_existing_values() {
    let attributes = agent_state_attributes(
        HashMap::from([("role".to_string(), "interviewer".to_string())]),
        AGENT_STATE_SPEAKING,
    );

    assert_eq!(attributes.get("role"), Some(&"interviewer".to_string()));
    assert_eq!(
        attributes.get(LIVEKIT_AGENT_STATE),
        Some(&AGENT_STATE_SPEAKING.to_string())
    );
}
/// The closing message is the one turn barge-in must not touch. Cutting it
/// leaves the candidate without the ending, and the wrap-up wait reads the
/// emptied queue as the turn being over, so the interview ended there.
#[test]
fn the_closing_message_is_not_cut_short_by_a_candidate_talking_over_it() {
    let (mut output_audio, _frames) = test_output_audio();
    let closing = output_audio.output_cancellation.clone();
    output_audio.playout_deadline = Instant::now() + Duration::from_secs(10);
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.floor = Floor::AwaitingPlayout;

    assert!(take_stale_playout(&mut activity, &mut output_audio, Interruptible::No).is_none());

    assert!(!closing.is_cancelled(), "the ending has to play out");
    assert!(output_audio.is_playing());
    assert_eq!(activity.floor, Floor::AwaitingPlayout);
}
/// The first candidate answer used to land behind the rest of the greeting.
/// Gemini streams a twenty second greeting in about two, marks the turn
/// complete, and then never reports an interruption, because from its side
/// that turn is long over. Only this process knows the queue is still
/// draining.
#[test]
fn a_candidate_speaking_over_a_draining_turn_drops_what_is_left_of_it() {
    let (mut output_audio, _frames) = test_output_audio();
    let stale = output_audio.output_cancellation.clone();
    output_audio.playout_deadline = Instant::now() + Duration::from_secs(10);
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.floor = Floor::AwaitingPlayout;

    let dropped = take_stale_playout(&mut activity, &mut output_audio, Interruptible::Yes)
        .expect("a draining turn must be dropped");
    assert!(
        dropped >= Duration::from_secs(9),
        "reports what it dropped: {dropped:?}"
    );

    assert!(
        stale.is_cancelled(),
        "queued greeting frames must be dropped"
    );
    assert!(!output_audio.is_playing(), "the deadline must come forward");
    assert_eq!(activity.floor, Floor::Listening);
}
/// The guard is two conditions and both are load bearing. `AwaitingPlayout`
/// is stamped once when the turn completes and outlives the queue it was
/// named for, so on its own it would cut into a turn that is only just
/// starting.
#[test]
fn nothing_is_dropped_once_the_queue_has_drained_or_while_the_agent_speaks() {
    for (floor, deadline, why) in [
        (
            Floor::AwaitingPlayout,
            Instant::now(),
            "queue already drained",
        ),
        (
            Floor::Speaking,
            Instant::now() + Duration::from_secs(10),
            "turn still being produced",
        ),
        (
            Floor::Listening,
            Instant::now() + Duration::from_secs(10),
            "candidate already holds the floor",
        ),
    ] {
        let (mut output_audio, _frames) = test_output_audio();
        let live = output_audio.output_cancellation.clone();
        output_audio.playout_deadline = deadline;
        let mut activity = RuntimeActivity::new(Instant::now());
        activity.floor = floor;

        assert!(
            take_stale_playout(&mut activity, &mut output_audio, Interruptible::Yes).is_none(),
            "{why}"
        );
        assert!(!live.is_cancelled(), "{why}");
        assert_eq!(activity.floor, floor, "{why}");
    }
}

/// A hint the candidate asked for comes back with their editor, so fitting
/// the clue to their code needs no `read_editor` first; one the interviewer
/// volunteered is only recorded, after it was given.
#[test]
fn a_requested_hint_returns_the_editor_with_its_clue() {
    let mut state = RuntimeState {
        code: "def two_sum(nums, target):\n    return []".to_string(),
        language: "python".to_string(),
        hint_ladder: &["first rung", "second rung"],
        ..RuntimeState::default()
    };
    let call = |requested: bool| GeminiFunctionCall {
        id: "1".to_string(),
        name: TOOL_LOG_HINT.to_string(),
        args: serde_json::json!({ "requested": requested }),
    };
    let asked = execute_tool_call(&mut state, &call(true));
    let asked = asked["result"].as_str().unwrap();
    assert!(asked.contains("first rung"), "{asked}");
    assert!(asked.contains("  2|     return []"), "{asked}");
    assert!(position_of(asked, "first rung") < position_of(asked, "BEGIN UNTRUSTED EDITOR"));

    // The candidate's text is fenced: everything they wrote sits between the
    // markers the instructions tell the model never to take orders from, and
    // the platform's timer comes after both fences, as the last sentence.
    let (open, close) = (
        position_of(asked, "BEGIN UNTRUSTED EDITOR (python)\n"),
        position_of(asked, "\nEND UNTRUSTED EDITOR"),
    );
    assert!(open < position_of(asked, "  2|     return []"));
    assert!(position_of(asked, "  2|     return []") < close);
    assert!(close < position_of(asked, "END UNTRUSTED TEST RUN"));
    assert!(
        asked.ends_with("minutes remain on the candidate's countdown."),
        "{asked}"
    );

    let volunteered = execute_tool_call(&mut state, &call(false));
    assert!(
        !volunteered["result"]
            .as_str()
            .unwrap()
            .contains("Editor language")
    );
}

fn position_of(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in:\n{text}"))
}

#[test]
fn execute_tool_call_reads_editor_and_tracks_hints() {
    let mut state = RuntimeState {
        code: "def two_sum(nums, target):\n    return [0, 1]".to_string(),
        language: "python".to_string(),
        last_test_run: Some(serde_json::json!({
            "language": "python",
            "passed": 1,
            "total": 2,
            "failures": [],
        })),
        test_runs: 1,
        ..RuntimeState::default()
    };

    let editor = execute_tool_call(
        &mut state,
        &GeminiFunctionCall {
            id: "1".to_string(),
            name: TOOL_READ_EDITOR.to_string(),
            args: serde_json::json!({}),
        },
    );
    let hint = execute_tool_call(
        &mut state,
        &GeminiFunctionCall {
            id: "2".to_string(),
            name: TOOL_LOG_HINT.to_string(),
            args: serde_json::json!({"requested": false}),
        },
    );
    let evidence = execute_tool_call(
        &mut state,
        &GeminiFunctionCall {
            id: "3".to_string(),
            name: TOOL_RECORD_FRAMEWORK_EVIDENCE.to_string(),
            args: serde_json::json!({
                "phase":"algorithm", "source":"candidate_speech", "kind":"observed",
                "confidence":90, "summary":"Candidate explained the invariant."
            }),
        },
    );

    assert!(
        editor["result"]
            .as_str()
            .unwrap()
            .contains("BEGIN UNTRUSTED EDITOR (python)")
    );
    assert!(
        editor["result"]
            .as_str()
            .unwrap()
            .contains("Latest test run")
    );
    assert_eq!(hint["result"], "Recorded. Total hints so far: 1.");

    // A call that leaves the flag out is read as asked for, so a candidate
    // whose request the model logged carelessly still gets the next rung.
    let mut laddered = RuntimeState {
        hint_ladder: &["first rung", "second rung", "third rung"],
        ..RuntimeState::default()
    };
    let unflagged = execute_tool_call(
        &mut laddered,
        &GeminiFunctionCall {
            id: "5".to_string(),
            name: TOOL_LOG_HINT.to_string(),
            args: serde_json::json!({}),
        },
    );
    assert!(unflagged["result"].as_str().unwrap().contains("first rung"));
    assert_eq!(laddered.hint_rungs_given, 1);
    assert_eq!(state.hints_used, 1);
    assert_eq!(evidence["result"]["phase"], "algorithm");
    assert_eq!(state.framework_evidence.len(), 1);
    assert_eq!(state.evidence_ledger.metrics.read_editor_calls, 1);
    assert!(state.evidence_ledger.metrics.read_editor_bytes > 0);

    // The hint and the evidence are model input as well, and each is counted as
    // a tool answer rather than as an editor read. The unflagged hint above ran
    // against its own state, so it is not among them.
    assert_eq!(state.evidence_ledger.metrics.tool_response_count, 2);
    assert!(state.evidence_ledger.metrics.tool_response_bytes > 0);

    // An interview with one phase of evidence is not a finished interview, and
    // the model saying so does not make it one.
    let refused = execute_tool_call(
        &mut state,
        &GeminiFunctionCall {
            id: "4".to_string(),
            name: TOOL_END_INTERVIEW.to_string(),
            args: serde_json::json!({}),
        },
    );
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("Test and Optimizations evidence"),
        "closing the session early is the one mistake here nobody can undo"
    );
    assert!(!state.end_requested);

    for phase in ["test", "optimizations"] {
        record_framework_evidence(
            &mut state,
            &serde_json::json!({
                "phase":phase, "source":"candidate_speech", "kind":"observed",
                "confidence":90, "summary":format!("Candidate finished {phase}.")
            }),
        )
        .unwrap();
    }

    // The coding evidence may arrive before the browser opens the behavioral
    // reserve. Letting the model close in that interval makes the required
    // round unreachable, so only a started or explicitly skipped reserve
    // permits the two-round interview to finish.
    let reserve_pending = execute_tool_call(
        &mut state,
        &GeminiFunctionCall {
            id: "5".to_string(),
            name: TOOL_END_INTERVIEW.to_string(),
            args: serde_json::json!({}),
        },
    );
    assert!(
        reserve_pending["error"]
            .as_str()
            .unwrap()
            .contains("behavioral reserve has not started or been skipped")
    );
    assert!(!state.end_requested);

    // Nothing here ends anything. The tool records a request and the room loop
    // reads it on the way out of the event that carried it, because ending
    // means publishing a report and leaving a room, and this function has
    // neither in scope.
    state.round_transition_seen = true;
    let ending = execute_tool_call(
        &mut state,
        &GeminiFunctionCall {
            id: "6".to_string(),
            name: TOOL_END_INTERVIEW.to_string(),
            args: serde_json::json!({}),
        },
    );
    assert!(state.end_requested);
    assert!(!state.ended, "the request is not the ending");
    assert!(
        ending["result"]
            .as_str()
            .unwrap()
            .contains("Say nothing further"),
        "Gemini owes a generation for every tool response, and the closing is \
         about to be prompted for: without this Jim says goodbye twice"
    );

    // Three more, and two of them were refusals that returned early. A count
    // taken inside the arms would have skipped exactly those, and a refusal is
    // text the model reads as surely as an answer is.
    assert_eq!(state.evidence_ledger.metrics.tool_response_count, 5);
    assert_eq!(state.evidence_ledger.metrics.read_editor_calls, 1);
}
/// The follow-ups arrive with the evidence that completes the coding round,
/// once, and not before: the live prompt no longer holds them.
#[test]
fn the_evidence_that_completes_coding_releases_the_follow_ups_once() {
    let problem = crate::agent::get_problem(Some("two-sum"));
    let mut state = RuntimeState {
        code: "def solve(nums):\n    return sorted(nums)\n".to_string(),
        ..RuntimeState::for_problem(problem)
    };
    state
        .code_templates
        .insert("python".to_string(), String::new());
    let record = |state: &mut RuntimeState, phase: &str| {
        execute_tool_call(
            state,
            &GeminiFunctionCall {
                id: phase.to_string(),
                name: TOOL_RECORD_FRAMEWORK_EVIDENCE.to_string(),
                args: serde_json::json!({
                    "phase": phase, "source": "candidate_speech", "kind": "observed",
                    "confidence": 90, "summary": format!("Candidate finished {phase}.")
                }),
            },
        )
    };
    let first = problem.variant().follow_ups[0];

    let tested = record(&mut state, "test");
    assert!(
        tested.get("followUps").is_none(),
        "Test alone completes nothing"
    );
    let optimized = record(&mut state, "optimizations");
    let released = optimized["followUps"]
        .as_str()
        .expect("the completing call releases them");
    assert!(released.contains(first) && released.contains("at most two of these"));
    let again = record(&mut state, "test");
    assert!(
        again.get("followUps").is_none(),
        "released once, not per note"
    );

    // A cold restart after the round completed hands them over again, since the
    // replacement session never saw that tool response.
    let restarted = crate::agent::cold_restart(&state);
    assert!(restarted.contains(first));
    assert!(
        restarted.contains("Do not ask another coding question")
            && !restarted.contains("The coding round is active"),
        "a completed round must not read as one still in progress"
    );

    // Once the behavioral round has begun the follow-ups are behind it: the
    // restart names the round and nothing sends the interviewer back.
    let behavioral = RuntimeState {
        behavioral_round_started: true,
        ..state.clone()
    };
    let restarted = crate::agent::cold_restart(&behavioral);
    assert!(restarted.contains("The behavioral round has just opened"));
    assert!(
        !restarted.contains(first) && !restarted.contains("follow-ups"),
        "the behavioral round must not be pointed back at coding follow-ups"
    );
    let fresh = RuntimeState::for_problem(problem);
    assert!(!crate::agent::cold_restart(&fresh).contains(first));
}
#[test]
fn end_interview_allows_a_completed_coding_only_plan() {
    let mut state = RuntimeState {
        interview_loop: crate::agent::InterviewLoop::CodingOnly,
        code: "def solve(nums):\n    return sorted(nums)\n".to_string(),
        ..RuntimeState::default()
    };
    for phase in ["test", "optimizations"] {
        record_framework_evidence(
            &mut state,
            &serde_json::json!({
                "phase": phase, "source": "candidate_speech", "kind": "observed",
                "confidence": 90, "summary": format!("Candidate finished {phase}.")
            }),
        )
        .unwrap();
    }

    let ending = execute_tool_call(
        &mut state,
        &GeminiFunctionCall {
            id: "1".to_string(),
            name: TOOL_END_INTERVIEW.to_string(),
            args: serde_json::json!({}),
        },
    );

    assert!(ending["result"].is_string());
    assert!(state.end_requested);
}

/// The reply that first ticks a later step names the earlier ones still open,
/// each once, and never names a step that already has a row, a skip included.
#[test]
fn evidence_reply_names_the_earlier_steps_still_open() {
    let mut state = RuntimeState {
        code: "def solve(nums):\n    return sorted(nums)\n".to_string(),
        ..RuntimeState::default()
    };
    let mut record = |phase: &str, kind: &str, source: &str, summary: &str| {
        execute_tool_call(
            &mut state,
            &GeminiFunctionCall {
                id: "1".to_string(),
                name: TOOL_RECORD_FRAMEWORK_EVIDENCE.to_string(),
                args: serde_json::json!({
                    "phase": phase, "source": source, "kind": kind,
                    "confidence": 90, "summary": summary,
                }),
            },
        )
    };

    let first = record("repeat", "observed", "candidate_speech", "Restated it.");
    assert!(first["result"].is_object());
    assert!(
        first.get("earlierSteps").is_none(),
        "nothing comes before Repeat"
    );

    let coding = record("coding", "observed", "editor_snapshot", "Wrote a sort.");
    let reminder = coding["earlierSteps"]
        .as_str()
        .expect("Coding skipped two steps");
    assert!(reminder.contains("example, algorithm"), "{reminder}");
    assert!(
        !reminder.contains("repeat"),
        "Repeat is already ticked: {reminder}"
    );
    assert!(reminder.contains("If they skipped it, record nothing"));

    // Coding is already ticked, so a second note on it is not a new gap.
    let again = record("coding", "observed", "editor_snapshot", "Added a guard.");
    assert!(again.get("earlierSteps").is_none());

    // Example comes before Algorithm, so the Algorithm gap is not its to name.
    let example = record("example", "observed", "candidate_speech", "Walked [1].");
    assert!(example.get("earlierSteps").is_none());

    // Algorithm is still open, but Coding already named it: the candidate may
    // have skipped it, and asking again leaves inventing it as the only answer.
    let test = record("test", "observed", "candidate_speech", "Predicted [].");
    assert!(test.get("earlierSteps").is_none(), "{test}");

    // A skip ticks nothing, so it has no gap to report, even with Situation and
    // Task open and never named.
    let action = record("action", "skipped", "session_timing", "Out of time.");
    assert!(action.get("earlierSteps").is_none(), "{action}");

    // A step closed out as skipped has a row, so it is not missing.
    let situation = record("situation", "skipped", "session_timing", "Out of time.");
    assert!(situation.get("earlierSteps").is_none());

    // STAR is its own list: a REACTO gap is not named on a STAR step. Task is
    // still named here, because the skip above named nothing.
    let result = record("result", "observed", "candidate_speech", "Shipped it.");
    let star = result["earlierSteps"]
        .as_str()
        .expect("STAR steps are open");
    assert!(star.contains(": task."), "{star}");
    assert!(!star.contains("situation"), "{star}");
    assert!(!star.contains("action"), "{star}");
    assert!(!star.contains("algorithm"), "{star}");
}
#[test]
fn execute_tool_call_reports_unknown_tools() {
    let mut state = RuntimeState::default();

    assert_eq!(
        execute_tool_call(
            &mut state,
            &GeminiFunctionCall {
                id: "1".to_string(),
                name: "missing".to_string(),
                args: serde_json::json!({}),
            },
        ),
        serde_json::json!({"error":"unknown tool: missing"})
    );
}
