//! The `tests` module of `src/livekit.rs`, which declares this file by path.
//! Everything here reaches into `src/livekit.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;

// Only the tests below reach for it now: the report packet that carries this
// topic is built in `report.rs`, and what is left here asserts which topics the
// loop refuses from a browser.
use crate::runtime::TOPIC_REPORT;

use crate::config::load_from_pairs;

/// The interview starts from the plan its token was minted for.
///
/// None of this can be recovered once it is missed: the clock the round
/// boundary is measured against, the loop that decides whether there is a
/// behavioral round at all, and the budgets the report divides the session
/// into. A field dropped here is a default carried for the whole hour.
#[test]
fn the_interview_begins_from_the_plan_it_was_booked_with() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google-key"),
    ])
    .unwrap();

    // A plan that differs from the default in every field, because the default
    // is the forty-five minute two-round one: booked with that, a field dropped
    // here still reads correctly and the test proves nothing.
    let boot = crate::runtime::bootstrap_with_rounds(
        &config,
        "interview-fixed",
        Some("two-sum"),
        30,
        crate::runtime::RuntimeOptions {
            interview_loop: crate::agent::InterviewLoop::CodingOnly,
            ..crate::runtime::RuntimeOptions::default()
        },
    );
    let untouched = RuntimeState::default();
    for (named, same) in [
        (
            "interview_loop",
            boot.interview_loop == untouched.interview_loop,
        ),
        (
            "coding_minutes",
            boot.coding_minutes == untouched.coding_minutes,
        ),
        (
            "behavioral_minutes",
            boot.behavioral_minutes == untouched.behavioral_minutes,
        ),
    ] {
        assert!(
            !same,
            "{named} matches the default, so dropping it is invisible"
        );
    }

    let started_at = Instant::now() - Duration::from_secs(300);
    let state = initial_runtime_state(&boot, started_at);

    assert_eq!(
        state.started_at, started_at,
        "the clock is the room's, not now"
    );
    assert_eq!(state.interview_loop, boot.interview_loop);
    assert_eq!(state.coding_minutes, boot.coding_minutes);
    assert_eq!(state.behavioral_minutes, boot.behavioral_minutes);
}

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

/// A packet the browser never receives is the same as one never sent.
///
/// The topic is the channel it listens on and reliability is whether a bad
/// network may drop it, and both were written out at each call site where
/// either could go missing on its own. The report and the control messages
/// differ only in which topic they name.
#[test]
fn a_published_packet_carries_its_topic_and_arrives_reliably() {
    for topic in [TOPIC_CONTROL, TOPIC_REPORT] {
        let packet = browser_packet(topic, &serde_json::json!({"type": "pause_state"}))
            .expect("a json object serializes");
        assert_eq!(
            packet.topic.as_deref(),
            Some(topic),
            "no topic is a channel nobody is reading"
        );
        assert!(packet.reliable, "the browser only gets one of these");
        let payload: serde_json::Value =
            serde_json::from_slice(&packet.payload).expect("the payload is the message");
        assert_eq!(payload["type"], "pause_state");
    }
}

/// The interview starts once, when the candidate joined.
///
/// Stamping a second `Instant::now()` after the room and the Gemini session
/// came up put this process a second or two behind the tab for the rest of
/// the interview, and the browser announces the round boundary exactly once
/// off its own clock. The gap has to be absorbed by a tolerance nobody can
/// justify, or removed here. Read as source because the two stamps are only
/// distinguishable in a room that really connected.
#[test]
fn the_interview_clock_starts_when_the_candidate_joined() {
    let source = include_str!("../../src/livekit.rs");
    let opener = source
        .split("async fn open_session")
        .nth(1)
        .expect("open_session is still defined here");
    let bound = opener
        .split_once("let started_at =")
        .expect("open_session still stamps the interview clock")
        .1
        .lines()
        .next()
        .unwrap_or_default();
    assert!(
        bound.contains("setup_began"),
        "the clock must start where the candidate's does, not after setup: {bound}"
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

#[test]
fn only_the_interview_candidate_can_drive_the_runtime() {
    assert!(is_interview_participant(
        Some("candidate-abc"),
        "candidate-abc"
    ));

    // A peer sharing the room must not be able to end the interview, rewrite
    // the editor, or speak to the interviewer as the candidate.
    assert!(!is_interview_participant(
        Some("candidate-xyz"),
        "candidate-abc"
    ));
    assert!(
        !is_interview_participant(None, "candidate-abc"),
        "an unattributed event drives nothing: this agent never sends itself packets"
    );
}

/// The three guards a data packet passes before it can drive anything, in
/// one place because they used to be three `continue`s that looked alike.
#[test]
fn only_the_candidates_parseable_packet_on_a_topic_drives_the_runtime() {
    let good = br#"{"type":"end_interview"}"#;

    let (topic, payload) = interview_packet(
        Some(TOPIC_CONTROL),
        Some("candidate-abc"),
        "candidate-abc",
        good,
    )
    .expect("the candidate's own JSON on a topic is the whole point");
    assert_eq!(topic, TOPIC_CONTROL);
    assert_eq!(payload["type"], "end_interview");

    // LiveKit makes the topic optional, and every handler downstream keys off
    // it, so a packet without one is addressed to nothing.
    assert!(interview_packet(None, Some("candidate-abc"), "candidate-abc", good).is_none());

    // The interesting half of the gate: a bystander in the room must not be
    // able to end the interview or rewrite the editor.
    assert!(
        interview_packet(
            Some(TOPIC_CONTROL),
            Some("candidate-xyz"),
            "candidate-abc",
            good
        )
        .is_none()
    );
    assert!(interview_packet(Some(TOPIC_CONTROL), None, "candidate-abc", good).is_none());

    // Dropped rather than propagated: the sender is the browser we ship, so a
    // body that will not parse is a bug to fix rather than an interview to end
    // under the candidate.
    assert!(
        interview_packet(
            Some(TOPIC_CONTROL),
            Some("candidate-abc"),
            "candidate-abc",
            b"not json",
        )
        .is_none()
    );
}

/// Ending an interview because a tab reloaded, and keeping a metered Gemini
/// session open because a return never cleared the clock, are the two ways
/// this goes wrong. Both used to be spread across three `select!` arms with
/// nothing asserting they agreed.
#[test]
fn the_absence_grace_starts_on_departure_and_a_return_clears_it() {
    let start = Instant::now();
    let mut presence = CandidatePresence::default();

    // Nobody has left, so no amount of elapsed time ends anything.
    assert!(!presence.gave_up(start + CANDIDATE_ABSENCE_LIMIT * 10));

    presence.left(start);
    assert!(
        !presence.gave_up(start),
        "the grace starts, it does not expire"
    );
    assert!(
        !presence.gave_up(start + CANDIDATE_ABSENCE_LIMIT - Duration::from_secs(1)),
        "a page refresh is inside the grace and must not end the interview"
    );

    // The boundary is the tick the watch would fire on, so it has to be
    // inclusive: an exclusive one waits an extra whole tick.
    assert!(presence.gave_up(start + CANDIDATE_ABSENCE_LIMIT));

    presence.returned();
    assert!(
        !presence.gave_up(start + CANDIDATE_ABSENCE_LIMIT * 10),
        "coming back cancels the grace outright rather than pausing it"
    );
}

#[test]
fn candidate_bootstrap_uses_participant_metadata() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google-key"),
    ])
    .unwrap();

    let boot = candidate_bootstrap(
        &config,
        "interview-fixed",
        Some(
            r#"{"problemId":"merge-intervals","durationMin":30,"interviewLoop":"coding_only","interviewProfile":{"role":"Platform engineer","seniority":"staff","targetCompany":"Example Co"}}"#,
        ),
    );

    assert_eq!(boot.problem.id, "merge-intervals");
    assert_eq!(boot.duration_min, 30);
    assert_eq!(boot.interview_loop, crate::agent::InterviewLoop::CodingOnly);
    assert_eq!((boot.coding_minutes, boot.behavioral_minutes), (30, 0));
    assert_eq!(boot.profile.role, "Platform engineer");
    assert_eq!(boot.profile.target_company, "Example Co");
    assert!(boot.instructions.contains("candidate selected staff"));
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
            args: serde_json::json!({}),
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
            .contains("Editor language: python")
    );
    assert!(
        editor["result"]
            .as_str()
            .unwrap()
            .contains("Latest test run")
    );
    assert_eq!(hint["result"], "Recorded. Total hints so far: 1.");
    assert_eq!(state.hints_used, 1);
    assert_eq!(evidence["result"]["phase"], "algorithm");
    assert_eq!(state.framework_evidence.len(), 1);
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

#[test]
fn runtime_activity_emits_periodic_prompts_and_updates_gates() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    let mut state = RuntimeState {
        code: "def two_sum(nums, target):\n    return []".to_string(),
        ..RuntimeState::default()
    };

    // Derived, not typed out: this test froze the threshold at "16 seconds" and
    // failed the day the threshold moved, which is the one change it has
    // nothing to say about.
    let past_silence = Duration::from_secs_f64(crate::agent::SILENCE_THRESHOLD_S + 1.0);
    activity.last_code_change = now - past_silence;
    activity.last_user_speech = now - past_silence;
    activity.last_agent_speech = now - past_silence;
    activity.last_nudge = now - Duration::from_secs_f64(crate::agent::SILENCE_COOLDOWN_S + 1.0);

    let prompt = activity.watch_prompt(&state, now).unwrap();

    assert!(prompt.contains("silent AND has not typed"));
    assert!(activity.watch_prompt(&state, now).is_none());

    activity.last_code_change = now - CODE_SETTLE - Duration::from_secs(1);
    activity.last_user_speech = now - Duration::from_secs(5);
    activity.last_agent_speech = now - Duration::from_secs(5);
    activity.last_review = now - Duration::from_secs(31);
    activity.last_interjection = now - Duration::from_secs(46);
    state
        .code
        .push_str("\nseen = {}\nfor i, n in enumerate(nums):\n    pass");

    let prompt = activity.watch_prompt(&state, now).unwrap();

    assert!(prompt.contains("Periodic editor snapshot"));
    assert_eq!(activity.code_at_last_review, state.code);
    assert!(activity.watch_prompt(&state, now).is_none());
}

/// The point of CODE_SETTLE. Without it the whole gate can be reverted to a
/// bare `false` and every other test still passes: the periodic review is
/// the only caller that can fire while the candidate is mid-edit.
#[test]
fn recent_typing_holds_off_the_periodic_review() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    let mut state = RuntimeState {
        code: "def two_sum(nums, target):\n    return []".to_string(),
        ..RuntimeState::default()
    };
    // Everything a proactive review needs is satisfied except the settle.
    activity.last_user_speech = now - Duration::from_secs(5);
    activity.last_agent_speech = now - Duration::from_secs(5);
    activity.last_review = now - Duration::from_secs(31);
    activity.last_interjection = now - Duration::from_secs(46);
    state
        .code
        .push_str("\nseen = {}\nfor i, n in enumerate(nums):\n    pass");

    activity.last_code_change = now - CODE_SETTLE + Duration::from_secs(1);
    assert!(
        activity.watch_prompt(&state, now).is_none(),
        "a candidate who typed a second ago is still working; the review must wait"
    );

    // Exactly CODE_SETTLE has settled: the window is half-open. Without this
    // the boundary is only bracketed, and `<` reads the same as `<=`.
    activity.last_code_change = now - CODE_SETTLE;
    let prompt = activity
        .watch_prompt(&state, now)
        .expect("an edit exactly CODE_SETTLE old has settled; the review may take the floor");
    assert!(prompt.contains("Periodic editor snapshot"));
}

#[test]
fn wrap_up_wait_finishes_after_turn_and_playout_complete() {
    let now = Instant::now();
    let (mut output_audio, _) = test_output_audio(Vec::new());
    let mut activity = RuntimeActivity::new(now);
    let drained = now - Duration::from_secs(1);
    let playing = now + Duration::from_secs(1);

    // Idle and nothing queued: the goodbye is already over.
    assert!(wrap_up_settled(&output_audio, &activity));

    // Mid-turn always waits, queued audio or not.
    activity.floor = Floor::Speaking;
    output_audio.playout_deadline = drained;
    assert!(!wrap_up_settled(&output_audio, &activity));
    output_audio.playout_deadline = playing;
    assert!(!wrap_up_settled(&output_audio, &activity));

    // Turn produced, audio still draining: wait for the buffer.
    activity.floor = Floor::AwaitingPlayout;
    assert!(!wrap_up_settled(&output_audio, &activity));

    output_audio.playout_deadline = drained;
    assert!(wrap_up_settled(&output_audio, &activity));

    // Barge-in hands the floor back with audio cleared.
    activity.floor = Floor::Listening;
    assert!(wrap_up_settled(&output_audio, &activity));
}

#[test]
fn floor_transitions_track_who_is_talking() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);

    assert_eq!(activity.floor, Floor::Listening);

    activity.mark_speaking();
    assert_eq!(activity.floor, Floor::Speaking);

    activity.floor = Floor::AwaitingPlayout;
    activity.mark_listening();
    assert_eq!(activity.floor, Floor::Listening);
    assert!(activity.last_agent_speech >= now);
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
/// leaves the candidate without the ending, and `wrap_up_settled` reads the
/// emptied queue as the turn being over, so the interview ended there.
#[test]
fn the_closing_message_is_not_cut_short_by_a_candidate_talking_over_it() {
    let (mut output_audio, _frames) = test_output_audio(Vec::new());
    let closing = output_audio.output_cancellation.clone();
    output_audio.playout_deadline = Instant::now() + Duration::from_secs(10);
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.floor = Floor::AwaitingPlayout;

    assert!(take_stale_playout(&mut activity, &mut output_audio, Interruptible::No).is_none());

    assert!(!closing.is_cancelled(), "the ending has to play out");
    assert!(output_audio.is_playing());
    assert_eq!(activity.floor, Floor::AwaitingPlayout);
}

/// Dropping ahead of a chunk `capture` will refuse leaves the interviewer
/// cut off mid-sentence with nothing behind it, which is worse than the
/// queueing this whole mechanism exists to avoid.
#[test]
fn only_a_chunk_that_will_be_queued_is_worth_dropping_a_turn_for() {
    let (output_audio, _frames) = test_output_audio(Vec::new());
    let rate = GEMINI_OUTPUT_AUDIO_SAMPLE_RATE;

    assert!(output_audio.accepts(&[1, 0], &format!("audio/pcm;rate={rate}")));
    assert!(
        !output_audio.accepts(&[], &format!("audio/pcm;rate={rate}")),
        "an empty chunk queues nothing"
    );
    assert!(
        !output_audio.accepts(&[1, 0], "audio/pcm;rate=8000"),
        "a rate this source cannot play queues nothing"
    );
    assert!(
        !output_audio.accepts(&[1, 0], "audio/webm"),
        "an unreadable mime type queues nothing"
    );
}

/// The reply-latency line used to measure from `last_user_speech`, which is
/// seeded at the interview start and only ever moved by an input
/// transcript. A reply that followed an `Interrupted` therefore reported
/// the age of the interview: one run logged 15.30s for a candidate who had
/// waited under a second. The stamp is now absent until Gemini says what
/// the candidate said, and absent means print nothing.
/// The three rules that decide whether a reply-latency line is a
/// measurement or a fabrication, exercised where they live rather than
/// through a Room. Every one of them survived mutation before this test
/// existed, which is how the log came to report 15.30s for a candidate who
/// had waited under a second.
#[test]
fn the_reply_latency_stamp_is_armed_and_cleared_by_the_floor() {
    let start = Instant::now() - Duration::from_secs(15);
    let mut activity = RuntimeActivity::new(start);
    assert_eq!(
        activity.awaiting_reply_since, None,
        "nothing waited for yet"
    );

    // Armed when the candidate finishes and the agent is not talking.
    activity.floor = Floor::Listening;
    activity.note_candidate_finished(Instant::now());
    let armed = activity.awaiting_reply_since.expect("a wait to measure");
    assert!(
        armed.elapsed() < Duration::from_secs(1),
        "measured from now, not from the seed"
    );

    // Not re-armed by a late fragment that lands mid-reply: that would restart
    // the clock on a turn the candidate is already hearing.
    activity.floor = Floor::Speaking;
    activity.note_candidate_finished(Instant::now() + Duration::from_secs(5));
    assert_eq!(
        activity.awaiting_reply_since,
        Some(armed),
        "a mid-turn fragment must not re-arm"
    );

    // The idle timers still move, because that is a different question.
    assert!(
        activity.last_user_speech > armed,
        "last_user_speech tracks every fragment"
    );

    // A turn that gets cut takes the pending measurement with it: the candidate
    // is talking again, so nobody is waiting.
    let (mut output_audio, _frames) = test_output_audio(Vec::new());
    output_audio.playout_deadline = Instant::now() + Duration::from_secs(3);
    cut_off_turn(&mut activity, &mut output_audio);
    assert_eq!(
        activity.awaiting_reply_since, None,
        "a cut turn leaves nothing to measure"
    );
}

#[test]
fn a_reply_nobody_was_measured_waiting_for_reports_no_latency() {
    let start = Instant::now() - Duration::from_secs(15);
    let mut activity = RuntimeActivity::new(start);

    // Nothing heard from the candidate yet, which is the state an interruption
    // leaves behind.
    assert_eq!(activity.awaiting_reply_since, None);
    assert!(
        activity.last_user_speech <= start,
        "the seed is still the interview start, and is still what the nudge reads"
    );

    // An input transcript is the only thing that starts the clock.
    activity.awaiting_reply_since = Some(Instant::now());
    let waited = activity.awaiting_reply_since.expect("stamped");
    assert!(
        waited.elapsed() < Duration::from_secs(1),
        "measured from the candidate finishing, not from the interview starting"
    );
}

/// `Interrupted` used to assign the floor bare. `last_agent_speech` is
/// parked at the playout deadline while audio is queued, so leaving it
/// there after discarding the queue suppressed the silence nudge for the
/// length of speech nobody heard.
#[test]
fn a_cut_off_turn_stamps_the_moment_it_was_cut_not_when_it_would_have_ended() {
    let (mut output_audio, _frames) = test_output_audio(Vec::new());
    let cut = output_audio.output_cancellation.clone();
    output_audio.playout_deadline = Instant::now() + Duration::from_secs(15);
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.last_agent_speech = output_audio.playout_deadline;
    activity.floor = Floor::Speaking;

    let unplayed = cut_off_turn(&mut activity, &mut output_audio);

    assert!(unplayed >= Duration::from_secs(14), "reports {unplayed:?}");
    assert!(cut.is_cancelled(), "queued frames must be dropped");
    assert_eq!(activity.floor, Floor::Listening);
    assert!(
        activity.last_agent_speech <= Instant::now(),
        "the stamp must not sit in the future, where it suppresses the nudge"
    );
}

/// The first candidate answer used to land behind the rest of the greeting.
/// Gemini streams a twenty second greeting in about two, marks the turn
/// complete, and then never reports an interruption, because from its side
/// that turn is long over. Only this process knows the queue is still
/// draining.
#[test]
fn a_candidate_speaking_over_a_draining_turn_drops_what_is_left_of_it() {
    let (mut output_audio, _frames) = test_output_audio(Vec::new());
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
        let (mut output_audio, _frames) = test_output_audio(Vec::new());
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

/// The ceiling is a boundary, so both sides of it are named. One attempt
/// short
/// of the limit still restarts; the limit itself does not, and neither does
/// the
/// count past it that a wrong comparison would let through.
///
/// Every case here uses a socket that died young, because a socket that
/// lived
/// clears the counter and there would be no ceiling left to test.
#[test]
fn the_restart_ceiling_admits_the_last_attempt_and_refuses_the_one_after() {
    let young = Duration::from_secs(1);

    let mut last = GEMINI_RESTART_LIMIT - 1;
    assert!(
        take_restart_attempt(&mut last, young),
        "the attempt below the ceiling is the one a flaky endpoint gets"
    );
    assert_eq!(last, GEMINI_RESTART_LIMIT);

    let mut at_limit = GEMINI_RESTART_LIMIT;
    assert!(!take_restart_attempt(&mut at_limit, young));
    assert_eq!(
        at_limit, GEMINI_RESTART_LIMIT,
        "a refused attempt is not a spent one"
    );
}

/// The bug this exists for: Gemini's own connection cap closes a healthy
/// socket
/// every ten minutes or so, and counting those against the ceiling turned a
/// budget for a failing endpoint into a limit on how long an interview
/// could
/// run. A socket that carried a working conversation costs nothing.
#[test]
fn a_socket_that_lived_clears_what_earlier_failures_spent() {
    let mut restarts = GEMINI_RESTART_LIMIT;

    assert!(take_restart_attempt(&mut restarts, HEALTHY_GEMINI_SOCKET));

    assert_eq!(
        restarts, 1,
        "the healthy socket resets the run, and this attempt is the first of the next one"
    );
}

/// The boundary of that reset, from the other side. One second short of
/// healthy
/// is still a failing socket, and it must not refill the budget.
#[test]
fn a_socket_that_died_just_short_of_healthy_still_spends() {
    let mut restarts = 3;

    assert!(take_restart_attempt(
        &mut restarts,
        HEALTHY_GEMINI_SOCKET - Duration::from_secs(1)
    ));

    assert_eq!(restarts, 4);
}

/// Exactly one attempt per restart. Started away from zero on purpose: at
/// zero
/// a counter that multiplied instead of adding would look identical to one
/// that
/// added.
#[test]
fn each_restart_spends_one_attempt() {
    let young = Duration::from_secs(1);
    let mut restarts = 3;

    assert!(take_restart_attempt(&mut restarts, young));
    assert_eq!(restarts, 4);

    assert!(take_restart_attempt(&mut restarts, young));
    assert_eq!(restarts, 5);
}

#[test]
fn observer_is_not_the_candidate() {
    assert!(candidate_identity_matches(
        "candidate-a1b2c3",
        r#"{"candidateIdentity":"candidate-a1b2c3"}"#,
    ));
    assert!(!candidate_identity_matches("observer-a1b2c3", "{}"));
    assert!(!candidate_identity_matches(
        "candidate-a1b2c3",
        r#"{"candidateIdentity":"candidate-other"}"#,
    ));
}
