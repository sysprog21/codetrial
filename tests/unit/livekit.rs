//! The `tests` module of `src/livekit.rs`, which declares this file by path.
//! Everything here reaches into `src/livekit.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::session::accept_gemini_event;
use super::*;
use crate::agent::ThinkingHold;
use ::livekit::webrtc::audio_source::AudioSourceOptions;
use ::livekit::webrtc::audio_source::native::NativeAudioSource;
use tokio::sync::mpsc;

use crate::config::load_from_pairs;

// Only the tests below reach for it now: the report packet that carries this
// topic is built in `report.rs`, and what is left here asserts which topics the
// loop refuses from a browser.
use crate::runtime::TOPIC_REPORT;

/// An `OutputAudio` wired to a channel instead of a room.
///
/// `pub(super)` and defined here rather than in each of the two test modules
/// that build one: `media`'s tests are a descendant of `livekit`, so they can
/// name it, and a second copy is a second thing to keep in step with the
/// fields.
///
/// The rate is the constant rather than the 24000 it expands to, because
/// `accepts` compares it against the rate in the mime type a caller hands
/// `capture`. A fixture pinned to the literal on both sides would keep passing
/// while testing a rate production had moved off.
pub(super) fn test_output_audio() -> (OutputAudio, mpsc::Receiver<QueuedOutputFrame>) {
    let (frames, queued_frames) = mpsc::channel(4);
    (
        OutputAudio::new(
            NativeAudioSource::new(
                AudioSourceOptions::default(),
                GEMINI_OUTPUT_AUDIO_SAMPLE_RATE,
                LIVEKIT_OUTPUT_CHANNELS,
                LIVEKIT_OUTPUT_QUEUE_MS,
            ),
            GEMINI_OUTPUT_AUDIO_SAMPLE_RATE,
            frames,
        ),
        queued_frames,
    )
}

/// A run that executed one case against the code in the editor now. The
/// integration tests keep the same helper in tests/agent.rs, which this crate
/// cannot reach; change the two together.
pub(crate) fn receive_test_run(state: &mut RuntimeState) {
    let run = serde_json::json!({"passed": 1, "total": 1,
        "code": state.code, "language": state.language});
    crate::agent::apply_data_event(state, crate::runtime::TOPIC_TEST_RESULTS, &run, 99.0);
}

/// The source an observation of `phase` is recorded from: Test is the run's.
pub(crate) fn observed_source(phase: &str) -> &'static str {
    if phase == "test" {
        "test_event"
    } else {
        "candidate_speech"
    }
}

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
            interview_mode: crate::agent::InterviewMode::Whiteboard,
            code_execution_disabled: true,
            ..crate::runtime::RuntimeOptions::default()
        },
    );
    let untouched = RuntimeState::default();
    for (named, same) in [
        (
            "code_execution_disabled",
            boot.code_execution_disabled == untouched.code_execution_disabled,
        ),
        (
            "interview_loop",
            boot.interview_loop == untouched.interview_loop,
        ),
        (
            "interview_mode",
            boot.interview_mode == untouched.interview_mode,
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
    assert_eq!(state.interview_mode, boot.interview_mode);
    assert_eq!(state.code_execution_disabled, boot.code_execution_disabled);
    assert_eq!(state.coding_minutes, boot.coding_minutes);
    assert_eq!(state.behavioral_minutes, boot.behavioral_minutes);
    assert_eq!(state.hint_ladder, boot.problem.variant().hints);
    assert_eq!(
        state.code_templates.len(),
        boot.problem.variant().starters.len(),
        "every language's starter is seeded from the problem"
    );

    // A coding-only interview has no behavioral round, so seeding its phases as
    // uncovered would ask the report to account for one that was never
    // configured.
    assert_eq!(
        state.evidence_ledger.coverage.uncovered.len(),
        crate::agent::REACTO_PHASE_IDS.len()
    );
    assert!(
        !state
            .evidence_ledger
            .coverage
            .uncovered
            .iter()
            .any(|phase| phase == "situation")
    );
}

/// The other loop, which is the one that owes STAR phases as well.
#[test]
fn a_two_round_interview_leaves_both_frameworks_uncovered() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google-key"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap_with_rounds(
        &config,
        "interview-fixed",
        Some("two-sum"),
        45,
        crate::runtime::RuntimeOptions {
            interview_loop: crate::agent::InterviewLoop::CodingBehavioral,
            ..crate::runtime::RuntimeOptions::default()
        },
    );
    let state = initial_runtime_state(&boot, Instant::now());
    assert_eq!(
        state.evidence_ledger.coverage.uncovered.len(),
        crate::agent::REACTO_PHASE_IDS.len() + crate::agent::STAR_PHASE_IDS.len()
    );
    assert!(
        state
            .evidence_ledger
            .coverage
            .uncovered
            .iter()
            .any(|phase| phase == "situation")
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

/// Every Live socket that opens is counted, from both places one opens.
///
/// The counting itself is tested against the ledger, and the cost line is
/// tested against its format, but neither notices if a call site stops
/// calling: deleting both recordings leaves the whole suite green, because a
/// room that really connected is the only other witness. Read as source for
/// the reason the interview clock is, one test below.
#[test]
fn both_ways_a_live_socket_opens_count_its_instruction() {
    let source = include_str!("../../src/livekit.rs");
    for opener in ["async fn open_session", "async fn replace_gemini_session"] {
        let body = source
            .split(opener)
            .nth(1)
            .unwrap_or_else(|| panic!("{opener} is still defined here"));

        // To the next item at column zero, so a later function's recording
        // cannot stand in for this one's.
        let body = body.split("\n}\n").next().unwrap_or_default();
        assert!(
            body.contains("ModelInputKind::LiveSetup"),
            "{opener} must count the instruction its socket was opened with"
        );
    }
}

/// Only a replacement tells the page why it waits: at startup the page is in
/// its own connecting state, and a reconnect notice would contradict it. The
/// retry loop is tested with its notifier passed in, so only the call sites
/// decide this, and they are distinguishable only in a room that really
/// connected.
#[test]
fn only_a_replacement_publishes_its_reconnect_wait() {
    let source = include_str!("../../src/livekit.rs");
    for (opener, room) in [
        ("async fn open_session", "None"),
        ("async fn replace_gemini_session", "Some(room)"),
    ] {
        let body = source
            .split(opener)
            .nth(1)
            .unwrap_or_else(|| panic!("{opener} is still defined here"));
        let body = body.split("\n}\n").next().unwrap_or_default();
        let call = body
            .lines()
            .find(|line| line.contains("open_cold_session("))
            .unwrap_or_else(|| panic!("{opener} still opens a cold session"));
        assert!(
            call.contains(&format!(", {room})")),
            "{opener} must pass {room} to open_cold_session: {call}"
        );
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

/// The planned minutes plus the grace. Both terms matter: this clock is what
/// stops a metered Gemini session when a candidate closes the tab, so it is
/// billed for rather than merely wrong.
#[test]
fn the_server_side_deadline_is_the_planned_minutes_plus_the_grace() {
    assert_eq!(
        interview_hard_deadline(45),
        Duration::from_secs(45 * 60) + INTERVIEW_DEADLINE_GRACE
    );

    // The longest interview the config admits still fits, which is the only
    // reason the seconds are computed in `u64`.
    assert_eq!(
        interview_hard_deadline(90),
        Duration::from_secs(90 * 60) + INTERVIEW_DEADLINE_GRACE
    );
}

/// The boundary both the goodbye and a `GoAway` wait for. The floor alone is
/// not enough: it is stamped once when a turn completes, and the LiveKit
/// playout queue drains on its own afterwards.
#[test]
fn output_is_settled_only_once_nothing_is_generating_or_queued() {
    assert!(!output_settled(Floor::Speaking, false));
    assert!(!output_settled(Floor::AwaitingPlayout, true));
    assert!(output_settled(Floor::Listening, false));
    assert!(output_settled(Floor::AwaitingPlayout, false));
}

/// `Floor::Speaking` is stamped by the first audio chunk, so a turn that has
/// issued a tool call and produced nothing yet still reads as settled.
/// Replacing the socket there sends the tool response out on a socket the reply
/// will never come back on, which is why `request` weighs one signal
/// `output_settled` cannot carry.
#[test]
fn a_go_away_waits_for_a_reply_that_has_not_started() {
    let mut deferred = DeferredRestart::default();

    assert!(output_settled(Floor::Listening, false));
    assert!(!deferred.request(Floor::Listening, false, true, false));
}

/// The test above hands `request` the boolean directly, which pins the rule and
/// not the wiring: an inverted `reply_in_flight` would keep every one of these
/// tests green and reverse the behaviour in the room, replacing the socket
/// during the tool call it exists to protect and holding the advisory through
/// the settled moment it exists to use. Both of its mutants survived until this
/// test read the field the accessor reads.
#[test]
fn a_pending_reply_is_what_the_advisory_reads_as_in_flight() {
    let mut activity = RuntimeActivity::new(Instant::now() - Duration::from_secs(15));
    assert!(
        !activity.reply_in_flight(),
        "nothing has been asked of Gemini yet"
    );

    // The stamp is armed only off the floor, so this is the state a tool call
    // is issued from: settled to `output_settled`, and still owed a reply.
    activity.floor = Floor::Listening;
    activity.note_candidate_finished(Instant::now(), false);
    assert!(
        activity.reply_in_flight(),
        "the candidate is waiting on Gemini"
    );

    let mut deferred = DeferredRestart::default();
    assert!(
        !deferred.request(
            activity.floor,
            false,
            activity.reply_in_flight(),
            activity.tool_response_outstanding,
        ),
        "a settled floor is not enough while a reply is owed"
    );

    // Cleared when the turn is cut, and the same advisory is then due.
    let (mut output_audio, _frames) = test_output_audio();
    cut_off_turn(&mut activity, &mut output_audio);
    assert!(!activity.reply_in_flight(), "a cut turn owes nothing");
    assert!(deferred.take_if_due(
        activity.floor,
        output_audio.is_playing(),
        activity.tool_response_outstanding,
    ));
}

#[test]
fn a_go_away_mid_turn_is_held_until_the_queue_drains() {
    let mut deferred = DeferredRestart::default();

    assert!(!deferred.request(Floor::Speaking, false, false, false));
    assert!(!deferred.take_if_due(Floor::Speaking, false, false));
    assert!(!deferred.take_if_due(Floor::AwaitingPlayout, true, false));
    assert!(deferred.take_if_due(Floor::Listening, false, false));

    // Spent once. A second boundary must not buy another replacement.
    assert!(!deferred.take_if_due(Floor::Listening, false, false));
}

/// The regression this type exists for. A candidate talking over the draining
/// queue empties it on an `InputTranscript`, and Gemini sends no `Interrupted`
/// for a turn it already considers over, so waiting for a turn boundary waits
/// out the whole `timeLeft` and the socket drops cold instead. That same event
/// stamps `awaiting_reply_since`, which is why the held advisory ignores it.
#[test]
fn a_held_go_away_is_spent_when_a_barge_in_drains_the_queue() {
    let mut deferred = DeferredRestart::default();

    assert!(!deferred.request(Floor::AwaitingPlayout, true, false, false));
    assert!(deferred.take_if_due(Floor::Listening, false, false));
}

/// A tool response is generation already paid for on the old socket, and it
/// leaves no trace in the terms `output_settled` reads: the floor is still
/// `Listening` because no audio chunk has stamped it, and nothing is queued.
/// A held advisory spent there closes the socket the answer was coming back
/// on, so the turn is lost and a cold restart is what replaces it.
#[test]
fn a_held_go_away_waits_for_a_tool_response_it_already_paid_for() {
    let mut deferred = DeferredRestart::default();

    // Deferred mid-turn, then the turn issues a tool call and produces no
    // audio: every term below reads settled while the answer is still owed.
    assert!(!deferred.request(Floor::Speaking, true, false, false));
    assert!(output_settled(Floor::Listening, false));
    assert!(!deferred.take_if_due(Floor::Listening, false, true));

    // Still held, so the answer arriving is what spends it.
    assert!(deferred.take_if_due(Floor::Listening, false, false));
}

/// The same debt refuses the advisory on arrival, not only afterwards.
#[test]
fn a_go_away_arriving_on_an_owed_tool_response_is_held() {
    let mut deferred = DeferredRestart::default();

    assert!(!deferred.request(Floor::Listening, false, false, true));
    assert!(deferred.take_if_due(Floor::Listening, false, false));
}

/// The close performs the replacement itself. Left armed, the advisory would
/// spend the first completed turn on the new socket on a second one.
#[test]
fn a_close_before_the_boundary_cancels_the_held_go_away() {
    let mut deferred = DeferredRestart::default();

    assert!(!deferred.request(Floor::Speaking, false, false, false));
    deferred.cancel();
    assert!(!deferred.take_if_due(Floor::Listening, false, false));
}

/// A second advisory on a settled floor restarts now, and must not leave the
/// first one armed behind it.
#[test]
fn an_immediate_go_away_clears_an_advisory_already_held() {
    let mut deferred = DeferredRestart::default();

    assert!(!deferred.request(Floor::Speaking, false, false, false));
    assert!(deferred.request(Floor::Listening, false, false, false));
    assert!(!deferred.take_if_due(Floor::Listening, false, false));
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
            r#"{"problemId":"merge-intervals","durationMin":30,"interviewLoop":"coding_only","codeExecution":false,"interviewProfile":{"role":"Platform engineer","seniority":"staff","targetCompany":"Example Co"}}"#,
        ),
    );

    assert_eq!(boot.problem.id, "merge-intervals");
    assert_eq!(boot.duration_min, 30);
    assert_eq!(boot.interview_loop, crate::agent::InterviewLoop::CodingOnly);
    assert_eq!((boot.coding_minutes, boot.behavioral_minutes), (30, 0));
    assert_eq!(boot.profile.role, "Platform engineer");
    assert_eq!(boot.profile.target_company, "Example Co");
    assert!(boot.code_execution_disabled);
    assert!(
        boot.instructions
            .contains("code execution is disabled for this interview")
    );
    assert!(boot.instructions.contains("candidate selected staff"));
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
    quiet_since(&mut activity, now - past_silence);
    activity.last_nudge = now - Duration::from_secs_f64(crate::agent::SILENCE_COOLDOWN_S + 1.0);

    let prompt = activity.watch_prompt(&mut state, now).unwrap();

    assert!(prompt.text.contains("Silent and not typing"));

    // The code reaches the prompt only inside the fenced excerpt, as the change
    // since the last review, which for the first one is the whole buffer.
    let fence = position(&prompt.text, "BEGIN UNTRUSTED EDITOR (");
    assert!(
        position(&prompt.text, "def two_sum") > fence,
        "{}",
        prompt.text
    );
    assert!(!prompt.behavioral_nudge);
    assert!(activity.watch_prompt(&mut state, now).is_none());

    activity.last_code_change = now - CODE_SETTLE - Duration::from_secs(1);
    activity.last_user_speech = now - Duration::from_secs(5);
    activity.last_agent_speech = now - Duration::from_secs(5);
    activity.last_review = now - Duration::from_secs(31);
    activity.last_interjection = now - Duration::from_secs(46);
    state.evidence_ledger.code.substantive_revision = 1;
    state.evidence_ledger.code.parser_observation = Some(crate::agent::CodeObservation::Parsed);

    state.behavioral_round_started = true;
    assert!(activity.watch_prompt(&mut state, now).is_none());
    state.behavioral_round_started = false;
    let prompt = activity.watch_prompt(&mut state, now).unwrap();

    assert!(prompt.text.contains("A code change settled"));

    // The nudge already showed this buffer, so the review says it is unchanged
    // rather than sending it again or sending the model to read it; only the
    // review consumes the change that armed it.
    assert!(
        !prompt.text.contains("BEGIN UNTRUSTED EDITOR"),
        "{}",
        prompt.text
    );
    assert!(
        prompt
            .text
            .contains("The editor is unchanged since you last saw it.")
    );
    assert!(!prompt.text.contains("read_editor"), "{}", prompt.text);
    assert_eq!(activity.substantive_revision_at_last_review, 1);
    assert_eq!(state.code_shown, state.code);
    assert!(!prompt.behavioral_nudge);
    assert!(activity.watch_prompt(&mut state, now).is_none());

    // No counter assertion here any more, and deliberately: a prompt is counted
    // where it is handed to the socket, so `watch_prompt` hands its caller a
    // string and nothing else. `send_model_text` is what counts it, alongside
    // every other realtime-input text this server sends, and
    // `record_model_input` is what tests the counting.
}

/// Every timestamp the watcher reads as activity, set to one instant.
fn quiet_since(activity: &mut RuntimeActivity, at: Instant) {
    activity.last_code_change = at;
    activity.last_user_speech = at;
    activity.last_agent_speech = at;
}

#[test]
fn behavioral_silence_nudge_waits_for_quiet_and_leaves_the_editor_out() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    let mut state = RuntimeState {
        behavioral_round_started: true,
        code: "editor sentinel".to_string(),
        ..RuntimeState::default()
    };
    let threshold = Duration::from_secs_f64(crate::agent::SILENCE_THRESHOLD_S);
    let cooldown = Duration::from_secs_f64(crate::agent::SILENCE_COOLDOWN_S);
    quiet_since(&mut activity, now - threshold);
    activity.last_nudge = now - cooldown;

    state.paused = true;
    assert!(activity.watch_prompt(&mut state, now).is_none());
    state.paused = false;
    for floor in [Floor::Speaking, Floor::AwaitingPlayout] {
        activity.floor = floor;
        assert!(activity.watch_prompt(&mut state, now).is_none());
    }
    activity.floor = Floor::Listening;

    // The candidate, the interviewer, and the editor each keep the room busy.
    let just_inside = now - threshold + Duration::from_millis(1);
    let recent: [fn(&mut RuntimeActivity, Instant); 3] = [
        |activity, at| activity.last_user_speech = at,
        |activity, at| activity.last_agent_speech = at,
        |activity, at| activity.last_code_change = at,
    ];
    for touch in recent {
        touch(&mut activity, just_inside);
        assert!(activity.watch_prompt(&mut state, now).is_none());
        quiet_since(&mut activity, now - threshold);
    }

    let last_review = activity.last_review;
    let prompt = activity
        .watch_prompt(&mut state, now)
        .expect("quiet behavioral answer gets a nudge");
    assert_eq!(
        prompt.text,
        crate::agent::with_timer(&state, crate::agent::behavioral_silence_nudge())
    );
    assert!(prompt.behavioral_nudge);
    assert!(!prompt.text.contains("editor sentinel"));
    assert_eq!(activity.last_nudge, now);
    assert_eq!(activity.last_interjection, now);
    assert_eq!(activity.last_review, last_review);
    assert_eq!(activity.substantive_revision_at_last_review, 0);
    assert!(
        activity
            .watch_prompt(&mut state, now + cooldown - Duration::from_millis(1))
            .is_none()
    );
}

#[tokio::test]
async fn only_a_delivered_behavioral_nudge_spends_the_round() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    let mut state = RuntimeState {
        behavioral_round_started: true,
        ..RuntimeState::default()
    };
    let cooldown = Duration::from_secs_f64(crate::agent::SILENCE_COOLDOWN_S);
    quiet_since(
        &mut activity,
        now - Duration::from_secs_f64(crate::agent::SILENCE_THRESHOLD_S),
    );
    activity.last_nudge = now - cooldown;

    let coding = WatchPrompt {
        text: String::new(),
        behavioral_nudge: false,
        allows_silence: false,
    };
    send_watched_prompt(&mut activity, &coding, async {
        Ok::<(), std::io::Error>(())
    })
    .await
    .unwrap();
    assert!(!activity.behavioral_nudged);
    assert_eq!(activity.floor, Floor::Speaking);
    assert!(
        activity.owes_reply(),
        "a nudge is a question the candidate is owed an answer to"
    );

    // An editor review may rightly get nothing back, so a socket replaced
    // before it answers owes nothing for it, though it still holds the floor.
    let review = WatchPrompt {
        text: String::new(),
        behavioral_nudge: false,
        allows_silence: true,
    };
    activity.floor = Floor::Listening;
    activity.prompted_at = None;
    send_watched_prompt(&mut activity, &review, async {
        Ok::<(), std::io::Error>(())
    })
    .await
    .unwrap();
    assert_eq!(activity.floor, Floor::Speaking);
    assert!(activity.prompted_at.is_some());
    assert!(!activity.owes_reply());
    activity.floor = Floor::Listening;

    let prompt = activity.watch_prompt(&mut state, now).unwrap();
    let failed = send_watched_prompt(&mut activity, &prompt, async {
        Err::<(), _>(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "closed socket",
        ))
    })
    .await;
    assert_eq!(failed.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
    assert!(!activity.behavioral_nudged);
    assert_eq!(activity.floor, Floor::Listening);

    clear_abandoned_socket_work(&mut state, &mut activity);
    let prompt = activity
        .watch_prompt(&mut state, now + cooldown)
        .expect("a nudge that never reached Gemini is offered again");
    send_watched_prompt(&mut activity, &prompt, async {
        Ok::<(), std::io::Error>(())
    })
    .await
    .unwrap();
    assert!(activity.behavioral_nudged);
    assert_eq!(activity.floor, Floor::Speaking);
    activity.floor = Floor::Listening;

    // One delivered nudge per round, including after a connection replacement.
    clear_abandoned_socket_work(&mut state, &mut activity);
    assert!(
        activity
            .watch_prompt(&mut state, now + cooldown * 2)
            .is_none()
    );
}

/// Every gate a periodic review waits on, opened as of `now`.
fn ready_for_review(activity: &mut RuntimeActivity, now: Instant) {
    activity.last_code_change = now - CODE_SETTLE - Duration::from_secs(1);
    activity.last_user_speech = now - Duration::from_secs(5);
    activity.last_agent_speech = now - Duration::from_secs(5);
    activity.last_review = now - Duration::from_secs(31);
    activity.last_interjection = now - Duration::from_secs(46);
}

/// An editor packet, received at `receipt`.
fn edit(state: &mut RuntimeState, receipt: u64, code: &str) {
    crate::agent::apply_data_event_at(
        state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": code }),
        99.0,
        receipt,
    );
}

/// A test run the browser reported, received at `receipt`.
fn run_tests(state: &mut RuntimeState, receipt: u64, passed: u32, total: u32) {
    crate::agent::apply_data_event_at(
        state,
        crate::runtime::TOPIC_TEST_RESULTS,
        &serde_json::json!({
            "passed": passed, "total": total, "language": "python", "cases": [],
            "setupError": null,
        }),
        99.0,
        receipt,
    );
}

/// Where `needle` sits in a prompt. A layout change is what these checks exist
/// to catch, so a missing marker fails with the prompt it is missing from.
fn position(prompt: &str, needle: &str) -> usize {
    prompt
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in the prompt:\n{prompt}"))
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
    state.evidence_ledger.code.substantive_revision = 1;
    state.evidence_ledger.code.parser_observation = Some(crate::agent::CodeObservation::Parsed);

    activity.last_code_change = now - CODE_SETTLE + Duration::from_secs(1);
    assert!(
        activity.watch_prompt(&mut state, now).is_none(),
        "a candidate who typed a second ago is still working; the review must wait"
    );

    // Exactly CODE_SETTLE has settled: the window is half-open. Without this
    // the boundary is only bracketed, and `<` reads the same as `<=`.
    activity.last_code_change = now - CODE_SETTLE;
    let prompt = activity
        .watch_prompt(&mut state, now)
        .expect("an edit exactly CODE_SETTLE old has settled; the review may take the floor");
    assert!(prompt.text.contains("A code change settled"));
}

#[test]
fn wrap_up_wait_finishes_after_turn_and_playout_complete() {
    let now = Instant::now();
    let (mut output_audio, _) = test_output_audio();
    let mut activity = RuntimeActivity::new(now);
    let drained = now - Duration::from_secs(1);
    let playing = now + Duration::from_secs(1);

    // Idle and nothing queued: the goodbye is already over.
    assert!(output_settled(activity.floor, output_audio.is_playing()));

    // Mid-turn always waits, queued audio or not.
    activity.floor = Floor::Speaking;
    output_audio.playout_deadline = drained;
    assert!(!output_settled(activity.floor, output_audio.is_playing()));
    output_audio.playout_deadline = playing;
    assert!(!output_settled(activity.floor, output_audio.is_playing()));

    // Turn produced, audio still draining: wait for the buffer.
    activity.floor = Floor::AwaitingPlayout;
    assert!(!output_settled(activity.floor, output_audio.is_playing()));

    output_audio.playout_deadline = drained;
    assert!(output_settled(activity.floor, output_audio.is_playing()));

    // Barge-in hands the floor back with audio cleared.
    activity.floor = Floor::Listening;
    assert!(output_settled(activity.floor, output_audio.is_playing()));
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

/// Dropping ahead of a chunk `capture` will refuse leaves the interviewer
/// cut off mid-sentence with nothing behind it, which is worse than the
/// queueing this whole mechanism exists to avoid.
#[test]
fn only_a_chunk_that_will_be_queued_is_worth_dropping_a_turn_for() {
    let (output_audio, _frames) = test_output_audio();
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
    activity.note_candidate_finished(Instant::now(), false);
    let armed = activity.awaiting_reply_since.expect("a wait to measure");
    assert!(
        armed.elapsed() < Duration::from_secs(1),
        "measured from now, not from the seed"
    );

    // Not re-armed by a late fragment that lands mid-reply: that would restart
    // the clock on a turn the candidate is already hearing. The reply is under
    // way once it has produced output, which is what the dispatcher notes for
    // every audio chunk before queueing it.
    activity.note_output(Instant::now());
    activity.floor = Floor::Speaking;
    activity.note_candidate_finished(Instant::now() + Duration::from_secs(5), false);
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
    let (mut output_audio, _frames) = test_output_audio();
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

/// The two turns an interruption must not cut: one nobody can be answering
/// yet, and one the interview is over after.
///
/// An interviewer line is not the candidate, which is the half a prefix check
/// can get wrong in the direction that matters: it would read Jim's own
/// greeting as evidence that somebody is there to interrupt it.
#[test]
fn a_greeting_and_a_goodbye_survive_an_interruption() {
    let (mut output_audio, _frames) = test_output_audio();
    let kept = output_audio.output_cancellation.clone();
    output_audio.playout_deadline = Instant::now() + Duration::from_secs(10);
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.floor = Floor::Speaking;
    let greeting_only = ["Jim: hey there, I am Jim".to_string()];

    assert!(
        session::cut_unless_protected(
            &greeting_only,
            Interruptible::Yes,
            &mut activity,
            &mut output_audio,
        )
        .is_none(),
        "a turn nobody has answered yet is kept"
    );
    assert!(!kept.is_cancelled(), "the queued frames must still play");
    assert!(output_audio.is_playing());

    let answered = [
        greeting_only[0].clone(),
        format!(
            "{}: sure, so the input is an array",
            crate::agent::CANDIDATE_SPEAKER
        ),
    ];
    let unplayed = session::cut_unless_protected(
        &answered,
        Interruptible::Yes,
        &mut activity,
        &mut output_audio,
    )
    .expect("a candidate who has been heard can barge in");

    // The closing message is the turn that plays to the end. Gemini reports its
    // own interruption through a different door from the transcript that
    // `take_stale_playout` guards, and that door used to have no lock.
    let (mut closing_audio, _closing_frames) = test_output_audio();
    let closing = closing_audio.output_cancellation.clone();
    closing_audio.playout_deadline = Instant::now() + Duration::from_secs(10);
    assert!(
        session::cut_unless_protected(
            &answered,
            Interruptible::No,
            &mut activity,
            &mut closing_audio,
        )
        .is_none(),
        "the goodbye is not cut by a throat cleared over it"
    );
    assert!(
        !closing.is_cancelled(),
        "the closing frames must still play"
    );

    assert!(unplayed >= Duration::from_secs(9), "reports {unplayed:?}");
    assert!(kept.is_cancelled(), "queued frames must be dropped");
    assert_eq!(activity.floor, Floor::Listening);
}

/// `Interrupted` used to assign the floor bare. `last_agent_speech` is
/// parked at the playout deadline while audio is queued, so leaving it
/// there after discarding the queue suppressed the silence nudge for the
/// length of speech nobody heard.
#[test]
fn a_cut_off_turn_stamps_the_moment_it_was_cut_not_when_it_would_have_ended() {
    let (mut output_audio, _frames) = test_output_audio();
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

/// The browser reveals "leave the room" on its own timer, and that timer has to
/// clear the deadline the agent is working to. They are two constants in two
/// languages that only ever agreed because whoever moved one remembered to move
/// the other, which is how `REPORT_TIMEOUT` went from 45 to 125 seconds in this
/// tree: by hand, in three files, with nothing to catch the file that got
/// missed. A candidate offered the escape hatch before their report lands takes
/// it, and leaving never saves the report.
///
/// The page owns the number and this reads it, in the shape
/// `the_integrity_detail_bound_is_the_same_number_on_both_sides` established:
/// a test that restated the literal would be the third copy of the thing it is
/// here to prevent.
#[test]
fn the_browser_escape_hatch_outlasts_the_report_deadline() {
    let page = std::fs::read_to_string("web/interview.js").expect("the page is readable");
    let declaration = "const REPORT_ESCAPE_WAIT_MS = ";
    let start = page
        .find(declaration)
        .expect("web/interview.js declares REPORT_ESCAPE_WAIT_MS")
        + declaration.len();
    let rest = &page[start..];
    let end = rest.find(';').expect("the declaration ends in a semicolon");
    let wait = Duration::from_millis(
        rest[..end]
            .trim()
            .parse()
            .expect("REPORT_ESCAPE_WAIT_MS is a number"),
    );

    // The Gemini close sits between the frozen report and its first publish.
    let close = crate::gemini::CLOSE_TIMEOUT;
    let delivery = report::DELIVERY_WAIT * report::DELIVERY_ATTEMPTS as u32;
    assert!(
        wait > REPORT_TIMEOUT + WRAP_UP_WAIT + close + delivery,
        "a report bounded at {REPORT_TIMEOUT:?} after a {WRAP_UP_WAIT:?} wrap-up and a \
         {close:?} Gemini close cannot land with up to {delivery:?} delivery time before \
         the page offers to leave at {wait:?}"
    );
}

/// A publish only queues the packet, so leaving right behind it can drop the
/// report. The ending leaves once delivery settles, with no fixed sleep
/// standing in for it, and a report goes out only through the receipt wait.
#[test]
fn the_ending_leaves_only_after_the_report_delivery_settles() {
    let source = include_str!("../../src/livekit.rs");
    // To the next item at column zero, as the source tests above read a body.
    let ending = source
        .split("async fn apply_data_packet(")
        .nth(1)
        .expect("apply_data_packet is still defined here")
        .split("\n}\n")
        .next()
        .unwrap_or_default();
    let publish = ending
        .find("publish_with_recovery(")
        .expect("the ending publishes through recovery");
    let leave = ending.find("leave_room(").expect("the ending leaves");
    assert!(publish < leave);
    assert!(!ending[publish..leave].contains("sleep("));

    let report = include_str!("../../src/livekit/report.rs");
    let live = report
        .split("impl RecoveryRoom for LiveRecoveryRoom")
        .nth(1)
        .expect("the live recovery room is still defined here")
        .split("\n}\n")
        .next()
        .unwrap_or_default();
    let publish = live
        .split("async fn publish(")
        .nth(1)
        .expect("the live room publishes reports");
    assert!(publish.contains("deliver_report("));
}

/// Each pause reads the stretch since the last one, and never that stretch
/// twice.
///
/// The cursor is the whole of what makes this cheap. Left unmoved, every pause
/// re-reads the interview from the beginning, which is the cost this exists to
/// remove, and the notes would pile up restating the opening minutes.
/// A pause note is read by the final reviewer, so a turn the recognizer
/// returned in another script is left out of the window the note is written
/// from, the same way the report leaves it out. Escaped: the script is the
/// point, not the words.
#[test]
fn a_pause_review_does_not_read_an_unrecognized_turn() {
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut state = RuntimeState {
        transcript: vec![
            "Candidate: \u{662f}\u{554a}\u{3002}\u{8863}\u{670d}\u{7684}\u{5c3a}\u{5bf8}"
                .to_string(),
            "Candidate: I check the map before inserting".to_string(),
        ],
        ..RuntimeState::default()
    };
    let window = take_interim_review_window(&mut state, &boot);
    assert!(window.contains(crate::agent::UNRECOGNIZED_TURN));
    assert!(!window.contains('\u{8863}'));
    assert!(window.contains("I check the map before inserting"));
    assert_eq!(state.transcript[0].chars().nth(11), Some('\u{662f}'));
}

#[test]
fn each_pause_reviews_the_speech_since_the_last_one() {
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut state = RuntimeState {
        transcript: vec![
            "Candidate: first I restate the problem".to_string(),
            "Candidate: then I pick a hash map".to_string(),
        ],
        code: "seen = {}".to_string(),
        ..RuntimeState::default()
    };

    let first = take_interim_review_window(&mut state, &boot);
    assert!(first.contains("first I restate the problem"));
    assert!(first.contains("then I pick a hash map"));
    assert!(first.contains("seen = {}"));
    assert!(first.contains("(nothing recorded yet)"));
    assert_eq!(state.interim_transcript_lines, 2);

    record_interim_notes(&mut state, "- Candidate restated the problem.");
    state
        .transcript
        .push("Candidate: now the duplicate case".to_string());

    let second = take_interim_review_window(&mut state, &boot);
    assert!(second.contains("now the duplicate case"));
    assert!(
        !second.contains("first I restate the problem"),
        "a stretch already reviewed is not paid for a second time"
    );
    assert!(
        second.contains("Candidate restated the problem."),
        "what is already on record is passed along so the next note does not repeat it"
    );
    assert_eq!(state.interim_transcript_lines, 3);

    // The code the first review read has not changed, so the second is told so
    // rather than sent it again.
    assert!(!second.contains("seen = {}"), "{second}");
    assert!(second.contains(INTERIM_CODE_UNCHANGED), "{second}");

    // A transcript shorter than the cursor is not reachable today. It is one
    // future edit away, and the arithmetic that would panic on it is in here.
    state.transcript.clear();
    let third = take_interim_review_window(&mut state, &boot);
    assert!(third.contains("(no speech was captured)"));
    assert_eq!(state.interim_transcript_lines, 0);

    // Only the recent notes travel. Sending the whole list grew the prefill of
    // every review by every note before it, to prevent a repeat that
    // `record_interim_notes` drops on arrival anyway.
    for index in 0..(INTERIM_CONTEXT_NOTES * 2) {
        record_interim_notes(&mut state, &format!("- note {index}"));
    }
    let bounded = take_interim_review_window(&mut state, &boot);
    assert!(
        bounded.contains(&format!("note {}", INTERIM_CONTEXT_NOTES * 2 - 1)),
        "the newest notes are the ones a new note might repeat"
    );
    assert!(
        !bounded.contains("note 0\n"),
        "a review carries the recent notes, not the whole session"
    );
    assert_eq!(state.evidence_ledger.metrics.interim_prompt_count, 4);
    // Each counted with the system instruction it goes out behind.
    let system = crate::agent::interim_system_instruction().len() + 2;
    assert_eq!(
        state.evidence_ledger.metrics.interim_prompt_bytes,
        (4 * system + first.len() + second.len() + third.len() + bounded.len()) as u64
    );
}

/// An editor cleared and then restored is a change, because the review in
/// between showed the model an empty one.
///
/// The cursor used to hold the code from before the clearing, so the restored
/// buffer compared equal to it and the review said "unchanged" to a model
/// whose last look at the editor found nothing in it.
#[test]
fn code_restored_after_an_empty_review_is_sent_again() {
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut state = RuntimeState {
        transcript: vec!["Candidate: here is my first pass".to_string()],
        code: "seen = {}".to_string(),
        ..RuntimeState::default()
    };

    let first = take_interim_review_window(&mut state, &boot);
    assert!(first.contains("seen = {}"), "{first}");

    // Cleared. The review carries no code at all, which is what makes the next
    // one a change rather than a repeat.
    state.code = "   \n".to_string();
    let cleared = take_interim_review_window(&mut state, &boot);
    assert!(!cleared.contains("seen = {}"), "{cleared}");
    assert!(!cleared.contains(INTERIM_CODE_UNCHANGED), "{cleared}");

    state.code = "seen = {}".to_string();
    let restored = take_interim_review_window(&mut state, &boot);
    assert!(restored.contains("seen = {}"), "{restored}");
    assert!(!restored.contains(INTERIM_CODE_UNCHANGED), "{restored}");

    // And a genuine repeat still says so, so the fix did not buy the change
    // report by sending the buffer every time.
    let repeated = take_interim_review_window(&mut state, &boot);
    assert!(repeated.contains(INTERIM_CODE_UNCHANGED), "{repeated}");
}

/// A review is owned for as long as it runs, and only for as long as it runs.
///
/// Both halves cost something. A task that panics answers nothing, so a loop
/// that tracked "one is running" separately would believe one forever and spend
/// a single panic to disable every remaining pause. And a handle dropped
/// without an abort keeps running against a room that has gone, holding a
/// cloned API key.
#[tokio::test]
async fn a_review_slot_clears_however_its_task_ended() {
    let mut slot = SideCall::default();
    assert!(!slot.is_running());
    assert!(slot.finished().is_none());

    // Bounded, like the drop cases below. A slot that stopped handing back
    // finished reviews would otherwise spin here until something outside the
    // test gave up, which reads as a hung suite rather than as the answer.
    async fn collect(slot: &mut SideCall) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while slot.finished().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a finished review has to leave the slot");
    }

    slot.start(tokio::spawn(async { "notes".to_string() }));
    assert!(slot.is_running());
    collect(&mut slot).await;
    assert!(!slot.is_running(), "a collected review leaves the slot");

    slot.start(tokio::spawn(async { panic!("the reviewer fell over") }));
    collect(&mut slot).await;
    assert!(
        !slot.is_running(),
        "a panicked review must not hold the slot for the rest of the interview"
    );

    // What the drop is for: the interview ends and the task goes with it.
    // Bounded rather than spun on, so a drop that stopped aborting fails here
    // instead of hanging the suite until something else times it out.
    let survivor = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        "never".to_string()
    });
    let watched = survivor.abort_handle();
    let mut ending = SideCall::default();
    ending.start(survivor);
    drop(ending);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !watched.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping the slot has to abort the review it was holding");

    // And `start` aborts what it replaces, so the type's promise that no call
    // site has to remember the abort holds for the one that hands it a second
    // handle as well.
    let replaced = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        "never".to_string()
    });
    let watched = replaced.abort_handle();
    let mut slot = SideCall::default();
    slot.start(replaced);
    slot.start(tokio::spawn(async { "second".to_string() }));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !watched.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replacing a running review has to abort the one it displaced");
}

/// Ending is the one decision here nobody can take back, so each thing that
/// holds it back is worth pinning.
///
/// A pause is the subtle one: `output_disposition` drops every audio frame
/// while paused, so a closing requested then is generated, dropped, and
/// followed by a report the candidate never heard coming. Pausing also clears
/// `tool_response_outstanding`, so the guard cannot be left to that flag.
#[test]
fn the_interviewers_close_waits_for_a_room_that_can_hear_it() {
    let mut activity = RuntimeActivity::new(Instant::now());
    let asked = RuntimeState {
        end_requested: true,
        ..RuntimeState::default()
    };
    assert!(ready_to_close(&asked, &activity));

    assert!(
        !ready_to_close(&RuntimeState::default(), &activity),
        "nobody asked to close"
    );
    assert!(
        !ready_to_close(
            &RuntimeState {
                paused: true,
                ..asked.clone()
            },
            &activity
        ),
        "the closing would be spoken into a room whose output is dropped"
    );
    assert!(
        !ready_to_close(
            &RuntimeState {
                ended: true,
                ..asked.clone()
            },
            &activity
        ),
        "the report has already gone"
    );

    activity.tool_response_outstanding = true;
    assert!(
        !ready_to_close(&asked, &activity),
        "the acknowledgement Gemini owes would be mistaken for the closing"
    );
}

#[test]
fn replacing_a_socket_drops_its_pending_close_request() {
    let mut state = RuntimeState {
        end_requested: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.discarding_output = true;
    activity.tool_response_outstanding = true;

    clear_abandoned_socket_work(&mut state, &mut activity);

    assert!(!state.end_requested);
    assert!(!activity.discarding_output);
    assert!(!activity.tool_response_outstanding);
}

/// The first open of an interview must ride out a 503 on the key it selected.
/// It once stopped on the first transport failure or 5xx, ending the interview
/// before the candidate heard anything. A backup does not change that: a 5xx
/// says nothing about the key, and moving to the next one during an outage
/// only spreads the traffic.
#[tokio::test]
// The handshake callback's error type is a full HTTP response.
#[allow(clippy::result_large_err)]
async fn first_cold_open_retries_a_503_on_the_selected_key() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, handshake::server};

    for (setting, value, first) in [
        ("GOOGLE_API_KEY", "cold-open-only", "cold-open-only"),
        (
            "GOOGLE_API_KEYS",
            "cold-open-first,cold-open-backup",
            "cold-open-first",
        ),
    ] {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            (setting, value),
            ("GEMINI_LIVE_MODEL", "gemini-live"),
        ])
        .unwrap();
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let refuse = seen.is_empty();
                let accepted = tokio_tungstenite::accept_hdr_async(
                    socket,
                    |request: &server::Request, response| {
                        seen.push(request.uri().query().unwrap_or_default().to_string());
                        if refuse {
                            let mut refusal = server::ErrorResponse::new(None);
                            *refusal.status_mut() = axum::http::StatusCode::SERVICE_UNAVAILABLE;
                            Err(refusal)
                        } else {
                            Ok(response)
                        }
                    },
                )
                .await;
                if let Ok(mut socket) = accepted {
                    socket.next().await.unwrap().unwrap();
                    socket
                        .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                        .await
                        .unwrap();

                    // Bounded: a close that never arrives is `shutdown`'s
                    // failure to report, and this test is about which keys were
                    // tried.
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
                        .await;
                    return seen;
                }
            }
        });
        let mut restarts = 0;
        let mut notices = Vec::new();
        let mut sent = Vec::new();
        let session = retry_cold_open(
            &keys,
            &mut restarts,
            || crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None),
            |notice| {
                // A room slow to take the estimate must not stretch the wait it
                // announced.
                let stall = if notice.get("waitSeconds").is_some() {
                    COLD_OPEN_BACKOFF / 2
                } else {
                    Duration::ZERO
                };
                notices.push(notice);
                sent.push(Instant::now());
                tokio::time::sleep(stall)
            },
        )
        .await
        .unwrap();
        let _ = session.close().await;
        let expected = format!("key={first}");
        assert_eq!(
            server.await.unwrap(),
            [expected.clone(), expected],
            "{value}"
        );
        assert_eq!(restarts, 1, "{value}");

        // What the loop reports, not what reaches a page: the first open passes
        // no room, so in production these go nowhere until a replacement.
        assert_eq!(
            notices,
            vec![
                reconnect_wait("retrying", Some(COLD_OPEN_BACKOFF)),
                reconnect_wait("retrying", None),
            ]
        );
        let waited = sent[1] - sent[0];
        assert!(
            waited < COLD_OPEN_BACKOFF + COLD_OPEN_BACKOFF / 4,
            "the announced {COLD_OPEN_BACKOFF:?} took {waited:?}"
        );
    }
}

/// A sole key's 429 is retried on the same key, as a 503 is, but it is a rate
/// limit and the page says so: labelling it unreachable told the candidate the
/// wrong thing for the whole cooldown. A sole key stays in rotation, so every
/// retry takes this path; the bound stops the loop inside the second backoff.
#[tokio::test]
// The handshake callback's error type is a full HTTP response.
#[allow(clippy::result_large_err)]
async fn a_sole_key_rate_limit_is_announced_as_quota() {
    use tokio_tungstenite::tungstenite::handshake::server;

    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "rate-limited-only"),
        ("GEMINI_LIVE_MODEL", "gemini-live"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = tokio_tungstenite::accept_hdr_async(
                socket,
                |_: &server::Request, _: server::Response| {
                    let mut refusal = server::ErrorResponse::new(None);
                    *refusal.status_mut() = axum::http::StatusCode::TOO_MANY_REQUESTS;
                    Err(refusal)
                },
            )
            .await;
        }
    });

    let mut notices = Vec::new();
    let _ = crate::gemini::first_open_within(
        COLD_OPEN_BACKOFF + COLD_OPEN_BACKOFF / 4,
        retry_cold_open(
            &keys,
            &mut 0,
            || crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None),
            |notice| {
                notices.push(notice);
                async {}
            },
        ),
    )
    .await;
    server.abort();

    assert_eq!(
        notices[..2],
        [
            reconnect_wait("quota", Some(COLD_OPEN_BACKOFF)),
            reconnect_wait("quota", None),
        ],
        "{notices:?}"
    );
    assert!(
        notices[2..]
            .iter()
            .all(|notice| notice["reason"] == "quota"),
        "{notices:?}"
    );
}

/// The first open's retries run inside a wall-clock bound, not only the
/// restart budget: the candidate is already looking at a listening interviewer,
/// and a budget of connect and setup timeouts kept them there for minutes. The
/// bound cuts a retry off mid-backoff rather than letting another attempt
/// start.
#[tokio::test]
// The handshake callback's error type is a full HTTP response.
#[allow(clippy::result_large_err)]
async fn a_first_open_gives_up_at_its_time_limit() {
    use tokio_tungstenite::tungstenite::handshake::server;

    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "first-open-limit"),
        ("GEMINI_LIVE_MODEL", "gemini-live"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = std::sync::Arc::clone(&connections);
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = tokio_tungstenite::accept_hdr_async(
                socket,
                |_: &server::Request, _: server::Response| {
                    let mut refusal = server::ErrorResponse::new(None);
                    *refusal.status_mut() = axum::http::StatusCode::SERVICE_UNAVAILABLE;
                    Err(refusal)
                },
            )
            .await;
        }
    });
    let started = Instant::now();
    let error = crate::gemini::first_open_within(
        COLD_OPEN_BACKOFF / 4,
        retry_cold_open(
            &keys,
            &mut 0,
            || crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None),
            |_| async {},
        ),
    )
    .await
    .err()
    .unwrap();
    server.abort();
    assert!(error.to_string().contains("did not open within"), "{error}");
    assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(started.elapsed() < COLD_OPEN_BACKOFF);
}

/// Every key's project out of credit: the rotation tries each once and then
/// runs dry, and the error it hands back is still the billing failure, which
/// is what the room's summary names, not the empty rotation it left behind.
#[tokio::test]
// The handshake callback's error type is a full HTTP response.
#[allow(clippy::result_large_err)]
async fn a_rotation_emptied_by_billing_failures_reports_billing() {
    use tokio_tungstenite::tungstenite::handshake::server;

    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEYS", "unpaid-a,unpaid-b"),
        ("GEMINI_LIVE_MODEL", "gemini-live"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = tokio_tungstenite::accept_hdr_async(
                socket,
                |_: &server::Request, _: server::Response| {
                    let mut refusal = server::ErrorResponse::new(None);
                    *refusal.status_mut() = axum::http::StatusCode::PAYMENT_REQUIRED;
                    Err(refusal)
                },
            )
            .await;
        }
    });
    let mut attempts = 0;
    let error = retry_cold_open(
        &keys,
        &mut 0,
        || {
            attempts += 1;
            crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        },
        |_| async {},
    )
    .await
    .err()
    .unwrap();
    server.abort();
    assert_eq!(attempts, 3);
    assert!(crate::gemini::is_billing_failure(error.as_ref()), "{error}");
    assert_eq!(
        LiveOutcome::from_error(error.as_ref(), LiveOutcome::GeminiUnreachable),
        LiveOutcome::Billing
    );
}

/// Two keys that both hit a 429 inside a minute must not end an interview one
/// key would have ridden out: the exhausted rotation waits, under the budget,
/// for its first key back. Keys that were all refused have nothing to wait for,
/// and neither does a spent budget. The wait is a minute, so a short bound
/// stands in for it: it cuts the wait off where a stop would have returned.
#[tokio::test]
// The handshake callback's error type is a full HTTP response.
#[allow(clippy::result_large_err)]
async fn an_exhausted_rotation_waits_only_for_a_key_out_on_quota() {
    use tokio_tungstenite::tungstenite::handshake::server;

    for (status, prefix, budget_left, waits) in [
        (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "all-quota",
            true,
            true,
        ),
        (
            axum::http::StatusCode::FORBIDDEN,
            "all-refused",
            true,
            false,
        ),
        (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "quota-spent",
            false,
            false,
        ),
    ] {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEYS", &format!("{prefix}-a,{prefix}-b")),
            ("GEMINI_LIVE_MODEL", "gemini-live"),
        ])
        .unwrap();
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let _ = tokio_tungstenite::accept_hdr_async(
                    socket,
                    move |_: &server::Request, _: server::Response| {
                        let mut refusal = server::ErrorResponse::new(None);
                        *refusal.status_mut() = status;
                        Err(refusal)
                    },
                )
                .await;
            }
        });
        // Both keys go out the way an interview's would, over the socket.
        for _ in 0..2 {
            assert!(
                crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
                    .await
                    .is_err()
            );
        }
        server.abort();

        let mut restarts = if budget_left { 0 } else { GEMINI_RESTART_LIMIT };
        let mut attempts = 0;
        let mut notices = Vec::new();
        let error = crate::gemini::first_open_within(
            COLD_OPEN_BACKOFF / 4,
            retry_cold_open(
                &keys,
                &mut restarts,
                || {
                    attempts += 1;
                    let error = keys.select().unwrap_err();
                    async move { Err(error.into()) }
                },
                |notice| {
                    notices.push(notice);
                    async {}
                },
            ),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(attempts, 1, "{prefix}");
        assert_eq!(notices.len(), usize::from(waits), "{prefix}");
        if waits {
            assert_eq!(notices[0]["reason"], "quota");
            assert_eq!(notices[0]["reconnecting"], true);
            assert!((1..=60).contains(&notices[0]["waitSeconds"].as_u64().unwrap()));
            assert!(error.to_string().contains("did not open within"), "{error}");
            assert_eq!(restarts, 1, "{prefix}");
        } else {
            assert!(
                error.to_string().contains("credentials exhausted"),
                "{error}"
            );
        }
    }
}

/// Who is written down first when both speakers close at once.
#[test]
fn a_closed_turn_is_recorded_in_the_order_it_opened() {
    // The usual case: the interviewer asked, the candidate answered.
    assert_eq!(
        closing_order(Some(4), Some(5)),
        ["interviewer", "candidate"]
    );

    // The case that was being reported backwards. The candidate's answer opened
    // first and a late interviewer fragment closed with it, so a fixed speaker
    // order filed the question ahead of the answer it followed.
    assert_eq!(
        closing_order(Some(9), Some(8)),
        ["candidate", "interviewer"]
    );

    // A speaker with no line has nothing to record and sorts last either way.
    assert_eq!(closing_order(None, Some(3)), ["candidate", "interviewer"]);
    assert_eq!(closing_order(Some(3), None), ["interviewer", "candidate"]);
    assert_eq!(closing_order(None, None), ["interviewer", "candidate"]);

    // Two turns cannot own one line, so the tie is unreachable from the room.
    // It is pinned anyway: the comparison is the whole function, and a tie that
    // silently changes hands is how a strict rule becomes a loose one.
    assert_eq!(
        closing_order(Some(3), Some(3)),
        ["interviewer", "candidate"]
    );
}

/// The proactive-review gate driven by an edit instead of a hand-set counter.
///
/// Every other test of this gate assigns `semantic_revision` directly, so what
/// the candidate types and what the interviewer interjects about were never
/// joined up in one test: an analyzer that reported an operator fix as
/// formatting left all of them passing and the interviewer silent.
#[test]
fn a_real_operator_fix_arms_the_proactive_review() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    let mut state = RuntimeState::default();
    let ready = |activity: &mut RuntimeActivity| ready_for_review(activity, now);

    edit(
        &mut state,
        100,
        "def search(low, high):\n    while low < high:\n        low += 1\n    return low\n",
    );
    edit(
        &mut state,
        200,
        "def search(low, high):\n    while low < high:\n        low += 1\n\n    return low\n",
    );
    ready(&mut activity);
    assert!(
        activity.watch_prompt(&mut state, now).is_none(),
        "a blank line is not something to interject about"
    );

    edit(
        &mut state,
        300,
        "def search(low, high):\n    while low <= high:\n        low += 1\n\n    return low\n",
    );
    ready(&mut activity);
    let prompt = activity
        .watch_prompt(&mut state, now)
        .expect("an off-by-one fix is a semantic edit");
    assert!(prompt.text.contains("A code change settled"));

    // The fixed line is inside the fenced excerpt, numbered as `read_editor`
    // numbers it, so the review needs no read to see it.
    let fence = position(&prompt.text, "BEGIN UNTRUSTED EDITOR (");
    assert!(
        position(&prompt.text, "2|     while low <= high:") > fence,
        "{}",
        prompt.text
    );
}

/// A watch prompt the socket refused showed the model nothing, so undoing it
/// leaves the change and the evidence it carried for the next one.
#[test]
fn an_unsent_review_leaves_its_change_for_the_next_one() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    let mut state = RuntimeState::default();
    let ready = |activity: &mut RuntimeActivity| ready_for_review(activity, now);

    // Nothing to undo before any prompt.
    activity.unsend_watch_prompt(&mut state);
    assert_eq!(activity.evidence_shown, None);

    // The first packet is the starter, which is not the candidate's edit.
    edit(&mut state, 50, "def f():\n    pass\n");
    edit(&mut state, 100, "def f():\n    return 1\n");
    ready(&mut activity);
    activity
        .watch_prompt(&mut state, now)
        .expect("the first review");
    let markers = |activity: &RuntimeActivity, state: &RuntimeState| {
        (
            activity.substantive_revision_at_last_review,
            state.code_shown.clone(),
            activity.evidence_shown.clone(),
        )
    };
    let seen = markers(&activity, &state);

    // Run before the edit: the reaction to a run shows the code that changed,
    // and this one has none to show, so it leaves the markers where they are.
    run_tests(&mut state, 150, 1, 2);
    assert_eq!(markers(&activity, &state), seen);
    edit(&mut state, 200, "def f():\n    return 2\n");
    ready(&mut activity);
    let failed = activity
        .watch_prompt(&mut state, now)
        .expect("the review that fails");
    let ran = "tests: browser-reported claims (unverified): 1 of 2 passing";
    assert!(failed.text.contains(ran), "{}", failed.text);
    assert_ne!(state.code_shown, seen.1);
    activity.unsend_watch_prompt(&mut state);
    assert_eq!(markers(&activity, &state), seen);

    // The retry carries the edit the failed prompt did, measured from what the
    // model last saw.
    ready(&mut activity);
    let retried = activity.watch_prompt(&mut state, now).expect("the retry");
    assert!(
        position(&retried.text, "    return 2")
            > position(&retried.text, "BEGIN UNTRUSTED EDITOR (")
    );
    assert!(retried.text.contains(ran), "{}", retried.text);
}

/// A Live session keeps what it was told, so a watch prompt sends only the
/// evidence lines that differ from the last one's, none at all when nothing
/// moved, and all of them to a session that has heard nothing.
#[test]
fn a_watch_prompt_sends_only_the_evidence_lines_that_changed() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    let mut state = RuntimeState::default();
    edit(&mut state, 50, "def f():\n    pass\n");
    edit(&mut state, 100, "def f():\n    return 1\n");
    ready_for_review(&mut activity, now);
    let first = activity
        .watch_prompt(&mut state, now)
        .expect("the first review");
    assert!(
        first
            .text
            .contains("Deterministic session evidence:\ntests: not run\n"),
        "{}",
        first.text
    );

    edit(&mut state, 200, "def f():\n    return 2\n");
    ready_for_review(&mut activity, now);
    let unchanged = activity
        .watch_prompt(&mut state, now)
        .expect("a second review");
    assert!(
        !unchanged.text.contains("Deterministic session evidence"),
        "{}",
        unchanged.text
    );
    assert!(!unchanged.text.contains("tests:"), "{}", unchanged.text);

    edit(&mut state, 300, "def f():\n    return 3\n");
    run_tests(&mut state, 350, 1, 2);
    ready_for_review(&mut activity, now);
    let tested = activity
        .watch_prompt(&mut state, now)
        .expect("a third review");
    assert!(
        tested.text.contains(
            "Deterministic session evidence:\ntests: browser-reported claims (unverified): 1 of 2 passing"
        ),
        "{}", tested.text
    );

    // A cold replacement starts from nothing.
    activity.evidence_shown = None;
    edit(&mut state, 400, "def f():\n    return 4\n");
    ready_for_review(&mut activity, now);
    let cold = activity
        .watch_prompt(&mut state, now)
        .expect("a review on a new session");
    assert!(
        cold.text
            .contains("tests: browser-reported claims (unverified): 1 of 2 passing"),
        "{}",
        cold.text
    );
}

/// A review is armed by a change worth one, in code that parses: a rename
/// alone is not one, and a buffer mid-edit that does not parse holds the review
/// until the next change that does.
#[test]
fn a_rename_or_code_that_does_not_parse_holds_the_review() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    let mut state = RuntimeState::default();
    edit(&mut state, 50, "def f():\n    pass\n");
    edit(
        &mut state,
        100,
        "def f():\n    total = 1\n    return total\n",
    );
    ready_for_review(&mut activity, now);
    activity
        .watch_prompt(&mut state, now)
        .expect("the first review");

    edit(
        &mut state,
        200,
        "def f():\n    count = 1\n    return count\n",
    );
    ready_for_review(&mut activity, now);
    assert!(
        activity.watch_prompt(&mut state, now).is_none(),
        "a rename armed a review"
    );

    edit(
        &mut state,
        300,
        "def f():\n    count = 2\n    return count\n",
    );
    edit(
        &mut state,
        400,
        "def f():\n    count = 2\n    return count +\n",
    );
    ready_for_review(&mut activity, now);
    assert!(
        activity.watch_prompt(&mut state, now).is_none(),
        "a review fired on code that does not parse"
    );

    edit(
        &mut state,
        500,
        "def f():\n    count = 2\n    return count + 1\n",
    );
    ready_for_review(&mut activity, now);
    activity
        .watch_prompt(&mut state, now)
        .expect("the change that parses arms it");

    // Nor a buffer past what the parser reads, which has no parse to trust: the
    // change before it is still unreviewed when it lands.
    edit(
        &mut state,
        600,
        "def f():\n    count = 3\n    return count + 1\n",
    );
    // One line past the limit, whatever the limit is.
    let unparsed = "x = 1\n".repeat(crate::agent::MAX_PARSED_BYTES / "x = 1\n".len() + 1);
    edit(&mut state, 700, &unparsed);
    ready_for_review(&mut activity, now);
    assert!(
        activity.watch_prompt(&mut state, now).is_none(),
        "a review fired on a buffer that was never parsed"
    );
}

/// A checkpoint handshake must not be the last message the replacement gets:
/// the browser result and the spoken answer can both postdate that checkpoint.
/// Driven through `brief_replacement`, the branch `replace_gemini_session`
/// takes, so a resumed socket that is sent nothing fails here.
#[tokio::test]
async fn replacement_sockets_receive_local_progress_before_continuing() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    /// What the replacement socket should receive, stated per case rather than
    /// derived, so a wrong rule in `send_recovery_brief` cannot also be the
    /// rule that predicts it.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Expect {
        /// Nothing now; the cold briefing goes out on unpause.
        Deferred,
        /// Context only, and whether it asks for the owed reply.
        Context { reply: bool },
        /// The spoken cold briefing.
        Spoken,
    }
    use Replacement::{Cold, Resumed};

    /// The resume, sent: the room loop pays the debt it carries once the
    /// send succeeds.
    fn unpause(state: &mut RuntimeState) -> String {
        let resumed = crate::agent::apply_data_event(
            state,
            crate::runtime::TOPIC_CONTROL,
            &serde_json::json!({"type": "pause_interview", "paused": false}),
            0.0,
        );
        if resumed.carries_thinking_debt {
            state.clear_thinking_debt();
        }
        resumed.generate_reply.unwrap()
    }

    let mut tool_activity = RuntimeActivity::new(Instant::now());
    tool_activity.mark_prompted(Instant::now(), None, false);
    tool_activity.note_output(Instant::now());
    tool_activity.tool_response_outstanding = true;
    let tool_continuation = Resumed {
        owed: tool_activity.owes_reply(),
    };

    // (replacement, paused, a cold briefing already owed, expected)
    for (replacement, paused, pending, expect) in [
        (Cold { owed: true }, false, false, Expect::Spoken),
        (Cold { owed: true }, true, false, Expect::Deferred),
        (
            tool_continuation,
            false,
            false,
            Expect::Context { reply: true },
        ),
        (
            Resumed { owed: false },
            false,
            false,
            Expect::Context { reply: false },
        ),
        (
            Resumed { owed: true },
            false,
            false,
            Expect::Context { reply: true },
        ),
        (
            Resumed { owed: false },
            true,
            false,
            Expect::Context { reply: false },
        ),
        (
            Resumed { owed: true },
            true,
            false,
            Expect::Context { reply: false },
        ),
        (Resumed { owed: false }, true, true, Expect::Deferred),
        (Resumed { owed: false }, false, true, Expect::Spoken),
    ] {
        let resumed = matches!(replacement, Resumed { .. });
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "recovery-test-key"),
        ])
        .unwrap();
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let setup: serde_json::Value =
                serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(
                setup["setup"]["sessionResumption"]["handle"].as_str(),
                resumed.then_some("old-checkpoint")
            );
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            let message = socket.next().await.unwrap().unwrap();
            if expect == Expect::Deferred {
                assert!(message.is_close(), "a paused replacement must stay silent");
                return None;
            }
            let message: serde_json::Value =
                serde_json::from_str(message.to_text().unwrap()).unwrap();
            let text = if let Expect::Context { reply } = expect {
                assert_eq!(message["clientContent"]["turnComplete"], reply);
                &message["clientContent"]["turns"][0]["parts"][0]["text"]
            } else {
                &message["realtimeInput"]["text"]
            };
            Some(text.as_str().unwrap().to_string())
        });
        let mut gemini = crate::gemini::live_session_with_keys_at(
            &url,
            &keys,
            &boot,
            resumed.then_some(("recovery-test-key", "old-checkpoint")),
        )
        .await
        .unwrap();
        let mut state = RuntimeState {
            paused,
            needs_cold_brief: pending,
            code: "return [0, 1]".to_string(),
            transcript: vec![
                "Candidate: Constant time and space for these two entries.".to_string(),
            ],
            last_test_run: Some(serde_json::json!({"language": "python", "passed": 3, "total": 3})),
            test_runs: 1,
            ..RuntimeState::default()
        };
        let mut activity = RuntimeActivity::new(Instant::now());
        const OWED_EVENT: &str = "The owed test reaction.";
        brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            replacement,
            Some(OWED_EVENT),
        )
        .await;
        let speaks = matches!(expect, Expect::Spoken | Expect::Context { reply: true });
        assert_eq!(activity.floor == Floor::Speaking, speaks, "{expect:?}");
        assert_eq!(activity.owes_reply(), speaks, "{expect:?}");
        if speaks {
            // What a drop before the answer leaves owed is the event, never the
            // briefing, which would nest inside the next one.
            assert_eq!(activity.prompt_text.as_deref(), Some(OWED_EVENT));
        }
        assert_eq!(state.needs_cold_brief, expect == Expect::Deferred);
        let _ = gemini.close().await;
        let sent = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        let text = match expect {
            Expect::Deferred => {
                assert!(sent.is_none());
                unpause(&mut state)
            }
            _ => sent.unwrap(),
        };
        assert!(text.contains("3/3 cases passed"));
        assert!(text.contains("Candidate: Constant time and space"));
        assert!(text.contains("return [0, 1]"));
        assert_eq!(text.matches("TIMER: about").count(), 1);
        assert!(!state.needs_cold_brief);
        if matches!(expect, Expect::Spoken | Expect::Deferred) {
            // A cold socket remembers nothing, so only the briefing can say a
            // reply is owed, now or on the unpause it was deferred to.
            let owed = replacement.owed();
            assert_eq!(text.contains("was lost with the connection"), owed);
            assert_eq!(text.matches(OWED_EVENT).count(), usize::from(owed));
        }
        if let Expect::Context { reply } = expect {
            assert!(text.contains("Your connection resumed from a checkpoint"));
            assert!(!text.contains("Do not mention the interruption, apologize, re-introduce"));
            assert_eq!(text.contains("was lost with the connection"), reply);
            assert_eq!(text.contains(OWED_EVENT), reply);
            assert_eq!(text.contains("not a request to speak"), !reply);
            if paused {
                // A reply owed during the pause could not be asked for then, so
                // unpausing asks for it; otherwise the plain resume line.
                let owed = matches!(replacement, Resumed { owed: true });
                assert_eq!(state.owed_reply_on_resume.is_some(), owed);
                let unpaused = unpause(&mut state);
                assert!(unpaused.starts_with("The interview has resumed."));
                assert!(!unpaused.contains("checkpoint"));
                assert_eq!(unpaused.contains("was lost with the connection"), owed);
                assert_eq!(unpaused.contains(OWED_EVENT), owed);
                assert!(state.owed_reply_on_resume.is_none(), "asked for once");
            }
        }
    }
}

/// What the replaced socket owed survives the teardown that clears it, and
/// the teardown leaves nothing of the old socket's turn behind.
#[test]
fn a_replaced_socket_reports_the_reply_it_owed_before_clearing_it() {
    let (mut output_audio, _frames) = test_output_audio();
    let now = Instant::now();
    type Setup = fn(&mut RuntimeActivity, Instant);
    let cases: [(&str, Setup, bool, &str); 5] = [
        (
            "idle",
            |_, _| {},
            false,
            "candidate=false prompt=none tool=false generating=false",
        ),
        (
            "prompt",
            |activity, now| activity.mark_prompted(now, Some("the owed event"), false),
            true,
            "candidate=false prompt=1 tool=false generating=false",
        ),
        (
            "editor review",
            |activity, now| activity.mark_prompted(now, None, true),
            false,
            "candidate=false prompt=none tool=false generating=false",
        ),
        (
            "candidate finished",
            |activity, now| activity.note_candidate_finished(now, false),
            true,
            "candidate=true prompt=none tool=false generating=false",
        ),
        (
            "tool continuation",
            |activity, _| activity.tool_response_outstanding = true,
            true,
            "candidate=false prompt=none tool=true generating=false",
        ),
    ];
    for (name, setup, owed, debt) in cases {
        let mut activity = RuntimeActivity::new(now);
        activity.evidence_shown = Some(vec!["tests: 1 of 1 passing".to_string()]);
        let mut state = RuntimeState {
            end_requested: true,
            ..RuntimeState::default()
        };
        setup(&mut activity, now);
        assert_eq!(
            hand_over(&mut state, &mut activity, &mut output_audio),
            (
                owed,
                debt.to_string(),
                (name == "prompt").then(|| "the owed event".to_string())
            ),
            "{name}"
        );
        assert!(!activity.owes_reply(), "{name}");
        assert!(activity.prompted_at.is_none(), "{name}");
        assert_eq!(activity.floor, Floor::Listening, "{name}");
        assert!(!state.end_requested, "{name}");
        assert!(
            activity.evidence_shown.is_none(),
            "{name}: the next watch prompt sends every line"
        );
    }
}

/// Only output that could be the answer settles a prompt. A turn boundary may
/// belong to the generation before the prompt, and blank text says nothing.
#[test]
fn only_real_output_answers_a_prompt() {
    let cases = [
        (
            GeminiEvent::Audio {
                bytes: vec![0; 4],
                mime_type: "audio/pcm;rate=24000".to_string(),
            },
            true,
        ),
        (
            GeminiEvent::Audio {
                bytes: Vec::new(),
                mime_type: "audio/pcm;rate=24000".to_string(),
            },
            false,
        ),
        (GeminiEvent::ToolCall(Vec::new()), true),
        (
            GeminiEvent::OutputTranscript("Looks good.".to_string()),
            true,
        ),
        (GeminiEvent::Text("Next question.".to_string()), true),
        (GeminiEvent::OutputTranscript("  ".to_string()), false),
        (GeminiEvent::Text(String::new()), false),
        (GeminiEvent::TurnComplete, false),
        (GeminiEvent::Interrupted, false),
        (GeminiEvent::InputTranscript("hello".to_string()), false),
        (
            GeminiEvent::GoAway {
                time_left: "50s".to_string(),
            },
            false,
        ),
    ];
    for (event, answers) in cases {
        assert_eq!(session::answers_prompt(&event), answers, "{event:?}");
    }
}

/// A briefing that failed to send hands its debt to the next replacement:
/// a cold one is retried as a cold briefing, an owed reply stays owed.
#[test]
fn a_failed_briefing_keeps_its_debt_for_the_next_socket() {
    let now = Instant::now();

    // A cold briefing asks for the reply the old socket owed, so a cold one
    // that never went out leaves that reply owed as well as itself.
    for (replacement, cold_owed, reply_owed) in [
        (Replacement::Cold { owed: true }, true, true),
        (Replacement::Cold { owed: false }, true, false),
        (Replacement::Resumed { owed: true }, false, true),
        (Replacement::Resumed { owed: false }, false, false),
    ] {
        let mut state = RuntimeState::default();
        let mut activity = RuntimeActivity::new(now);
        activity.mark_prompted(now, Some("an answered prompt"), false);
        activity.note_output(Instant::now());
        keep_recovery_debt(&mut state, &mut activity, replacement, Some("owed event"));
        assert_eq!(state.needs_cold_brief, cold_owed);
        assert_eq!(activity.owes_reply(), reply_owed);
        if reply_owed {
            assert_eq!(activity.prompt_text.as_deref(), Some("owed event"));
        }
    }
}

/// A briefing whose send fails leaves its debt for the next socket, through
/// the same function a replacement calls. The socket is closed before the
/// briefing, which is the state a replacement that died again is found in.
#[tokio::test]
async fn a_briefing_that_fails_to_send_keeps_its_debt() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    use Replacement::{Cold, Resumed};
    for (replacement, cold_owed, reply_owed) in [
        (Cold { owed: true }, true, true),
        (Resumed { owed: true }, false, true),
    ] {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "recovery-test-key"),
        ])
        .unwrap();
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let _ = socket.next().await;
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            while socket.next().await.is_some() {}
        });
        let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
            .await
            .unwrap();
        gemini.shutdown().await.unwrap();

        let mut state = RuntimeState::default();
        let mut activity = RuntimeActivity::new(Instant::now());
        brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            replacement,
            Some(REACTION),
        )
        .await;
        assert_eq!(activity.prompt_text.as_deref(), Some(REACTION));
        assert_eq!(state.needs_cold_brief, cold_owed, "{cold_owed}");
        assert_eq!(activity.owes_reply(), reply_owed);
        assert_eq!(activity.floor, Floor::Listening);
        server.await.unwrap();
    }
}

/// The clock on the #66 log lines reads as the interview timer does.
#[test]
fn the_log_clock_reads_minutes_and_seconds_since_the_start() {
    let state = RuntimeState {
        started_at: Instant::now() - Duration::from_secs(125),
        ..RuntimeState::default()
    };
    assert!(session::log_clock(&state).starts_with("2:05."));
    assert_eq!(session::clock(Duration::from_millis(125_007)), "2:05.007");
    assert_eq!(session::clock(Duration::from_millis(59_999)), "0:59.999");
    assert_eq!(session::clock(Duration::from_millis(600_000)), "10:00.000");
}

/// Whether a `GoAway` is still held, which is what names an expired one.
#[test]
fn a_deferred_restart_reports_whether_it_is_held() {
    let mut restart = DeferredRestart::default();
    assert!(!restart.is_armed());
    assert!(!restart.request(Floor::Speaking, false, false, false));
    assert!(restart.is_armed());
    restart.cancel();
    assert!(!restart.is_armed());
}

/// Opens a session against a local fake Gemini that answers setup and hands
/// back the first message the client sends after it, as text.
async fn fake_resumed_socket() -> (
    GeminiLiveSession,
    tokio::task::JoinHandle<serde_json::Value>,
) {
    fake_socket(Some("checkpoint-before-the-run")).await
}

/// The same fake, resumed from `handle` or opened cold without one. It never
/// sends a model event, which is the silent socket the reply watchdog exists
/// for: setup succeeds, and nothing ever answers.
async fn fake_socket(
    handle: Option<&str>,
) -> (
    GeminiLiveSession,
    tokio::task::JoinHandle<serde_json::Value>,
) {
    let (gemini, server) = fake_recording_socket(handle, 1).await;
    (
        gemini,
        tokio::spawn(async move { server.await.unwrap().remove(0) }),
    )
}

/// `fake_socket` for a sequence: hands back the first `count` messages the
/// client sends after setup, in order.
async fn fake_recording_socket(
    handle: Option<&str>,
    count: usize,
) -> (
    GeminiLiveSession,
    tokio::task::JoinHandle<Vec<serde_json::Value>>,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "sequence-test-key"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let _setup = socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();

        // Bounded, so a write that never happens fails the test instead of
        // hanging it: under mutation testing a hang is a timeout, which is
        // neither caught nor missed.
        let mut messages = Vec::with_capacity(count);
        while messages.len() < count {
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("the client never sent the message the test waits for")
                .unwrap()
                .unwrap();
            messages.push(serde_json::from_str(message.to_text().unwrap()).unwrap());
        }
        messages
    });
    let gemini = crate::gemini::live_session_with_keys_at(
        &url,
        &keys,
        &boot,
        handle.map(|handle| ("sequence-test-key", handle)),
    )
    .await
    .unwrap();
    (gemini, server)
}

/// A chunk of interviewer audio, as Gemini delivers one mid-reply.
fn audio_chunk() -> GeminiEvent {
    GeminiEvent::Audio {
        bytes: vec![1, 2],
        mime_type: "audio/pcm".into(),
    }
}

/// The text a fake socket was sent: a cold briefing goes out as realtime
/// input, a resumed one as client content.
fn sent_text(sent: &serde_json::Value) -> &str {
    sent["realtimeInput"]["text"]
        .as_str()
        .or_else(|| sent["clientContent"]["turns"][0]["parts"][0]["text"].as_str())
        .unwrap()
}

/// A candidate who has run passing tests and answered the complexity, as the
/// reported sessions had, with the analysis recorded. Mirrors the
/// `with_written_code` state in tests/agent.rs, which this crate cannot reach.
fn tested_and_answered() -> RuntimeState {
    let mut state = RuntimeState {
        code: "def two_sum(nums, target):\n    seen = {}\n    return []\n".to_string(),
        code_templates: [("python".to_string(), String::new())].into(),
        ..RuntimeState::default()
    };
    receive_test_run(&mut state);
    state
        .transcript
        .push("Candidate: Linear time with the map, linear space, and duplicates work.".into());
    crate::agent::record_framework_evidence(
        &mut state,
        &serde_json::json!({"phase": "optimizations", "source": observed_source("optimizations"),
            "kind": "observed", "confidence": 90, "summary": "Gave O(n) time and space."}),
    )
    .unwrap();
    state
}

/// The test reaction the stalled socket never answered, which the resumed
/// briefing has to name.
const REACTION: &str =
    "[SYSTEM EVENT] The candidate just ran the built-in test cases and every one passed.";

/// How the old socket stalled before its `GoAway` could be spent.
#[derive(Clone, Copy, Debug)]
enum Stall {
    /// The candidate finished their analysis and Gemini sent nothing back.
    CandidateTurn,
    /// The test reaction went out and Gemini never answered it.
    Reaction,
}

/// Drives the reported sequence through the steps the room loop takes, each
/// by the function it calls: the stall, a `GoAway` held on the owed reply, the
/// watch tick at twenty seconds that lets it go, the hand-over, and the
/// briefing the resumed socket receives. Returns the briefing text and the
/// activity it left behind.
async fn replace_after_stall(
    mut state: RuntimeState,
    stall: Stall,
    advisory: bool,
) -> (String, RuntimeActivity) {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let (mut output_audio, _frames) = test_output_audio();
    let mut restart = DeferredRestart::default();
    match stall {
        Stall::CandidateTurn => activity.note_candidate_finished(start, false),
        Stall::Reaction => activity.mark_prompted(start, Some(REACTION), false),
    }
    let before = activity.prompt_sequence;
    if advisory {
        assert!(!restart.request(
            activity.floor,
            output_audio.is_playing(),
            activity.reply_in_flight(),
            activity.tool_response_outstanding,
        ));
    }

    // A tick short of the stall changes nothing; the one at it lets the held
    // advisory go.
    let early = activity.settle_stalls(start + PROMPT_STALL - Duration::from_millis(1), false);
    assert!(!early.spend_restart, "{stall:?}: too early");
    let stalls = activity.settle_stalls(start + PROMPT_STALL, false);
    assert!(stalls.spend_restart, "{stall:?}");
    if advisory {
        assert!(restart.take_if_settled(&activity, output_audio.is_playing()));
    } else {
        // No advisory to spend: the watch tick's own decision is what replaces
        // the socket, at the reply timeout and not before.
        let just_short = start + REPLY_TIMEOUT - Duration::from_millis(1);
        assert_ne!(
            reply_watch(&state, &activity, just_short, false),
            ReplyWatch::Recover
        );
        assert_eq!(
            reply_watch(&state, &activity, start + REPLY_TIMEOUT, false),
            ReplyWatch::Recover
        );
        restart.cancel();
    }
    assert_eq!(stalls.prompt_released, matches!(stall, Stall::Reaction));

    let (owed, _, owed_prompt) = hand_over(&mut state, &mut activity, &mut output_audio);
    assert!(owed, "{stall:?}: the old socket owed a reply");
    let (mut gemini, server) = fake_resumed_socket().await;
    let spoke = brief_replacement(
        &mut gemini,
        &mut state,
        &mut activity,
        Replacement::Resumed { owed },
        owed_prompt.as_deref(),
    )
    .await;
    let _ = gemini.close().await;
    let sent = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert!(spoke, "{stall:?}");
    assert_eq!(
        sent["clientContent"]["turnComplete"], true,
        "{stall:?}: asked to answer"
    );
    assert!(
        activity.owes_reply(),
        "{stall:?}: the new socket owes that answer now"
    );
    assert_eq!(
        activity.prompt_sequence,
        before + 1,
        "{stall:?}: the briefing is a prompt with its own number"
    );
    (sent_text(&sent).to_string(), activity)
}

/// The reported sequence, both ways the old socket can stall: the resumed
/// socket is asked for the owed reply, and what it is sent is the resumed
/// briefing for this state. The briefing's wording is the prompt tests'
/// business; this checks the socket got that briefing, and asked to answer.
#[tokio::test]
async fn a_reply_left_owed_by_a_stall_survives_a_held_go_away() {
    for stall in [Stall::CandidateTurn, Stall::Reaction] {
        let state = tested_and_answered();
        let (text, _) = replace_after_stall(state.clone(), stall, true).await;
        let owed_prompt = matches!(stall, Stall::Reaction).then_some(REACTION);
        assert!(
            text.starts_with(&crate::agent::resumed_context(&state, true, owed_prompt)),
            "{stall:?}: {text}"
        );
    }
}

/// The `tests:` log line: what the run earned and how it compares, whether the
/// credited run still covers the editor, and what became of the reaction. A
/// packet dropped before judgement prints no counts.
#[test]
fn the_tests_log_line_says_how_a_run_was_judged() {
    use crate::agent::{SincePrevious, TestRunNote};
    let run = serde_json::json!({"passed": 2, "total": 3});
    let line = |credited, reacted, ended| {
        session::TestRunLine {
            run: Some(&run),
            note: Some(TestRunNote { credited }),
            credited_run_is_current: true,
            reacted,
            ended,
        }
        .to_string()
    };
    assert_eq!(
        line(Some(SincePrevious::Unchanged), true, false),
        "passed=2 total=3 credited=Unchanged credited_run_is_current=true outcome=reaction_requested"
    );
    assert!(line(None, false, false).contains("credited=no"));
    assert!(line(None, false, false).ends_with("outcome=cooldown"));
    assert!(line(None, false, true).ends_with("outcome=ended"));
    let dropped = session::TestRunLine {
        run: Some(&run),
        note: None,
        credited_run_is_current: true,
        reacted: false,
        ended: false,
    };
    assert_eq!(dropped.to_string(), "outcome=dropped");
}

/// A held `GoAway` is spent only once the output has settled: not while audio
/// still plays, a tool reply is owed, or Gemini holds the floor.
#[test]
fn a_held_go_away_is_taken_only_once_the_room_has_settled() {
    let mut activity = RuntimeActivity::new(Instant::now());
    let mut restart = DeferredRestart::default();
    assert!(
        !restart.take_if_settled(&activity, false),
        "nothing is held"
    );

    assert!(!restart.request(Floor::Speaking, false, false, false));
    assert!(
        !restart.take_if_settled(&activity, true),
        "audio still playing"
    );
    activity.tool_response_outstanding = true;
    assert!(
        !restart.take_if_settled(&activity, false),
        "a tool reply owed"
    );
    activity.tool_response_outstanding = false;
    activity.floor = Floor::Speaking;
    assert!(
        !restart.take_if_settled(&activity, false),
        "Gemini still speaking"
    );
    activity.floor = Floor::Listening;
    assert!(restart.take_if_settled(&activity, false));
    assert!(!restart.take_if_settled(&activity, false), "spent once");
}

/// The shared fields of every `prompt:` log line: the prompt's number and the
/// progress it was sent against.
#[test]
fn the_prompt_log_fields_name_the_prompt_and_its_progress() {
    let mut activity = RuntimeActivity::new(Instant::now());
    let mut state = RuntimeState {
        code: "def two_sum(nums, target):\n    seen = {}\n    return []\n".to_string(),
        code_templates: [("python".to_string(), String::new())].into(),
        ..RuntimeState::default()
    };
    receive_test_run(&mut state);
    crate::agent::record_framework_evidence(
        &mut state,
        &serde_json::json!({"phase": "optimizations", "source": observed_source("optimizations"),
            "kind": "observed", "confidence": 90, "summary": "Gave O(n) time and space."}),
    )
    .unwrap();
    activity.mark_prompted(Instant::now(), None, false);
    activity.mark_prompted(Instant::now(), None, false);
    assert_eq!(
        session::prompt_fields(&state, &activity),
        "id=2 test_runs=1 evidenced=test,optimizations"
    );
    let line = session::prompt_line(&state, &activity, "kind=briefing", "room-1");
    assert!(line.starts_with("prompt: at="), "{line}");
    assert!(
        line.ends_with(" kind=briefing id=2 test_runs=1 evidenced=test,optimizations room=room-1"),
        "{line}"
    );
}

/// A tool response Gemini never continues from stalls like a prompt: at
/// twenty seconds the held `GoAway` may go, and the continuation stays owed as
/// a prompt so the replacement still gives it.
#[test]
fn a_stalled_tool_continuation_releases_a_held_go_away() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let mut restart = DeferredRestart::default();
    activity.note_tool_response(start);
    assert!(!restart.request(
        activity.floor,
        false,
        false,
        activity.tool_response_outstanding
    ));
    assert!(
        !activity
            .settle_stalls(start + PROMPT_STALL - Duration::from_millis(1), false)
            .spend_restart
    );
    let stalls = activity.settle_stalls(start + PROMPT_STALL, false);
    assert!(stalls.spend_restart);
    assert!(!activity.tool_response_outstanding);
    assert!(activity.owes_prompt(), "the continuation is still owed");
    assert!(restart.take_if_settled(&activity, false));
}

/// What a stalled tool continuation owes is the continuation, not the text of
/// an earlier prompt already answered, which a briefing would present as
/// unanswered.
#[test]
fn a_stalled_tool_continuation_owes_no_earlier_prompt() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("an answered reaction"), false);
    activity.note_output(Instant::now());
    activity.note_turn_boundary(Instant::now());
    activity.note_tool_response(start);
    assert!(
        activity
            .settle_stalls(start + PROMPT_STALL, false)
            .spend_restart
    );
    assert!(activity.owes_prompt());
    assert_eq!(activity.prompt_text, None);
}

/// A continuation that has begun is not stalled while it keeps producing,
/// however long it runs, or while what it said is still playing: its release
/// would let a pending close start the wrap-up mid-turn. One that began and
/// then went silent for a stall is released, or a close waiting on it would
/// never be acted on.
#[test]
fn a_continuation_under_way_does_not_stall() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_tool_response(start);
    let mut at = start;
    for _ in 0..3 {
        at += PROMPT_STALL - Duration::from_secs(1);
        activity.note_output(at);
        assert!(!activity.settle_stalls(at, false).spend_restart);
    }
    assert!(activity.tool_response_outstanding);

    let silent = at + PROMPT_STALL;
    assert!(
        !activity.settle_stalls(silent, true).spend_restart,
        "its audio still plays"
    );
    assert!(activity.tool_response_outstanding);
    assert!(activity.settle_stalls(silent, false).spend_restart);
    assert!(!activity.tool_response_outstanding);
}

/// The stall release leaves the floor alone while earlier audio still plays:
/// that is the playout's floor, and handing it back mid-sentence resets the
/// silence timers early.
#[test]
fn a_stalled_prompt_waits_for_playout_before_the_floor_goes_back() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, None, false);
    assert!(
        !activity
            .settle_stalls(start + PROMPT_STALL, true)
            .prompt_released
    );
    assert_eq!(activity.floor, Floor::Speaking);
    assert!(
        activity
            .settle_stalls(start + PROMPT_STALL, false)
            .prompt_released
    );
}

/// The discarded turn's late ending belongs to the old reply. It must not
/// clear the resume prompt or the candidate turn that superseded it.
#[test]
fn a_discarded_turn_ending_preserves_work_sent_after_resume() {
    for ending in [GeminiEvent::Interrupted, GeminiEvent::TurnComplete] {
        for candidate_spoke in [false, true] {
            let start = Instant::now();
            let mut activity = RuntimeActivity::new(start);
            activity.discarding_output = true;
            activity.mark_prompted(start, Some("resume"), false);
            if candidate_spoke {
                activity.note_candidate_finished(start, false);
            }
            assert!(!accept_gemini_event(
                &ending,
                &mut activity,
                false,
                Instant::now()
            ));
            assert!(!activity.discarding_output);
            assert!(activity.owes_reply());
            assert_eq!(activity.reply_in_flight(), candidate_spoke);
            assert_eq!(activity.owes_prompt(), !candidate_spoke);
            assert!(activity.reply_timed_out(start + REPLY_TIMEOUT, false));
        }
    }
}

/// A candidate who speaks over a prompt nothing has answered yet takes over
/// its debt, and with it the floor the prompt took: left there, no stall
/// releases it, and a held `GoAway` is never spent.
#[test]
fn a_candidate_turn_over_an_unanswered_prompt_still_stalls() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, None, false);
    activity.note_candidate_finished(start, false);
    assert!(!activity.owes_prompt());
    assert_eq!(activity.floor, Floor::Speaking);
    let stalls = activity.settle_stalls(start + PROMPT_STALL, false);
    assert!(stalls.prompt_released && stalls.spend_restart);
    assert_eq!(activity.floor, Floor::Listening);
}

/// Refusing `end_interview` with Test already recorded names what is missing,
/// complexity, and does not invite another Run.
#[test]
fn a_refused_ending_after_test_does_not_invite_another_run() {
    let mut state = tested_and_answered();
    state
        .framework_evidence
        .retain(|evidence| evidence.phase != crate::agent::FrameworkPhase::Optimizations);
    crate::agent::record_framework_evidence(
        &mut state,
        &serde_json::json!({"phase": "test", "source": "test_event",
            "kind": "observed", "confidence": 90, "summary": "Ran the tests."}),
    )
    .unwrap();
    let call = crate::gemini::GeminiFunctionCall {
        id: "end".to_string(),
        name: crate::runtime::TOOL_END_INTERVIEW.to_string(),
        args: serde_json::json!({}),
    };
    let refusal = execute_tool_call(&mut state, &call)["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        refusal.starts_with("The coding round has no Optimizations evidence yet"),
        "{refusal}"
    );
    assert!(!refusal.contains("Test and Optimizations"));
    assert!(!refusal.contains("click Run"));
}

#[tokio::test(start_paused = true)]
async fn overdue_playout_wakes_even_when_the_loop_missed_its_deadline() {
    let deadline = Instant::now() - Duration::from_millis(1);
    tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_playout(Floor::AwaitingPlayout, deadline),
    )
    .await
    .expect("a drained queue must release the waiting floor");
    for floor in [Floor::Listening, Floor::Speaking] {
        assert!(
            tokio::time::timeout(Duration::from_millis(5), wait_for_playout(floor, deadline))
                .await
                .is_err(),
            "a floor not waiting for playout must not wake"
        );
    }
    let deadline = (tokio::time::Instant::now() + Duration::from_millis(30)).into_std();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(5),
            wait_for_playout(Floor::AwaitingPlayout, deadline)
        )
        .await
        .is_err(),
        "queued audio must finish before releasing the floor"
    );
    tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_playout(Floor::AwaitingPlayout, deadline),
    )
    .await
    .unwrap();
    assert!(tokio::time::Instant::now() >= deadline.into());
}

/// The summary is a contract with the log analyzer, which groups by `session`
/// and reads `outcome`; both shapes carry them in the same place.
#[test]
fn both_usage_summaries_share_one_spelling() {
    let usage = crate::gemini::TokenUsage::default();
    let finished = live_usage_line("room-a", 7, "live", 60, LiveOutcome::Ok, Some(2), &usage);
    assert!(
        finished.starts_with(
            "codetrial live_usage room=room-a session=7 model=live elapsed_s=60 outcome=ok sockets=2 prompt_tokens=0"
        ),
        "{finished}"
    );
    let startup = live_usage_line("room-a", 7, "live", 0, LiveOutcome::Billing, None, &usage);
    assert!(
        startup.starts_with(
            "codetrial live_usage room=room-a session=7 model=live elapsed_s=0 outcome=billing phase=startup prompt_tokens=0"
        ),
        "{startup}"
    );
}

/// The window reaches the interview's state, which is the one place the
/// checkpoint and the tool answers read it from.
#[test]
fn the_compression_window_reaches_the_interview_state() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
        ("GEMINI_CONTEXT_TRIGGER_TOKENS", "20000"),
        ("GEMINI_CONTEXT_TARGET_TOKENS", "8000"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let state = initial_runtime_state(&boot, Instant::now());
    assert_eq!(state.context_compression, config.gemini_context_compression);
    assert!(state.context_compression.is_some());
}

#[test]
fn a_replacement_abandons_an_unconfirmed_thinking_hold() {
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Requested {
            since: std::time::Instant::now(),
        },
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    clear_abandoned_socket_work(&mut state, &mut activity);
    assert_eq!(state.thinking_hold, ThinkingHold::Off);
}

#[tokio::test]
async fn a_resumed_socket_waits_through_thinking_and_retains_its_reply() {
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held {
            since: std::time::Instant::now(),
        },
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    let (mut gemini, server) = fake_resumed_socket().await;
    assert!(
        !brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            Replacement::Resumed { owed: true },
            Some("the outstanding question")
        )
        .await
    );
    assert!(
        state
            .owed_reply_on_resume
            .clone()
            .flatten()
            .unwrap()
            .contains("the outstanding question")
    );
    let sent = server.await.unwrap();
    assert_eq!(sent["clientContent"]["turnComplete"], false);
}

#[tokio::test]
async fn yielding_sends_the_native_audio_stream_finalization_signal() {
    let (mut gemini, server) = fake_resumed_socket().await;
    gemini.end_audio_turn().await.unwrap();
    assert_eq!(
        server.await.unwrap(),
        crate::gemini::realtime_audio_end_message()
    );
}

#[tokio::test]
async fn a_socket_replacement_answers_an_unconfirmed_request_instead_of_stranding_it() {
    let start = Instant::now();
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    activity.observe_thinking_fragment(&mut state, "Let me think", Instant::now(), 100);
    let (mut output_audio, _frames) = test_output_audio();
    let (owed, _, _) = hand_over(&mut state, &mut activity, &mut output_audio);
    assert!(owed);
    assert!(!state.thinking_hold.is_active());
    let (mut gemini, server) = fake_resumed_socket().await;
    assert!(
        brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            Replacement::Cold { owed },
            None
        )
        .await
    );
    let sent = server.await.unwrap();
    assert!(
        sent["realtimeInput"]["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("[SYSTEM EVENT]"))
    );
    assert_eq!(activity.floor, Floor::Speaking);

    state.thinking_hold = ThinkingHold::Held {
        since: std::time::Instant::now(),
    };
    clear_abandoned_socket_work(&mut state, &mut activity);
    assert!(
        state.thinking_hold.is_active(),
        "a confirmed hold survives replacement"
    );
}

#[test]
fn a_thinking_hold_cancels_a_model_requested_close_after_its_audio_was_cut() {
    let mut state = RuntimeState {
        end_requested: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.tool_response_outstanding = true;
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({"type":"thinking","thinking":true}),
        0.0,
    );
    let (mut output_audio, _frames) = test_output_audio();
    cut_off_turn(&mut activity, &mut output_audio);
    assert!(!activity.tool_response_outstanding);
    assert!(!ready_to_close(&state, &activity));
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({"type":"thinking","thinking":false}),
        0.0,
    );
    assert!(!state.thinking_hold.is_active());
    assert!(!state.end_requested);
    assert!(!ready_to_close(&state, &activity));
    state.end_requested = true;
    assert!(ready_to_close(&state, &activity));
}

#[test]
fn a_spoken_hold_cancels_the_old_close_before_the_candidate_resumes() {
    let mut state = RuntimeState {
        end_requested: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.observe_thinking_fragment(&mut state, "Wait, let me think", Instant::now(), 100);
    assert!(!state.end_requested);
    activity.confirm_thinking_request(&mut state, Instant::now(), 101);
    assert_eq!(state.take_thinking_notice(), Some(true));
    activity.observe_thinking_fragment(
        &mut state,
        "Actually, the complexity is",
        Instant::now(),
        102,
    );
    assert_eq!(state.take_thinking_notice(), Some(false));
    assert!(!ready_to_close(&state, &activity));
}

#[tokio::test]
async fn choosing_thinking_ends_the_audio_stream_and_ignores_the_transcript_it_releases() {
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held {
            since: std::time::Instant::now(),
        },
        ..RuntimeState::default()
    };
    let click = Instant::now();
    let mut activity = RuntimeActivity::new(click);
    let (mut gemini, server) = fake_resumed_socket().await;
    let mut audio = Vec::new();
    begin_button_hold(
        &mut gemini,
        &mut audio,
        &mut activity,
        click,
        THINKING_TRANSCRIPT_GRACE,
    )
    .await
    .unwrap();
    assert_eq!(
        server.await.unwrap(),
        crate::gemini::realtime_audio_end_message()
    );
    activity.observe_thinking_fragment(
        &mut state,
        "I would use a hash map",
        click + Duration::from_millis(300),
        1_300,
    );
    assert_eq!(state.take_thinking_notice(), None);
    assert!(state.thinking_hold.is_active());
}

/// `settle_hold` against `turn`, clicked now, at the default grace.
async fn settle(
    turn: &mut TurnState,
    output_audio: &mut OutputAudio,
    gemini: &mut GeminiLiveSession,
    media: &mut CandidateMedia,
    result: &crate::agent::DataEventResult,
    reply: &mut Option<String>,
) -> HoldEffects {
    let mut board = board::Board::new();
    let mut context = turn.context(output_audio, gemini, &mut board, media);
    settle_hold(
        &mut context,
        result,
        reply,
        Instant::now(),
        THINKING_TRANSCRIPT_GRACE,
    )
    .await
}

/// A `TurnState` for `settle_hold`, which reads the room loop's whole context.
fn hold_turn(state: RuntimeState) -> TurnState {
    TurnState {
        state,
        agent_state: String::new(),
        activity: RuntimeActivity::new(Instant::now()),
        turns: SpeakerTurns::default(),
    }
}

#[tokio::test]
async fn drawing_yields_finish_audio_and_ask_once_with_or_without_speech() {
    for speaking in [false, true] {
        let mut turn = hold_turn(RuntimeState::default());
        if speaking {
            turn.turns.candidate.record(
                &mut turn.state.transcript,
                crate::agent::CANDIDATE_SPEAKER,
                "this box is one connected region",
            );
        }
        let (mut output_audio, _frames) = test_output_audio();
        let (mut gemini, server) = fake_recording_socket(None, 2).await;
        let mut media = CandidateMedia::new();
        let result = crate::agent::apply_data_event(
            &mut turn.state,
            crate::runtime::TOPIC_CONTROL,
            &serde_json::json!({"type":"yield_turn","surface":"example"}),
            0.0,
        );
        let mut reply = result.generate_reply.clone();
        let effects = settle(
            &mut turn,
            &mut output_audio,
            &mut gemini,
            &mut media,
            &result,
            &mut reply,
        )
        .await;
        assert_eq!(effects, HoldEffects::default());
        if speaking {
            assert!(
                reply.is_none(),
                "the spoken turn carries the drawing context"
            );
            assert!(turn.activity.thinking_reply_fallback.is_some());
        } else {
            let prompt = reply.expect("drawing alone still needs an explicit reply");
            send_model_text(
                &mut gemini,
                &mut turn.state,
                ModelInputKind::Turn,
                TurnCause::Turn,
                &prompt,
            )
            .await
            .unwrap();
        }
        let sent = server.await.unwrap();
        if speaking {
            assert_eq!(sent[0]["clientContent"]["turnComplete"], false);
            assert!(sent[0].to_string().contains("Example drawing"));
            assert_eq!(sent[1], crate::gemini::realtime_audio_end_message());
        } else {
            assert_eq!(sent[0], crate::gemini::realtime_audio_end_message());
            assert!(
                sent[1]["realtimeInput"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Example drawing")
            );
        }
    }
}

#[tokio::test]
async fn yielding_flushes_buffered_speech_then_ends_the_stream() {
    let mut turn = hold_turn(RuntimeState::default());
    let (mut output_audio, _frames) = test_output_audio();
    let (mut gemini, server) = fake_recording_socket(Some("checkpoint-before-the-run"), 2).await;
    let mut media = CandidateMedia::new();
    media.audio_bytes = vec![0; 640];
    let result = crate::agent::DataEventResult {
        yield_turn: true,
        ..Default::default()
    };
    let mut reply = None;
    let effects = settle(
        &mut turn,
        &mut output_audio,
        &mut gemini,
        &mut media,
        &result,
        &mut reply,
    )
    .await;
    assert_eq!(effects, HoldEffects::default());
    assert!(media.audio_bytes.is_empty());
    let sent = server.await.unwrap();
    assert!(sent[0]["realtimeInput"]["audio"].is_object(), "{sent:?}");
    assert_eq!(sent[1], crate::gemini::realtime_audio_end_message());
}

#[tokio::test]
async fn repeated_thinking_acknowledges_without_finalizing_resumed_speech() {
    let mut turn = hold_turn(RuntimeState::default());
    let click = Instant::now();
    let payload = serde_json::json!({"type":"thinking","thinking":true});
    let first = crate::agent::apply_data_event(
        &mut turn.state,
        crate::runtime::TOPIC_CONTROL,
        &payload,
        0.0,
    );
    assert_eq!(first.thinking_changed, Some(true));
    assert_eq!(turn.state.take_thinking_notice(), Some(true));
    turn.activity
        .ignore_input_before_hold(click, THINKING_TRANSCRIPT_GRACE);
    let original_window = turn.activity.thinking_ignore_input_until;
    let original_hold = turn.state.thinking_hold;
    let duplicate_at = click + THINKING_TRANSCRIPT_GRACE + Duration::from_millis(100);
    let (mut output_audio, _frames) = test_output_audio();
    let (mut gemini, server) = fake_recording_socket(Some("checkpoint-before-the-run"), 1).await;
    let mut media = CandidateMedia::new();
    media.audio_bytes = vec![0; 640];
    let result = crate::agent::apply_data_event(
        &mut turn.state,
        crate::runtime::TOPIC_CONTROL,
        &payload,
        0.0,
    );
    assert_eq!(result.thinking_changed, None);
    assert_eq!(turn.state.take_thinking_notice(), Some(true));
    assert_eq!(turn.state.thinking_hold, original_hold);
    let mut reply = result.generate_reply.clone();
    let effects = {
        let mut board = board::Board::new();
        let mut context = turn.context(&mut output_audio, &mut gemini, &mut board, &mut media);
        settle_hold(
            &mut context,
            &result,
            &mut reply,
            duplicate_at,
            THINKING_TRANSCRIPT_GRACE,
        )
        .await
    };
    assert_eq!(effects, HoldEffects::default());
    assert_eq!(media.audio_bytes.len(), 640);
    assert_eq!(turn.activity.thinking_ignore_input_until, original_window);
    turn.activity.observe_thinking_fragment(
        &mut turn.state,
        "I would use a hash map",
        duplicate_at + Duration::from_millis(300),
        2_000,
    );
    assert!(!turn.state.thinking_hold.is_active());
    assert_eq!(turn.state.take_thinking_notice(), Some(false));
    assert_eq!(turn.state.evidence_ledger.lifecycle.transitions, 2);
    let repeated_release = crate::agent::apply_data_event(
        &mut turn.state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({"type":"thinking","thinking":false}),
        0.0,
    );
    assert_eq!(repeated_release.thinking_changed, None);
    assert_eq!(turn.state.take_thinking_notice(), Some(false));
    gemini.end_audio_turn().await.unwrap();
    assert_eq!(
        server.await.unwrap(),
        vec![crate::gemini::realtime_audio_end_message()]
    );
}

#[tokio::test]
async fn choosing_thinking_while_jim_talks_cuts_him_off_and_says_why() {
    let mut turn = hold_turn(RuntimeState {
        thinking_hold: ThinkingHold::Held {
            since: std::time::Instant::now(),
        },
        ..RuntimeState::default()
    });
    turn.activity.mark_speaking();
    let (mut output_audio, _frames) = test_output_audio();
    let (mut gemini, server) = fake_recording_socket(Some("checkpoint-before-the-run"), 2).await;
    let mut media = CandidateMedia::new();
    let result = crate::agent::DataEventResult {
        thinking_changed: Some(true),
        ..Default::default()
    };
    let mut reply = None;
    let effects = settle(
        &mut turn,
        &mut output_audio,
        &mut gemini,
        &mut media,
        &result,
        &mut reply,
    )
    .await;
    assert_eq!(
        effects,
        HoldEffects {
            cut_off: true,
            abandoned: false
        }
    );
    assert_eq!(turn.activity.floor, Floor::Listening);
    assert!(
        turn.state.thinking_unheard_reply,
        "owed until a prompt carrying it is sent"
    );
    let sent = server.await.unwrap();
    assert_eq!(sent[0], crate::gemini::realtime_audio_end_message());
    assert!(
        sent[1].to_string().contains("has kept the floor"),
        "{sent:?}"
    );
    assert_eq!(sent[1]["clientContent"]["turnComplete"], false);
}

#[tokio::test]
async fn releasing_into_an_open_candidate_turn_delivers_the_prompt_as_context() {
    let mut turn = hold_turn(RuntimeState {
        needs_cold_brief: true,
        ..RuntimeState::default()
    });
    turn.turns.candidate.record(
        &mut turn.state.transcript,
        crate::agent::CANDIDATE_SPEAKER,
        "so I would sort first",
    );
    let (mut output_audio, _frames) = test_output_audio();
    let (mut gemini, server) = fake_recording_socket(Some("checkpoint-before-the-run"), 2).await;
    let mut media = CandidateMedia::new();
    let result = crate::agent::DataEventResult {
        thinking_changed: Some(false),
        carries_thinking_debt: true,
        ..Default::default()
    };
    let mut reply = Some("[SYSTEM EVENT] The candidate is ready.".to_string());
    let effects = settle(
        &mut turn,
        &mut output_audio,
        &mut gemini,
        &mut media,
        &result,
        &mut reply,
    )
    .await;
    assert_eq!(effects, HoldEffects::default());
    assert!(reply.is_none(), "delivered as context, not asked for again");
    assert!(!turn.state.needs_cold_brief, "the debt it carried is paid");
    assert!(
        turn.activity.thinking_reply_fallback.is_some(),
        "asked for outright if the open turn is never answered"
    );
    let sent = server.await.unwrap();
    assert_eq!(sent[0]["clientContent"]["turnComplete"], false);
    assert!(sent[0].to_string().contains("The candidate is ready."));
    assert_eq!(sent[1], crate::gemini::realtime_audio_end_message());
}

#[tokio::test]
async fn a_cold_replacement_during_a_pause_waits_to_brief_and_keeps_the_reply_it_owed() {
    let mut state = RuntimeState {
        paused: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    let (mut gemini, _server) = fake_recording_socket(Some("checkpoint-before-the-run"), 0).await;
    assert!(
        !brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            Replacement::Cold { owed: true },
            Some("the outstanding question"),
        )
        .await
    );
    assert!(state.needs_cold_brief);
    assert!(
        state
            .owed_reply_on_resume
            .clone()
            .flatten()
            .is_some_and(|owed| owed.contains("the outstanding question"))
    );
}

#[tokio::test]
async fn a_cold_replacement_during_a_hold_is_briefed_at_once_without_a_reply() {
    let mut state = RuntimeState {
        // A pause's own cold replacement left its briefing owed; this one
        // delivers it, so the hold's release must not deliver it again.
        needs_cold_brief: true,
        thinking_hold: ThinkingHold::Held {
            since: std::time::Instant::now(),
        },
        code: "return 42".into(),
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    let (mut gemini, server) = fake_resumed_socket().await;
    assert!(
        !brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            Replacement::Cold { owed: true },
            Some("the outstanding question"),
        )
        .await
    );
    let sent = server.await.unwrap();
    assert_eq!(sent["clientContent"]["turnComplete"], false);
    assert!(sent.to_string().contains("return 42"), "{sent}");
    assert!(!state.needs_cold_brief, "already briefed");
    assert_eq!(state.code_shown, state.code);
    assert!(
        state
            .owed_reply_on_resume
            .clone()
            .flatten()
            .is_some_and(|owed| owed.contains("the outstanding question"))
    );
}

#[tokio::test]
async fn a_resumed_reply_carries_the_unheard_note_it_pays() {
    let mut state = RuntimeState {
        thinking_unheard_reply: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    let (mut gemini, server) = fake_resumed_socket().await;
    assert!(
        brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            Replacement::Resumed { owed: true },
            Some("the outstanding question"),
        )
        .await
    );
    assert!(!state.thinking_unheard_reply);
    let sent = server.await.unwrap();
    assert!(
        sent.to_string()
            .contains("Nothing you said while the candidate was thinking"),
        "{sent}"
    );
}

#[tokio::test]
async fn an_ordinary_reply_leaves_the_audio_stream_alone() {
    let mut turn = hold_turn(RuntimeState::default());
    let (mut output_audio, _frames) = test_output_audio();
    let (mut gemini, server) = fake_recording_socket(Some("checkpoint-before-the-run"), 1).await;
    let mut media = CandidateMedia::new();
    media.audio_bytes = vec![0; 640];
    let result = crate::agent::DataEventResult::default();
    let mut reply = Some("[SYSTEM EVENT] The tests passed.".to_string());
    let effects = settle(
        &mut turn,
        &mut output_audio,
        &mut gemini,
        &mut media,
        &result,
        &mut reply,
    )
    .await;
    assert_eq!(effects, HoldEffects::default());
    assert_eq!(reply.as_deref(), Some("[SYSTEM EVENT] The tests passed."));
    assert_eq!(
        media.audio_bytes.len(),
        640,
        "buffered speech is not flushed"
    );

    // Nothing went out ahead of this, so it is the first thing the socket sees.
    gemini.send_context("marker", false).await.unwrap();
    let sent = server.await.unwrap();
    assert!(sent[0].to_string().contains("marker"), "{sent:?}");
}

#[tokio::test]
async fn releasing_with_no_candidate_turn_open_leaves_the_reply_to_be_asked_for() {
    let mut turn = hold_turn(RuntimeState::default());
    let (mut output_audio, _frames) = test_output_audio();
    let (mut gemini, server) = fake_recording_socket(Some("checkpoint-before-the-run"), 2).await;
    let mut media = CandidateMedia::new();
    let result = crate::agent::DataEventResult {
        thinking_changed: Some(false),
        carries_thinking_debt: true,
        ..Default::default()
    };
    let mut reply = Some("[SYSTEM EVENT] The candidate is ready.".to_string());
    let effects = settle(
        &mut turn,
        &mut output_audio,
        &mut gemini,
        &mut media,
        &result,
        &mut reply,
    )
    .await;
    assert_eq!(effects, HoldEffects::default());
    assert!(reply.is_some(), "a realtime prompt, sent after this");
    gemini.send_context("marker", false).await.unwrap();
    let sent = server.await.unwrap();
    assert_eq!(sent[0], crate::gemini::realtime_audio_end_message());
    assert!(sent[1].to_string().contains("marker"), "{sent:?}");
}

#[test]
fn a_cold_briefing_that_never_went_out_keeps_the_reply_it_asked_for() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    keep_recovery_debt(
        &mut state,
        &mut activity,
        Replacement::Cold { owed: true },
        Some("the outstanding question"),
    );
    assert!(state.needs_cold_brief);
    assert!(activity.owes_prompt());
    assert_eq!(
        activity.prompt_text.as_deref(),
        Some("the outstanding question")
    );

    // Nothing owed, nothing invented.
    let mut activity = RuntimeActivity::new(Instant::now());
    keep_recovery_debt(
        &mut state,
        &mut activity,
        Replacement::Cold { owed: false },
        None,
    );
    assert!(!activity.owes_prompt());
}

#[tokio::test]
async fn an_unanswered_turn_recovers_without_a_go_away() {
    for stall in [Stall::CandidateTurn, Stall::Reaction] {
        let (briefing, activity) = replace_after_stall(tested_and_answered(), stall, false).await;
        assert!(briefing.contains("reply"), "{briefing}");
        assert!(activity.owes_reply());
    }
}

#[test]
fn reply_timeout_requires_unanswered_work_and_no_output() {
    let start = Instant::now();
    let deadline = start + REPLY_TIMEOUT;
    let mut activity = RuntimeActivity::new(start);
    assert!(!activity.reply_timed_out(deadline, false));
    activity.mark_prompted(start, Some("editor review"), true);
    activity.settle_stalls(deadline, false);
    assert!(!activity.reply_timed_out(deadline, false));
    activity.mark_prompted(start, Some("greeting"), false);
    activity.settle_stalls(start + PROMPT_STALL, false);
    assert!(activity.reply_timed_out(deadline, false));
    assert!(!activity.reply_timed_out(deadline, true));
    activity.discarding_output = true;
    assert!(activity.reply_timed_out(deadline, false));
    activity.discarding_output = false;
    activity.note_output(start + Duration::from_secs(1));
    assert!(!activity.reply_timed_out(deadline, false));
}

#[test]
fn unanswered_tool_continuation_keeps_its_timeout_after_floor_release() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_tool_response(start);
    assert!(!activity.reply_timed_out(start + REPLY_TIMEOUT - Duration::from_millis(1), false));
    activity.settle_stalls(start + PROMPT_STALL, false);
    assert!(activity.reply_timed_out(start + REPLY_TIMEOUT, false));
    assert!(activity.owes_reply());
}

#[test]
fn a_new_candidate_turn_replaces_the_old_reply_deadline() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("old prompt"), false);
    activity.note_candidate_finished(start + Duration::from_secs(30), false);
    assert!(!activity.reply_timed_out(start + REPLY_TIMEOUT, false));
    assert!(activity.reply_timed_out(start + Duration::from_secs(30) + REPLY_TIMEOUT, false));
}

#[test]
fn a_silent_completed_turn_settles_candidate_debt() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    activity.note_turn_boundary(Instant::now());
    assert!(!activity.owes_reply());
    assert!(!activity.reply_timed_out(start + REPLY_TIMEOUT, false));
}

#[test]
fn a_generation_that_stops_making_progress_times_out() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_output(start);
    assert!(!activity.reply_timed_out(start + REPLY_TIMEOUT - Duration::from_millis(1), false));
    assert!(activity.reply_timed_out(start + REPLY_TIMEOUT, false));
    let (mut output_audio, _) = test_output_audio();
    let mut state = RuntimeState::default();
    assert!(hand_over(&mut state, &mut activity, &mut output_audio).0);
    activity.note_output(start);
    assert!(!activity.reply_timed_out(start + REPLY_TIMEOUT, true));
    activity.note_tool_response(start + Duration::from_secs(20));
    activity.settle_stalls(start + Duration::from_secs(40), false);
    assert!(!activity.reply_timed_out(start + REPLY_TIMEOUT, false));
    activity.last_output_at = Some(start + Duration::from_secs(30));
    assert!(!activity.reply_timed_out(start + Duration::from_secs(60), false));
    activity.note_turn_boundary(Instant::now());
    assert!(!activity.reply_timed_out(start + Duration::from_secs(120), false));
}

#[test]
fn paused_time_does_not_age_an_unanswered_candidate_turn() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let (mut output_audio, _) = test_output_audio();
    let mut state = RuntimeState::default();
    activity.note_candidate_finished(start, false);
    assert_eq!(activity.floor, Floor::Listening);
    state.paused = true;
    assert!(reply_watch(&state, &activity, start + REPLY_TIMEOUT, false) != ReplyWatch::Recover);
    update_pause_activity(&mut state, &mut activity, &mut output_audio, start);
    assert!(!activity.owes_reply());
    assert!(state.owed_reply_on_resume.is_some());

    // The unpause the page sends, handled the way `handle_data_packet` does:
    // the resume prompt asks for the owed reply, and its deadline starts at the
    // resume, not at the pause five minutes earlier.
    let resumed_at = start + Duration::from_secs(300);
    toggle_pause(&mut state, &mut activity, &mut output_audio, resumed_at).unwrap();
    assert!(reply_watch(&state, &activity, resumed_at, false) != ReplyWatch::Recover);
    assert!(
        reply_watch(&state, &activity, resumed_at + REPLY_TIMEOUT, false) == ReplyWatch::Recover
    );
    state.end_requested = true;
    assert!(
        reply_watch(&state, &activity, resumed_at + REPLY_TIMEOUT, false) != ReplyWatch::Recover
    );
}

#[test]
fn a_cold_brief_on_resume_keeps_the_owed_reply() {
    let mut state = RuntimeState {
        paused: true,
        needs_cold_brief: true,
        owed_reply_on_resume: Some(Some("the owed test reaction".into())),
        ..RuntimeState::default()
    };
    let reply = crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({"type": "pause_interview", "paused": false}),
        0.0,
    )
    .generate_reply
    .unwrap();
    assert!(reply.contains("the owed test reaction"));
    assert!(reply.contains("connection"));

    // Paid once the resume is sent, as the room loop does on a send that
    // succeeds; a failed one leaves it for the next socket.
    assert!(state.needs_cold_brief, "owed until the resume goes out");
    state.clear_thinking_debt();
    assert!(!state.needs_cold_brief);
    assert!(state.owed_reply_on_resume.is_none());
}

#[test]
fn silent_replacements_cannot_reset_the_restart_budget() {
    let idle = RuntimeActivity::new(Instant::now());
    let long_lived = HEALTHY_GEMINI_SOCKET * 2;
    let mut restarts = 0;
    for _ in 0..GEMINI_RESTART_LIMIT {
        assert!(take_restart_attempt(
            &mut restarts,
            replaced_socket_age(long_lived, true, &idle)
        ));
    }
    assert!(!take_restart_attempt(
        &mut restarts,
        replaced_socket_age(long_lived, true, &idle)
    ));
    assert!(take_restart_attempt(
        &mut restarts,
        replaced_socket_age(HEALTHY_GEMINI_SOCKET, false, &idle)
    ));
    assert_eq!(restarts, 1);
}

#[test]
fn only_completed_delivered_output_clears_recovery_failures() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    assert!(!completed_live_reply(
        &state,
        &activity,
        &GeminiEvent::TurnComplete
    ));
    activity.note_output(Instant::now());
    assert!(completed_live_reply(
        &state,
        &activity,
        &GeminiEvent::TurnComplete
    ));
    assert!(!completed_live_reply(
        &state,
        &activity,
        &GeminiEvent::Interrupted
    ));
    activity.discarding_output = true;
    assert!(!completed_live_reply(
        &state,
        &activity,
        &GeminiEvent::TurnComplete
    ));
    activity.discarding_output = false;
    state.paused = true;
    assert!(!completed_live_reply(
        &state,
        &activity,
        &GeminiEvent::TurnComplete
    ));
}

#[test]
fn discarded_tool_calls_do_not_answer_a_resume_prompt() {
    for tool_before_resume in [false, true] {
        let start = Instant::now();
        let mut activity = RuntimeActivity::new(start);
        activity.discarding_output = true;
        if !tool_before_resume {
            activity.mark_prompted(start, Some("resume"), false);
        }
        assert!(accept_gemini_event(
            &GeminiEvent::ToolCall(Vec::new()),
            &mut activity,
            false,
            Instant::now()
        ));
        activity.note_tool_response(start);
        assert!(!activity.generating);
        assert!(!activity.tool_response_outstanding);
        if tool_before_resume {
            activity.mark_prompted(start, Some("resume"), false);
        }
        assert!(!accept_gemini_event(
            &GeminiEvent::TurnComplete,
            &mut activity,
            false,
            Instant::now()
        ));
        assert!(activity.owes_prompt());
        assert!(!activity.prompt_behind_turn);
        assert!(activity.reply_timed_out(start + REPLY_TIMEOUT, false));
    }
}

#[test]
fn pausing_a_tool_generation_without_audio_discards_its_continuation() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let mut state = RuntimeState::default();
    let (mut output_audio, _) = test_output_audio();
    activity.note_output(Instant::now());
    activity.note_tool_response(start);
    assert_eq!(activity.floor, Floor::Listening);
    state.paused = true;
    update_pause_activity(&mut state, &mut activity, &mut output_audio, start);
    activity.mark_prompted(start, Some("resume"), false);
    assert!(!accept_gemini_event(
        &audio_chunk(),
        &mut activity,
        false,
        Instant::now()
    ));
    assert!(activity.owes_prompt());
    assert!(!activity.generating);
}

#[test]
fn a_tool_stall_preserves_the_prompt_queued_behind_it() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_output(Instant::now());
    activity.mark_prompted(start, Some(REACTION), false);
    activity.note_tool_response(start);
    activity.settle_stalls(start + PROMPT_STALL, false);
    assert!(activity.prompt_behind_turn);
    assert_eq!(activity.prompt_text.as_deref(), Some(REACTION));
    activity.note_output(Instant::now());
    activity.note_turn_boundary(Instant::now());
    assert!(activity.owes_prompt());
    let own_turn_at = activity.prompted_at.unwrap();
    assert!(!activity.reply_timed_out(
        own_turn_at + REPLY_TIMEOUT - Duration::from_millis(1),
        false
    ));
    assert!(activity.reply_timed_out(own_turn_at + REPLY_TIMEOUT, false));
}

#[test]
fn a_late_interrupted_turn_complete_does_not_settle_new_work() {
    for discarded in [false, true] {
        let start = Instant::now();
        let mut activity = RuntimeActivity::new(start);
        let (mut output_audio, _) = test_output_audio();
        activity.discarding_output = discarded;
        let dispatch = accept_gemini_event(
            &GeminiEvent::Interrupted,
            &mut activity,
            false,
            Instant::now(),
        );
        assert_eq!(dispatch, !discarded);
        if dispatch {
            cut_off_turn(&mut activity, &mut output_audio);
        }
        activity.note_candidate_finished(start, false);
        assert!(!accept_gemini_event(
            &GeminiEvent::TurnComplete,
            &mut activity,
            false,
            Instant::now()
        ));
        assert!(activity.reply_timed_out(start + REPLY_TIMEOUT, false));
        assert!(!activity.interrupted_turn_pending);
    }
}

#[test]
fn new_output_clears_the_interrupted_boundary_guard() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    assert!(accept_gemini_event(
        &GeminiEvent::Interrupted,
        &mut activity,
        false,
        Instant::now()
    ));
    activity.note_candidate_finished(start, false);
    assert!(accept_gemini_event(
        &GeminiEvent::OutputTranscript("new reply".into()),
        &mut activity,
        false,
        Instant::now()
    ));
    assert!(!activity.interrupted_turn_pending);
    assert!(accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        Instant::now()
    ));
    assert!(!activity.owes_reply());
}

#[test]
fn a_transcript_during_uncut_playout_does_not_arm_reply_recovery() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    activity.note_output(Instant::now());
    activity.note_turn_boundary(Instant::now());
    activity.floor = Floor::AwaitingPlayout;
    activity.note_candidate_finished(start + Duration::from_secs(1), true);
    activity.mark_listening();
    assert!(!activity.reply_timed_out(start + Duration::from_secs(60), false));
    // A real barge-in cuts playout first and owns a new reply deadline.
    activity.note_candidate_finished(start + Duration::from_secs(60), false);
    assert!(activity.reply_timed_out(start + Duration::from_secs(60) + REPLY_TIMEOUT, false));
}

#[test]
fn a_drained_playout_floor_records_the_candidates_new_answer() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.floor = Floor::AwaitingPlayout;
    activity.note_candidate_finished(start, false);
    assert_eq!(activity.floor, Floor::Listening);
    assert!(activity.reply_in_flight());
    assert!(activity.reply_timed_out(start + REPLY_TIMEOUT, false));
}

#[test]
fn delayed_paused_transcription_gets_a_fresh_deadline_on_resume() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let mut state = RuntimeState {
        paused: true,
        ..RuntimeState::default()
    };
    let (mut output_audio, _) = test_output_audio();
    update_pause_activity(&mut state, &mut activity, &mut output_audio, start);
    let transcribed_at = start + Duration::from_secs(1);
    activity.note_candidate_finished(transcribed_at, false);
    assert_eq!(activity.awaiting_reply_since, Some(transcribed_at));
    let resumed_at = start + Duration::from_secs(300);
    assert!(reply_watch(&state, &activity, resumed_at, false) != ReplyWatch::Recover);
    toggle_pause(&mut state, &mut activity, &mut output_audio, resumed_at)
        .expect("an unpause sends the resume prompt");
    assert_eq!(activity.awaiting_reply_since, Some(resumed_at));
    assert!(reply_watch(&state, &activity, resumed_at, false) != ReplyWatch::Recover);
    assert!(!activity.settle_stalls(resumed_at, false).spend_restart);
    assert!(
        !activity
            .settle_stalls(resumed_at + PROMPT_STALL - Duration::from_millis(1), false)
            .spend_restart
    );
    assert!(
        activity
            .settle_stalls(resumed_at + PROMPT_STALL, false)
            .spend_restart
    );
    assert!(
        reply_watch(
            &state,
            &activity,
            resumed_at + REPLY_TIMEOUT - Duration::from_millis(1),
            false
        ) != ReplyWatch::Recover
    );
    assert!(
        reply_watch(&state, &activity, resumed_at + REPLY_TIMEOUT, false) == ReplyWatch::Recover
    );
    assert!(activity.owes_reply());
}

#[test]
fn resume_rebases_tool_and_generation_progress_without_creating_debt() {
    let start = Instant::now();
    let resumed_at = start + Duration::from_secs(300);
    let mut idle = RuntimeActivity::new(start);
    idle.resume_reply_wait(resumed_at);
    assert!(!idle.owes_reply());
    assert_eq!(idle.last_output_at, None);
    assert_eq!(idle.tool_response_at, None);
    assert!(!idle.reply_timed_out(resumed_at + REPLY_TIMEOUT, false));
    idle.tool_response_at = Some(start);
    idle.last_output_at = Some(start);
    idle.resume_reply_wait(resumed_at);
    assert_eq!(idle.tool_response_at, None);
    assert_eq!(idle.last_output_at, None);
    assert!(!idle.owes_reply());

    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("required prompt"), false);
    activity.resume_reply_wait(resumed_at);
    assert_eq!(activity.prompted_at, Some(resumed_at));
    assert_eq!(activity.prompt_text.as_deref(), Some("required prompt"));
    assert!(!activity.reply_timed_out(resumed_at, false));
    assert!(activity.reply_timed_out(resumed_at + REPLY_TIMEOUT, false));

    activity.note_output(start);
    activity.note_tool_response(start);
    activity.resume_reply_wait(resumed_at);
    assert_eq!(activity.last_output_at, Some(resumed_at));
    assert_eq!(activity.tool_response_at, Some(resumed_at));
    assert!(activity.tool_response_outstanding);
    assert!(
        !activity.reply_timed_out(resumed_at + REPLY_TIMEOUT - Duration::from_millis(1), false)
    );
    assert!(activity.reply_timed_out(resumed_at + REPLY_TIMEOUT, false));
}

#[tokio::test]
async fn a_failed_resume_write_keeps_raw_debt_for_warm_and_cold_replacements() {
    for cold in [false, true] {
        for replacement in [
            Replacement::Cold { owed: true },
            Replacement::Resumed { owed: true },
        ] {
            let mut state = RuntimeState {
                paused: true,
                needs_cold_brief: cold,
                owed_reply_on_resume: Some(Some(REACTION.into())),
                ..RuntimeState::default()
            };
            let debt = ResumeDebt::capture(&state).unwrap();
            let mut activity = RuntimeActivity::new(Instant::now());
            let (mut output_audio, _) = test_output_audio();
            let resumed = crate::agent::apply_data_event(
                &mut state,
                crate::runtime::TOPIC_CONTROL,
                &serde_json::json!({"type": "pause_interview", "paused": false}),
                0.0,
            );
            assert_eq!(resumed.pause_changed, Some(false));
            let prompt = resumed.generate_reply.unwrap();
            update_pause_activity(&mut state, &mut activity, &mut output_audio, Instant::now());
            assert!(!activity.owes_reply());
            let error = record_event_prompt(
                &mut state,
                &mut activity,
                &prompt,
                Some(&debt),
                Err("closed"),
                Instant::now(),
            );
            assert_eq!(error, Err("closed"));
            assert_eq!(state.needs_cold_brief, cold);
            assert!(activity.owes_prompt());
            assert_eq!(activity.prompt_text.as_deref(), Some(REACTION));
            assert_eq!(activity.floor, Floor::Listening);
            let (owed, _, owed_prompt) = hand_over(&mut state, &mut activity, &mut output_audio);
            assert!(owed);
            let (mut gemini, server) = fake_resumed_socket().await;
            assert!(
                brief_replacement(
                    &mut gemini,
                    &mut state,
                    &mut activity,
                    replacement,
                    owed_prompt.as_deref()
                )
                .await
            );
            let _ = gemini.close().await;
            let sent = tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
            if cold || matches!(replacement, Replacement::Cold { .. }) {
                assert!(sent.get("realtimeInput").is_some());
            } else {
                assert_eq!(sent["clientContent"]["turnComplete"], true);
            }
            let text = sent_text(&sent);
            assert_eq!(text.matches(REACTION).count(), 1);
            assert_eq!(text.matches("BEGIN OWED EVENT").count(), 1);
            assert_eq!(text.matches("END OWED EVENT").count(), 1);
            assert_eq!(activity.prompt_text.as_deref(), Some(REACTION));
            assert!(activity.owes_reply());
        }
    }
}

#[test]
fn a_successful_event_prompt_owns_the_floor_without_rebasing_candidate_debt() {
    let start = Instant::now();
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    record_event_prompt(
        &mut state,
        &mut activity,
        "new event",
        None,
        Ok::<_, &str>(()),
        Instant::now(),
    )
    .unwrap();
    assert_eq!(activity.floor, Floor::Speaking);
    assert_eq!(activity.awaiting_reply_since, Some(start));
    assert_eq!(activity.prompt_text.as_deref(), Some("new event"));
    let mut idle = RuntimeActivity::new(start);
    assert!(
        record_event_prompt(
            &mut state,
            &mut idle,
            "failed event",
            None,
            Err("closed"),
            Instant::now()
        )
        .is_err()
    );
    assert!(!idle.owes_reply());
}

#[test]
fn successful_resumes_keep_raw_events_across_repeated_pauses() {
    let mut state = RuntimeState {
        paused: true,
        owed_reply_on_resume: Some(Some(REACTION.into())),
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    let (mut output_audio, _) = test_output_audio();
    for _ in 0..3 {
        let debt = ResumeDebt::capture(&state).unwrap();
        let resumed = crate::agent::apply_data_event(
            &mut state,
            crate::runtime::TOPIC_CONTROL,
            &serde_json::json!({"type": "pause_interview", "paused": false}),
            0.0,
        );
        let prompt = resumed.generate_reply.unwrap();
        assert_eq!(prompt.matches("BEGIN OWED EVENT").count(), 1);
        record_event_prompt(
            &mut state,
            &mut activity,
            &prompt,
            Some(&debt),
            Ok::<_, &str>(()),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(activity.prompt_text.as_deref(), Some(REACTION));
        crate::agent::apply_data_event(
            &mut state,
            crate::runtime::TOPIC_CONTROL,
            &serde_json::json!({"type": "pause_interview", "paused": true}),
            0.0,
        );
        update_pause_activity(&mut state, &mut activity, &mut output_audio, Instant::now());
        assert_eq!(state.owed_reply_on_resume, Some(Some(REACTION.into())));
    }
}

#[tokio::test]
async fn a_paused_replacement_keeps_the_known_event_after_delayed_transcription() {
    let start = Instant::now();
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    let (mut output_audio, _) = test_output_audio();
    activity.mark_prompted(start, Some(REACTION), false);
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({"type": "pause_interview", "paused": true}),
        0.0,
    );
    update_pause_activity(&mut state, &mut activity, &mut output_audio, start);
    assert_eq!(state.owed_reply_on_resume, Some(Some(REACTION.into())));
    activity.note_candidate_finished(start + Duration::from_secs(1), false);
    let (owed, _, owed_prompt) = hand_over(&mut state, &mut activity, &mut output_audio);
    assert!(owed);
    assert_eq!(owed_prompt, None);
    let (mut gemini, server) = fake_resumed_socket().await;
    assert!(
        !brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            Replacement::Resumed { owed },
            owed_prompt.as_deref()
        )
        .await
    );
    let _ = gemini.close().await;
    let sent = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent["clientContent"]["turnComplete"], false);
    assert_eq!(state.owed_reply_on_resume, Some(Some(REACTION.into())));
    let resumed = crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({"type": "pause_interview", "paused": false}),
        0.0,
    );
    let prompt = resumed.generate_reply.unwrap();
    assert_eq!(prompt.matches(REACTION).count(), 1);
    assert_eq!(prompt.matches("BEGIN OWED EVENT").count(), 1);
}

/// Issue 95's silent provider, through the decisions each watch tick makes and
/// the replacement steps the room loop takes, with a socket that completes
/// setup and then never answers. The room is shown the wait before the
/// watchdog fires, no nudge replaces the debt, every cold replacement is asked
/// for the reply the candidate is still owed, and the budget runs out on
/// schedule with a reason that asks nobody for a goodbye. What that ending
/// then does, `end_without_interviewer`, needs a room and is not reached here.
#[tokio::test]
async fn a_silent_provider_is_shown_and_recovered_until_its_budget_is_spent() {
    let mut state = tested_and_answered();
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let (mut output_audio, _frames) = test_output_audio();
    activity.note_candidate_finished(start, false);

    // Each tick asks what `on_watch_tick` asks, in its order: stalls first,
    // then the one decision that recovers, shows the wait, or lets a nudge run.
    let tick = Duration::from_secs_f64(crate::agent::WATCH_TICK_S);
    let mut at = start;
    let mut shown = None;
    // Bounded, so a watchdog that never fires fails here instead of hanging.
    let ticks = (REPLY_TIMEOUT.as_secs_f64() / crate::agent::WATCH_TICK_S).ceil() as usize + 1;
    let mut recovered = None;
    for _ in 0..ticks {
        at += tick;
        activity.settle_stalls(at, false);
        match reply_watch(&state, &activity, at, false) {
            ReplyWatch::Recover => {
                recovered = Some(at);
                break;
            }
            ReplyWatch::Owed { shown: true } => {
                shown.get_or_insert(at);
            }
            ReplyWatch::Owed { shown: false } => assert!(shown.is_none(), "shown stays shown"),
            ReplyWatch::Settled => panic!("a nudge would replace the owed reply"),
        }
    }
    let recovered = recovered.expect("the watchdog recovers within its timeout");
    let shown_at = shown.expect("the wait was shown");
    for (paused, end_requested) in [(true, false), (false, true)] {
        let quiet = RuntimeState {
            paused,
            end_requested,
            ..state.clone()
        };
        assert_eq!(
            reply_watch(&quiet, &activity, shown_at, false),
            ReplyWatch::Owed { shown: false },
            "no wait shown, and no nudge either"
        );
    }
    let shown = shown_at.duration_since(start);
    assert!(shown >= REPLY_WAIT_SHOWN && shown < REPLY_WAIT_SHOWN + tick);
    let recovered = recovered.duration_since(start);
    assert!(recovered >= REPLY_TIMEOUT && recovered < REPLY_TIMEOUT + tick);

    // A timeout never counts the socket it replaces as healthy, however long it
    // stayed connected.
    let mut restarts = 0;
    let long_lived = HEALTHY_GEMINI_SOCKET * 2;
    for attempt in 0..GEMINI_RESTART_LIMIT {
        assert!(take_restart_attempt(
            &mut restarts,
            replaced_socket_age(long_lived, true, &activity)
        ));
        let (owed, _, owed_prompt) = hand_over(&mut state, &mut activity, &mut output_audio);
        assert!(owed, "{attempt}");
        let (mut gemini, server) = fake_socket(None).await;
        assert!(
            brief_replacement(
                &mut gemini,
                &mut state,
                &mut activity,
                Replacement::Cold { owed },
                owed_prompt.as_deref(),
            )
            .await
        );
        let _ = gemini.close().await;
        let sent = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        let text = sent_text(&sent);
        assert_eq!(
            text.matches("was lost with the connection").count(),
            1,
            "{attempt}: {text}"
        );
        assert!(
            !text.contains("BEGIN OWED EVENT"),
            "a candidate turn names no event"
        );

        // The replacement is as silent as the socket before it, and its own
        // deadline runs from the briefing.
        let briefed = activity.prompted_at.unwrap();
        let just_short = briefed + REPLY_TIMEOUT - Duration::from_millis(1);
        assert_ne!(
            reply_watch(&state, &activity, just_short, false),
            ReplyWatch::Recover
        );
        assert_eq!(
            reply_watch(&state, &activity, briefed + REPLY_TIMEOUT, false),
            ReplyWatch::Recover
        );
    }
    assert!(!take_restart_attempt(
        &mut restarts,
        replaced_socket_age(long_lived, true, &activity)
    ));
    assert!(!should_send_wrap_up(INTERVIEWER_UNAVAILABLE));
}

/// A candidate turn transcribed after the pause began is owed on a cold
/// socket too. The briefing cannot be sent while paused, so the unpause has to
/// ask for the reply, with no event to name since the candidate's turn is it.
#[tokio::test]
async fn a_paused_cold_replacement_keeps_an_unanswered_candidate_turn() {
    let start = Instant::now();
    let mut state = RuntimeState {
        paused: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    let (mut output_audio, _) = test_output_audio();
    activity.note_candidate_finished(start, false);
    let (owed, _, owed_prompt) = hand_over(&mut state, &mut activity, &mut output_audio);
    assert!(owed);
    assert_eq!(owed_prompt, None);
    let (mut gemini, server) = fake_socket(None).await;
    assert!(
        !brief_replacement(
            &mut gemini,
            &mut state,
            &mut activity,
            Replacement::Cold { owed },
            None,
        )
        .await
    );
    let _ = gemini.close().await;
    server.abort();
    assert!(state.needs_cold_brief);
    assert_eq!(state.owed_reply_on_resume, Some(None));
    let resumed = crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({"type": "pause_interview", "paused": false}),
        0.0,
    );
    let prompt = resumed.generate_reply.unwrap();
    assert_eq!(prompt.matches("was lost with the connection").count(), 1);
    assert!(resumed.carries_thinking_debt);
    state.clear_thinking_debt();
    assert!(!state.needs_cold_brief);
}

/// A cold briefing that fails to send leaves an unanswered candidate turn owed
/// on the next socket, with no event text to attach to it.
#[test]
fn a_failed_cold_briefing_keeps_an_unanswered_candidate_turn() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    keep_recovery_debt(
        &mut state,
        &mut activity,
        Replacement::Cold { owed: true },
        None,
    );
    assert!(state.needs_cold_brief);
    assert!(activity.owes_prompt());
    assert_eq!(activity.prompt_text, None);
}

/// A pause's discard and the interruption guard can wait on the same old
/// turn. Its one `TurnComplete` ends both; swallowing it on the guard alone
/// left the discard armed to drop the next reply the candidate was owed.
#[test]
fn an_interrupted_turn_ending_inside_a_discard_ends_both() {
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.discarding_output = true;
    activity.interrupted_turn_pending = true;
    assert!(!accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        Instant::now()
    ));
    assert!(!activity.discarding_output);
    assert!(!activity.interrupted_turn_pending);
    assert!(accept_gemini_event(
        &audio_chunk(),
        &mut activity,
        false,
        Instant::now()
    ));
}

/// Only a completed turn opens the late-transcript grace. An interruption
/// inside it is the candidate barging in, so what they say next is a new turn
/// and is owed a reply.
#[test]
fn only_a_completed_turn_opens_the_late_transcript_grace() {
    let mut activity = RuntimeActivity::new(Instant::now());
    let audio = audio_chunk();
    assert!(accept_gemini_event(
        &audio,
        &mut activity,
        false,
        Instant::now()
    ));
    assert!(accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        Instant::now()
    ));
    let completed = activity.turn_completed_at.expect("a completed turn stamps");
    activity.note_candidate_finished(completed, false);
    assert!(!activity.reply_in_flight(), "the answered turn's tail");

    assert!(accept_gemini_event(
        &audio,
        &mut activity,
        false,
        Instant::now()
    ));
    assert!(accept_gemini_event(
        &GeminiEvent::Interrupted,
        &mut activity,
        false,
        Instant::now()
    ));
    assert_eq!(activity.turn_completed_at, None);
    activity.note_candidate_finished(completed, false);
    assert!(activity.reply_in_flight(), "the barge-in is a new turn");
}

/// What `handle_data_packet` does with the pause packet the page sends: the
/// pause flips, the activity follows it, and an unpause sends the resume
/// prompt, which is returned.
fn toggle_pause(
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    output_audio: &mut OutputAudio,
    now: Instant,
) -> Option<String> {
    let resume_debt = ResumeDebt::capture(state);
    let result = crate::agent::apply_data_event(
        state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({"type": "pause_interview", "paused": !state.paused}),
        0.0,
    );
    update_pause_activity(state, activity, output_audio, now);
    let prompt = result.generate_reply?;
    if result.carries_thinking_debt {
        state.clear_thinking_debt();
    }
    record_event_prompt(
        state,
        activity,
        &prompt,
        resume_debt
            .as_ref()
            .filter(|_| result.pause_changed == Some(false)),
        Ok::<_, &str>(()),
        now,
    )
    .unwrap();
    Some(prompt)
}

/// One thing the room loop does to the reply the interviewer owes, applied
/// through the production function the loop calls for it, with no room or
/// socket in the way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DebtStep {
    /// A candidate transcript fragment.
    Candidate,
    /// A data event that asks for a reply, such as a test reaction.
    Prompt,
    /// An editor review, which silence may answer, sent only when the watch
    /// tick's own decision lets a nudge run.
    Review,
    /// An audible chunk of a reply.
    Audio,
    /// A tool call, answered at once.
    ToolCall,
    TurnComplete,
    /// Gemini cutting its own turn for a barge-in.
    Interrupted,
    /// The candidate pausing, or unpausing, which sends the resume prompt.
    Pause,
    /// The socket resumed and the briefing lost with it, which leaves the
    /// debt for the next socket.
    Replace,
    /// The same, on a cold socket that has to be briefed from scratch.
    ReplaceCold,
    /// A watch tick settling stalls, `PROMPT_STALL` after the step before, so
    /// what stalls does.
    Tick,
}

const DEBT_STEPS: [DebtStep; 11] = [
    DebtStep::Candidate,
    DebtStep::Prompt,
    DebtStep::Review,
    DebtStep::Audio,
    DebtStep::ToolCall,
    DebtStep::TurnComplete,
    DebtStep::Interrupted,
    DebtStep::Pause,
    DebtStep::Replace,
    DebtStep::ReplaceCold,
    DebtStep::Tick,
];

/// Whether the candidate is still owed something, wherever the debt is held:
/// on the activity, owed or under way, or across a pause for the unpause to
/// ask for. A reply under way is debt too: a replacement is briefed to finish
/// it, and the watchdog's progress deadline covers it.
fn debt_held(state: &RuntimeState, activity: &RuntimeActivity) -> bool {
    activity.reply_unfinished()
        || (state.paused && (state.owed_reply_on_resume.is_some() || state.needs_cold_brief))
}

/// Applies one step and says whether it was one of the few events allowed to
/// settle what was owed: output that answers it, a completed turn (which may
/// be a deliberate silence), or a barge-in that ends the turn.
fn apply_debt_step(
    step: DebtStep,
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    output_audio: &mut OutputAudio,
    now: Instant,
) -> bool {
    match step {
        DebtStep::Candidate => {
            activity.note_candidate_finished(now, false);
            false
        }
        DebtStep::Prompt => {
            activity.mark_prompted(now, Some(REACTION), false);
            false
        }
        DebtStep::Review => {
            if reply_watch(state, activity, now, false) == ReplyWatch::Settled && !state.paused {
                activity.mark_prompted(now, Some("editor review"), true);
            }
            false
        }
        DebtStep::Audio => {
            let delivered = accept_gemini_event(&audio_chunk(), activity, state.paused, now);
            if delivered {
                activity.note_reply_audible();
            }
            delivered
        }
        DebtStep::ToolCall => {
            let delivered = accept_gemini_event(
                &GeminiEvent::ToolCall(Vec::new()),
                activity,
                state.paused,
                now,
            );
            if delivered {
                activity.note_tool_response(now);
            }
            delivered
        }
        DebtStep::TurnComplete => {
            let delivered =
                accept_gemini_event(&GeminiEvent::TurnComplete, activity, state.paused, now);
            if delivered {
                activity.settle_completed_turn(false);
            }
            delivered
        }
        DebtStep::Interrupted => {
            let delivered =
                accept_gemini_event(&GeminiEvent::Interrupted, activity, state.paused, now);
            if delivered {
                state.end_requested = false;
                let spoken = ["Candidate: wait, one thing".to_string()];
                assert!(
                    session::cut_unless_protected(
                        &spoken,
                        Interruptible::Yes,
                        activity,
                        output_audio,
                    )
                    .is_some()
                );
            }
            delivered
        }
        DebtStep::Pause => {
            toggle_pause(state, activity, output_audio, now);
            false
        }
        DebtStep::Replace | DebtStep::ReplaceCold => {
            let (owed, _, owed_prompt) = hand_over(state, activity, output_audio);
            let replacement = if step == DebtStep::Replace {
                Replacement::Resumed { owed }
            } else {
                Replacement::Cold { owed }
            };
            keep_recovery_debt(state, activity, replacement, owed_prompt.as_deref());
            false
        }
        DebtStep::Tick => {
            activity.settle_stalls(now, false);
            false
        }
    }
}

/// The reply debt is spread over a dozen fields that ten functions write, and
/// the bugs issue 95's reviews found were each two of those writers
/// disagreeing, never one of them alone. An enum cannot hold it, because a
/// candidate turn, a prompt and a tool continuation can be owed at once. So the
/// rules every writer has to keep are checked instead, after every step of
/// every sequence of up to five. Steps are a second apart, so the transcript
/// grace is crossed both ways, and a tick comes a stall later:
///
/// - Owed means recoverable: a reply owed or a generation under way always
///   has a deadline the watchdog can reach, and nothing else does. This is the
///   silence the issue reported.
/// - Nothing loses a debt except output that answers it, a completed turn, or
///   a barge-in. A pause moves it to the unpause, a replacement to the next
///   socket, a stall tick into a prompt.
/// - A `TurnComplete` never leaves a discard armed behind it.
/// - Output delivered outside a pause always disarms the interruption guard.
#[test]
fn every_short_sequence_keeps_the_reply_debt_rules() {
    let (mut output_audio, _frames) = test_output_audio();
    let far = Duration::from_secs(3600);
    let mut sequence = [DebtStep::Tick; 5];
    let mut checked = 0usize;
    for index in 0..DEBT_STEPS.len().pow(sequence.len() as u32) {
        let mut rest = index;
        for slot in &mut sequence {
            *slot = DEBT_STEPS[rest % DEBT_STEPS.len()];
            rest /= DEBT_STEPS.len();
        }
        let start = Instant::now();
        let mut state = RuntimeState::default();
        let mut activity = RuntimeActivity::new(start);
        let mut now = start;
        for (offset, &step) in sequence.iter().enumerate() {
            now += if step == DebtStep::Tick {
                PROMPT_STALL
            } else {
                Duration::from_secs(1)
            };
            let owed_before = debt_held(&state, &activity);
            let discarding_before = activity.discarding_output;
            let settled = apply_debt_step(step, &mut state, &mut activity, &mut output_audio, now);
            let context = || format!("{:?} at step {offset}", &sequence[..=offset]);
            if owed_before && !settled {
                assert!(debt_held(&state, &activity), "debt lost: {}", context());
            }
            if step == DebtStep::TurnComplete {
                assert!(
                    !activity.discarding_output,
                    "discard left armed: {}",
                    context()
                );
            }

            // Paused, nothing reaches Gemini to start a newer generation, so
            // output then is the old one's and rightly leaves the guard armed.
            if settled
                && matches!(step, DebtStep::Audio | DebtStep::ToolCall)
                && !discarding_before
                && !state.paused
            {
                assert!(
                    !activity.interrupted_turn_pending,
                    "guard left armed: {}",
                    context()
                );
            }
            if !state.paused {
                assert_eq!(
                    activity.reply_timed_out(now + far, false),
                    activity.owes_reply() || activity.generating,
                    "owed without a deadline, or a deadline owing nothing: {}",
                    context()
                );
            }
            checked += 1;
        }
    }
    assert_eq!(
        checked,
        5 * DEBT_STEPS.len().pow(5),
        "every step was checked"
    );
}

/// A pause while a generation is stalled, which Gemini never ends with a
/// `TurnComplete`. The resume prompt gets no `Interrupted` for a generation
/// the server is no longer running, so a discard armed on it had nothing to
/// end it but the answer's own completion, and that answer was dropped whole
/// until the watchdog asked again. A stalled generation arms none, and the
/// answer to the resume prompt is heard.
#[test]
fn the_answer_to_a_resume_after_a_stalled_generation_is_heard() {
    let start = Instant::now();
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    let (mut output_audio, _) = test_output_audio();

    // Audible, as a stalled reply is: the chunk took the floor, and without a
    // `TurnComplete` nothing hands it back.
    let chunk = audio_chunk();
    assert!(accept_gemini_event(&chunk, &mut activity, false, start));
    activity.note_reply_audible();
    assert_eq!(activity.floor, Floor::Speaking);
    let paused_at = start + PROMPT_STALL;

    assert!(toggle_pause(&mut state, &mut activity, &mut output_audio, paused_at).is_none());
    assert!(!activity.discarding_output, "nothing stalled is on its way");
    let resumed_at = paused_at + Duration::from_secs(5);
    toggle_pause(&mut state, &mut activity, &mut output_audio, resumed_at).unwrap();
    assert!(activity.owes_prompt());

    let answer = audio_chunk();
    let answered_at = resumed_at + Duration::from_secs(1);
    assert!(accept_gemini_event(
        &answer,
        &mut activity,
        false,
        answered_at
    ));
    assert!(!activity.owes_prompt(), "the resume prompt is answered");

    // A generation still producing at the pause keeps its discard, and the
    // interruption the resume prompt causes is what ends it.
    let mut live = RuntimeActivity::new(start);
    live.note_output(start);
    let mut paused = RuntimeState {
        paused: true,
        ..RuntimeState::default()
    };
    update_pause_activity(&mut paused, &mut live, &mut output_audio, start);
    assert!(live.discarding_output);
    assert!(!accept_gemini_event(
        &GeminiEvent::Interrupted,
        &mut live,
        false,
        start
    ));
    assert!(!live.discarding_output);
}

/// Jim calls `end_interview` and Gemini never sends the acknowledgement. The
/// stall releases the hold, and the watch tick has to close then: the reply
/// watch stays out of a requested close and holds the nudges back, and a
/// silent socket sends no event for the check after each one to see it.
#[test]
fn a_close_whose_acknowledgement_never_comes_is_ready_after_the_stall() {
    let start = Instant::now();
    let mut state = RuntimeState {
        end_requested: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    activity.note_tool_response(start);
    assert!(
        !ready_to_close(&state, &activity),
        "the acknowledgement is owed"
    );
    activity.settle_stalls(start + PROMPT_STALL, false);
    assert!(ready_to_close(&state, &activity));
    assert_ne!(
        reply_watch(&state, &activity, start + PROMPT_STALL, false),
        ReplyWatch::Settled,
        "no nudge would provoke the event that used to close it"
    );
    state.paused = true;
    assert!(!ready_to_close(&state, &activity), "not into a paused room");

    // An acknowledgement that starts to speak and then stops is released the
    // same way, once what it said has played.
    state.paused = false;
    let mut spoke = RuntimeActivity::new(start);
    spoke.note_tool_response(start);
    spoke.note_output(start);
    assert!(!ready_to_close(&state, &spoke));
    spoke.settle_stalls(start + PROMPT_STALL, true);
    assert!(!ready_to_close(&state, &spoke), "its audio still plays");
    spoke.settle_stalls(start + PROMPT_STALL, false);
    assert!(ready_to_close(&state, &spoke));
}

/// A close is held across a pause rather than generated into a room whose
/// output is dropped, and acted on once the candidate is back: the rule
/// `ready_to_close` states. A close from the generation the pause cut off is
/// held the same way, since that decision was made before the pause too.
#[test]
fn a_close_from_a_discarded_generation_is_held_until_the_resume() {
    let start = Instant::now();
    let mut state = RuntimeState {
        paused: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    activity.discarding_output = true;
    assert!(accept_gemini_event(
        &GeminiEvent::ToolCall(Vec::new()),
        &mut activity,
        true,
        start
    ));

    // What `execute_tool_call` records for an accepted `end_interview`, and the
    // response that goes out for it.
    state.end_requested = true;
    activity.note_tool_response(start);
    assert!(
        !activity.tool_response_outstanding,
        "its acknowledgement is discarded"
    );
    assert!(!ready_to_close(&state, &activity), "held while paused");
    state.paused = false;
    assert!(
        ready_to_close(&state, &activity),
        "acted on after the resume"
    );
}

/// A pause cuts off a generation that stalled, so no discard is armed, and
/// the generation later ends after all. Its late `Interrupted` and
/// `TurnComplete` belong to the old turn and must not settle the resume
/// prompt, or the watchdog would have nothing left to recover if the resume
/// is not answered.
#[test]
fn a_stalled_generations_late_ending_does_not_settle_the_resume() {
    let start = Instant::now();
    let mut state = RuntimeState {
        paused: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    let (mut output_audio, _) = test_output_audio();
    activity.note_output(start);
    let paused_at = start + PROMPT_STALL;
    update_pause_activity(&mut state, &mut activity, &mut output_audio, paused_at);
    assert!(!activity.discarding_output);
    assert!(activity.stale_turn_pending);

    state.paused = false;
    let resumed_at = paused_at + Duration::from_secs(5);
    activity.mark_prompted(resumed_at, Some("resume"), false);
    for ending in [GeminiEvent::Interrupted, GeminiEvent::TurnComplete] {
        assert!(!accept_gemini_event(
            &ending,
            &mut activity,
            false,
            resumed_at
        ));
        assert!(activity.owes_prompt(), "{ending:?} is the old turn's");
    }
    assert!(!activity.stale_turn_pending);
    assert!(activity.reply_timed_out(resumed_at + REPLY_TIMEOUT, false));

    // New output disarms it, so the resume's own ending counts.
    activity.stale_turn_pending = true;
    let chunk = audio_chunk();
    assert!(accept_gemini_event(
        &chunk,
        &mut activity,
        false,
        resumed_at
    ));
    assert!(!activity.stale_turn_pending);
    assert!(accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        resumed_at
    ));
}

/// A socket replaced while it still owed a reply is not counted healthy for
/// having lived past a minute, whoever replaced it.
#[test]
fn a_socket_replaced_while_owing_a_reply_counts_as_unanswered() {
    let long_lived = HEALTHY_GEMINI_SOCKET * 2;
    let mut activity = RuntimeActivity::new(Instant::now());
    assert_eq!(
        replaced_socket_age(long_lived, false, &activity),
        long_lived
    );
    assert_eq!(
        replaced_socket_age(long_lived, true, &activity),
        Duration::ZERO
    );
    activity.note_candidate_finished(Instant::now(), false);
    assert_eq!(
        replaced_socket_age(long_lived, false, &activity),
        Duration::ZERO
    );

    let mut restarts = GEMINI_RESTART_LIMIT;
    assert!(!take_restart_attempt(
        &mut restarts,
        replaced_socket_age(long_lived, false, &activity)
    ));
}

/// The operator's limits reach the activity the room loop runs on.
#[test]
fn the_room_loop_runs_on_the_configured_limits() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google-key"),
        ("CODETRIAL_GEMINI_REPLY_TIMEOUT_S", "70"),
        ("CODETRIAL_MAX_INTERIM_REVIEWS", "3"),
    ])
    .unwrap();
    let activity = runtime_activity(&config, Instant::now(), 7);
    assert_eq!(activity.reply_timeout, Duration::from_secs(70));
    assert_eq!(activity.max_interim_reviews, 3);
}

/// A turn Gemini completes with nothing in it opens no grace: the candidate
/// paused on "um," and what they say next is the answer still owed a reply.
#[test]
fn a_silent_completion_opens_no_late_transcript_grace() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    assert!(accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        start
    ));
    assert_eq!(activity.turn_completed_at, None);
    activity.note_candidate_finished(start + Duration::from_millis(500), false);
    assert!(activity.reply_in_flight());
}

/// A reply Gemini started and never finished is still unfinished business:
/// the nudges stay out of it, a socket that drops partway through it does not
/// count as healthy, and once its audio has drained with nothing more coming
/// the room shows the interviewer working on it instead of still speaking.
#[test]
fn a_reply_left_partway_is_owed_everywhere_it_is_read() {
    let start = Instant::now();
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    let chunk = audio_chunk();
    activity.note_candidate_finished(start, false);
    assert!(accept_gemini_event(&chunk, &mut activity, false, start));
    activity.note_reply_audible();
    assert!(!activity.owes_reply(), "the reply started");

    assert!(activity.reply_unfinished());
    assert_eq!(
        replaced_socket_age(HEALTHY_GEMINI_SOCKET, false, &activity),
        Duration::ZERO
    );
    let shown = start + REPLY_WAIT_SHOWN;
    assert_eq!(
        reply_watch(&state, &activity, shown - Duration::from_millis(1), false),
        ReplyWatch::Owed { shown: false }
    );
    assert_eq!(
        reply_watch(&state, &activity, shown, true),
        ReplyWatch::Owed { shown: false },
        "the audio it did produce is still playing"
    );
    assert_eq!(
        reply_watch(&state, &activity, shown, false),
        ReplyWatch::Owed { shown: true }
    );

    assert!(accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        shown
    ));
    assert!(!activity.reply_unfinished());
    assert_eq!(
        reply_watch(&state, &activity, shown, false),
        ReplyWatch::Settled
    );
}

/// The wrap-up goes out behind a close acknowledgement that stalled and was
/// released. That turn's late `TurnComplete` settles the floor, and the wait
/// has to go on until the goodbye itself is said and played.
#[test]
fn a_late_acknowledgement_ending_does_not_cut_the_goodbye() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_tool_response(start);
    activity.note_output(start);
    activity.settle_stalls(start + PROMPT_STALL, false);
    assert!(
        !activity.tool_response_outstanding,
        "the stalled ack is released"
    );

    let wrap_up_at = start + PROMPT_STALL;
    activity.mark_prompted(wrap_up_at, Some("goodbye"), false);
    assert!(activity.prompt_behind_turn);
    assert!(accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        wrap_up_at
    ));
    activity.settle_completed_turn(false);
    assert!(
        !session::goodbye_heard(&activity, false),
        "the goodbye is still owed"
    );

    let chunk = audio_chunk();
    assert!(accept_gemini_event(
        &chunk,
        &mut activity,
        false,
        wrap_up_at
    ));
    activity.note_reply_audible();
    assert!(accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        wrap_up_at
    ));
    activity.settle_completed_turn(true);
    assert!(!session::goodbye_heard(&activity, true), "still playing");
    assert!(session::goodbye_heard(&activity, false));
}

/// The candidate keeping the floor to think is the silence they asked for:
/// replies are dropped on purpose, so an owed one is neither recovered nor
/// shown as the interviewer thinking until the hold ends.
#[test]
fn a_thinking_hold_is_not_an_interviewer_stall() {
    let start = Instant::now();
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held { since: start },
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    let late = start + REPLY_TIMEOUT * 2;
    assert_eq!(
        reply_watch(&state, &activity, late, false),
        ReplyWatch::Owed { shown: false }
    );
    state.thinking_hold = ThinkingHold::Off;
    assert_eq!(
        reply_watch(&state, &activity, late, false),
        ReplyWatch::Recover
    );
}

/// A completed reply the room loop counts as output, ended by `event`.
fn reply_event(
    recovery: &mut ResumeRecovery,
    state: &mut RuntimeState,
    restarts: &mut usize,
    event: &GeminiEvent,
) {
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.note_output(Instant::now());
    recovery.note_event(restarts, state, &activity, event);
}

fn complete_reply(recovery: &mut ResumeRecovery, state: &mut RuntimeState, restarts: &mut usize) {
    reply_event(recovery, state, restarts, &GeminiEvent::TurnComplete);
}

const YOUNG: Duration = Duration::from_secs(12);

#[test]
fn failed_resumption_rebuilds_until_recovery() {
    let mut recovery = ResumeRecovery::default();
    let mut state = RuntimeState::default();
    let mut restarts = 0;
    assert!(recovery.allow_resume(Duration::ZERO));
    recovery.connected(true);
    assert!(!recovery.allow_resume(YOUNG));
    recovery.connected(false);
    assert!(!recovery.allow_resume(YOUNG));
    recovery.connected(false);
    complete_reply(&mut recovery, &mut state, &mut restarts);
    assert!(recovery.allow_resume(YOUNG));
}

#[test]
fn healthy_socket_lifts_suppression() {
    let mut recovery = ResumeRecovery::default();
    recovery.connected(true);
    assert!(!recovery.allow_resume(YOUNG));
    recovery.connected(false);
    assert!(recovery.allow_resume(HEALTHY_GEMINI_SOCKET));
    recovery.connected(true);
    assert!(recovery.allow_resume(HEALTHY_GEMINI_SOCKET));
}

#[test]
fn resumed_socket_that_answers_then_dies_young_still_rebuilds() {
    let mut recovery = ResumeRecovery::default();
    let mut restarts = 0;
    recovery.connected(true);
    complete_reply(&mut recovery, &mut RuntimeState::default(), &mut restarts);
    assert!(!recovery.allow_resume(YOUNG));
}

#[test]
fn refused_resumption_does_not_suppress_the_next_one() {
    let mut recovery = ResumeRecovery::default();
    assert!(recovery.allow_resume(Duration::ZERO));
    recovery.connected(false);
    assert!(recovery.allow_resume(YOUNG));
}

#[test]
fn sockets_that_only_answer_their_briefing_still_end_the_interview() {
    let mut recovery = ResumeRecovery::default();
    let mut state = RuntimeState::default();
    let mut restarts = 0;
    let mut attempts = 0;
    while take_restart_attempt(&mut restarts, YOUNG) {
        let resumed = recovery.allow_resume(YOUNG);
        recovery.connected(resumed);
        state.recovery_reply_pending = true;
        complete_reply(&mut recovery, &mut state, &mut restarts);
        attempts += 1;
        assert!(attempts <= GEMINI_RESTART_LIMIT);
    }
    assert_eq!(attempts, GEMINI_RESTART_LIMIT);
}

#[test]
fn only_output_beyond_the_briefing_proves_recovery() {
    let mut recovery = ResumeRecovery::default();
    let mut state = RuntimeState::default();
    let mut restarts = GEMINI_RESTART_LIMIT;
    recovery.connected(true);
    assert!(!recovery.allow_resume(YOUNG));
    recovery.connected(false);
    state.recovery_reply_pending = true;

    // A tool call in the briefing's answer ends nothing; the continuation's
    // completion is still that answer, whatever cause the tool response gave
    // it.
    reply_event(
        &mut recovery,
        &mut state,
        &mut restarts,
        &GeminiEvent::ToolCall(Vec::new()),
    );
    complete_reply(&mut recovery, &mut state, &mut restarts);
    assert_eq!(restarts, GEMINI_RESTART_LIMIT);
    assert!(recovery.suppressed);

    complete_reply(&mut recovery, &mut state, &mut restarts);
    assert_eq!(restarts, 0);
    assert!(!recovery.suppressed);
}

#[test]
fn a_silent_or_interrupted_briefing_does_not_swallow_the_next_reply() {
    for ending in [GeminiEvent::TurnComplete, GeminiEvent::Interrupted] {
        let mut recovery = ResumeRecovery::default();
        let mut state = RuntimeState {
            recovery_reply_pending: true,
            ..RuntimeState::default()
        };
        let mut restarts = GEMINI_RESTART_LIMIT;
        recovery.note_event(
            &mut restarts,
            &mut state,
            &RuntimeActivity::new(Instant::now()),
            &ending,
        );
        assert_eq!(restarts, GEMINI_RESTART_LIMIT);

        complete_reply(&mut recovery, &mut state, &mut restarts);
        assert_eq!(restarts, 0);
    }
}

#[test]
fn a_briefing_held_for_an_unpause_or_a_hold_is_still_a_briefing() {
    for held in [
        RuntimeState {
            needs_cold_brief: true,
            ..RuntimeState::default()
        },
        RuntimeState {
            owed_reply_on_resume: Some(None),
            ..RuntimeState::default()
        },
    ] {
        let mut state = held;
        let mut recovery = ResumeRecovery::default();
        let mut restarts = GEMINI_RESTART_LIMIT;

        // What the unpause and the end of a hold call once the held debt has
        // gone out with the reply it asks for.
        state.clear_thinking_debt();
        complete_reply(&mut recovery, &mut state, &mut restarts);
        assert_eq!(restarts, GEMINI_RESTART_LIMIT);
        complete_reply(&mut recovery, &mut state, &mut restarts);
        assert_eq!(restarts, 0);
    }

    let mut state = RuntimeState {
        thinking_unheard_reply: true,
        ..RuntimeState::default()
    };
    state.clear_thinking_debt();
    assert!(!state.recovery_reply_pending);
}

#[test]
fn a_replacement_drops_the_briefing_its_predecessor_owed() {
    let mut state = RuntimeState {
        recovery_reply_pending: true,
        ..RuntimeState::default()
    };
    clear_abandoned_socket_work(&mut state, &mut RuntimeActivity::new(Instant::now()));
    assert!(!state.recovery_reply_pending);
}

#[test]
fn reconnect_wait_uses_machine_reason_and_rounds_up() {
    assert_eq!(
        reconnect_wait("quota", Some(Duration::from_millis(42001))),
        serde_json::json!({
            "type": "interviewer_state", "reconnecting": true, "reason": "quota", "waitSeconds": 43
        })
    );
    assert_eq!(
        reconnect_wait("retrying", Some(Duration::from_secs(2)))["waitSeconds"],
        2
    );
    assert_eq!(
        interviewer_state(false),
        serde_json::json!({ "type": "interviewer_state", "reconnecting": false })
    );
    assert_eq!(
        reconnect_wait("retrying", None),
        serde_json::json!({ "type": "interviewer_state", "reconnecting": true, "reason": "retrying" })
    );
}

/// Only the end waits for the phase judge in the room loop, since waiting
/// there stops the candidate's audio reaching the interviewer. A round
/// transition that a judgment may still change is held instead, and nothing
/// else waits: holding every control packet would stall the room.
#[test]
fn only_the_end_waits_for_the_phase_judge_and_a_round_transition_is_held() {
    let control = |kind: &str| serde_json::json!({"type": kind, "round": "behavioral"});
    let mut state = RuntimeState::default();
    state.started_at -= Duration::from_secs(u64::from(state.coding_minutes) * 60);
    assert!(settles_phase_judge(
        &state,
        TOPIC_CONTROL,
        &control("end_interview")
    ));
    for other in [
        "round_transition",
        "thinking",
        "pause_interview",
        "time_warning",
    ] {
        assert!(
            !settles_phase_judge(&state, TOPIC_CONTROL, &control(other)),
            "{other}"
        );
    }
    assert!(!settles_phase_judge(
        &state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &control("end_interview")
    ));

    // Held only while a judgment is in flight or due.
    let transition = control("round_transition");
    assert!(!defers_round_transition(
        &RuntimeState::default(),
        true,
        TOPIC_CONTROL,
        &transition
    ));
    assert!(!defers_round_transition(
        &state,
        true,
        TOPIC_CONTROL,
        &serde_json::json!({"type": "round_transition"})
    ));
    assert!(!defers_round_transition(
        &state,
        false,
        TOPIC_CONTROL,
        &transition
    ));
    assert!(defers_round_transition(
        &state,
        true,
        TOPIC_CONTROL,
        &transition
    ));
    state
        .transcript
        .push("Candidate: it runs in linear time overall".to_string());
    assert!(defers_round_transition(
        &state,
        false,
        TOPIC_CONTROL,
        &transition
    ));
    assert!(!defers_round_transition(
        &state,
        true,
        TOPIC_CONTROL,
        &control("end_interview")
    ));
    state.round_transition_seen = true;
    assert!(!defers_round_transition(
        &state,
        true,
        TOPIC_CONTROL,
        &transition
    ));
    state.round_transition_seen = false;
    state.ended = true;
    assert!(!settles_phase_judge(
        &state,
        TOPIC_CONTROL,
        &control("end_interview")
    ));
    assert!(!defers_round_transition(
        &state,
        true,
        TOPIC_CONTROL,
        &transition
    ));
}

/// A held transition is applied once nothing the candidate said is waiting
/// for a judgment, or when the hold runs out. A judgment that finished on an
/// older snapshot does not release it: the newer words get a judgment of their
/// own while the hold lasts. Never while paused, since the transition is
/// refused during a pause and the page does not send it again.
#[tokio::test]
async fn a_held_round_transition_waits_for_its_judgment_or_its_deadline() {
    let now = Instant::now();
    let late = now + PHASE_JUDGE_SETTLE;
    let quiet = RuntimeState::default();
    let spoke = RuntimeState {
        transcript: vec!["Candidate: it runs in linear time overall".to_string()],
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(now);
    assert!(
        !activity.parked_transition_ready(&quiet, now),
        "nothing held"
    );
    assert!(
        !activity.held_transition_needs_judgment(&spoke, now),
        "nothing held"
    );
    activity.pending_round_transition = Some(turn::ParkedPacket {
        payload: serde_json::json!({"type": "round_transition"}),
        received: true,
        deadline: late,
    });
    assert!(
        activity.parked_transition_ready(&quiet, now),
        "nothing to judge"
    );

    // Words since the last snapshot: held, and a judgment is asked for.
    assert!(!activity.parked_transition_ready(&spoke, now));
    assert!(activity.held_transition_needs_judgment(&spoke, now));
    assert!(!activity.held_transition_needs_judgment(&quiet, now));
    assert!(
        !activity.held_transition_needs_judgment(&spoke, late),
        "no time left"
    );

    // A judgment out holds it, whatever is due, until the deadline.
    activity
        .phase_judge
        .start(tokio::spawn(std::future::pending::<String>()));
    assert!(!activity.parked_transition_ready(&quiet, now));
    assert!(
        !activity.held_transition_needs_judgment(&spoke, now),
        "one at a time"
    );
    assert!(activity.parked_transition_ready(&spoke, late));

    // Paused: kept past its deadline, and released on the first tick after.
    let paused = RuntimeState {
        paused: true,
        ..RuntimeState::default()
    };
    assert!(!activity.parked_transition_ready(&paused, late));
    assert!(activity.parked_transition_ready(&quiet, late));
}

/// The room-loop wiring the pure checks above cannot see: the end settles the
/// judge before the packet is applied, a held transition returns before it is,
/// and the watch tick applies a held transition after collecting the judgment.
#[test]
fn the_room_loop_settles_the_end_and_holds_the_transition_before_applying_them() {
    let source = include_str!("../../src/livekit.rs");
    let body = |name: &str| {
        source
            .split(name)
            .nth(1)
            .unwrap_or_else(|| panic!("{name} is still defined here"))
            .split("\n}\n")
            .next()
            .unwrap_or_default()
    };
    let packet = body("async fn handle_data_packet(");
    let settle = packet
        .find("settle_phase_judge(context")
        .expect("the end settles");
    let hold = packet
        .find("pending_round_transition")
        .expect("a transition is held");
    let apply = packet
        .find("apply_data_packet(")
        .expect("packets are applied");
    assert!(settle < apply && hold < apply);
    assert!(packet[hold..apply].contains("return Ok(ControlFlow::Continue(()))"));
    assert!(packet[..hold].contains("reserve_phase_judge("));
    assert!(body("async fn settle_phase_judge(").contains("reserve_phase_judge("));

    let tick = body("async fn on_watch_tick(");
    let collect = tick
        .find("phase_judge.finished()")
        .expect("the tick collects");
    let release = tick
        .find("parked_transition_ready(")
        .expect("the tick releases");
    let publish = tick
        .find("flush_framework_progress(")
        .expect("the tick publishes what it collected");
    assert!(collect < release && collect < publish);
    assert!(tick[release..publish].contains("apply_data_packet("));
    assert!(!tick[release..publish].contains("handle_data_packet("));
    let application = body("async fn apply_data_packet(");
    assert!(application.contains("apply_data_event_at("));
    assert!(!application.contains("defers_round_transition("));
    assert!(!application.contains("pending_round_transition"));

    // Not on every turn of the room loop, which runs for every audio frame.
    let room_loop = body("tokio::select! {");
    assert!(!room_loop.contains("flush_framework_progress("));
}

/// A side call's slot hands its handle over once, running or finished, and is
/// empty after: the end waits on what it took, and nothing else can.
#[tokio::test]
async fn a_side_call_hands_over_its_handle_once() {
    let mut slot = SideCall::default();
    assert!(slot.take().is_none());
    slot.start(tokio::spawn(async { "done".to_string() }));
    let handle = slot.take().expect("the started call");
    assert!(!slot.is_running());
    assert!(slot.take().is_none());
    assert_eq!(handle.await.unwrap(), "done");
}
