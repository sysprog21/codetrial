//! The `tests` module of `src/livekit.rs`, which declares this file by path.
//! Everything here reaches into `src/livekit.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;
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
    activity.note_candidate_finished(Instant::now());
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
    // every other text this server sends, and `record_model_input` is what
    // tests the counting.
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
    };
    send_watched_prompt(&mut activity, &coding, async {
        Ok::<(), std::io::Error>(())
    })
    .await
    .unwrap();
    assert!(!activity.behavioral_nudged);
    assert_eq!(activity.floor, Floor::Speaking);
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

    assert!(
        wait >= REPORT_TIMEOUT + WRAP_UP_WAIT,
        "a report bounded at {REPORT_TIMEOUT:?} after a {WRAP_UP_WAIT:?} wrap-up cannot land \
         before the page offers to leave at {wait:?}"
    );
}

/// Each pause reads the stretch since the last one, and never that stretch
/// twice.
///
/// The cursor is the whole of what makes this cheap. Left unmoved, every pause
/// re-reads the interview from the beginning, which is the cost this exists to
/// remove, and the notes would pile up restating the opening minutes.
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
    let mut slot = InterimReview::default();
    assert!(!slot.is_running());
    assert!(slot.finished().is_none());

    // Bounded, like the drop cases below. A slot that stopped handing back
    // finished reviews would otherwise spin here until something outside the
    // test gave up, which reads as a hung suite rather than as the answer.
    async fn collect(slot: &mut InterimReview) {
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
    let mut ending = InterimReview::default();
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
    let mut slot = InterimReview::default();
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
                    let _ = socket.next().await;
                    return seen;
                }
            }
        });
        let mut restarts = 0;
        let session = retry_cold_open(&keys, &mut restarts, || {
            crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        })
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
    }
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
        retry_cold_open(&keys, &mut 0, || {
            crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        }),
    )
    .await
    .err()
    .unwrap();
    server.abort();
    assert!(error.to_string().contains("did not open within"), "{error}");
    assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(started.elapsed() < COLD_OPEN_BACKOFF);
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
        let error = crate::gemini::first_open_within(
            COLD_OPEN_BACKOFF / 4,
            retry_cold_open(&keys, &mut restarts, || {
                attempts += 1;
                let error = keys.select().unwrap_err();
                async move { Err(error.into()) }
            }),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(attempts, 1, "{prefix}");
        if waits {
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
