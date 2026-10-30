//! The `tests` module of `src/livekit/turn.rs`, which declares this file by
//! path.
//! Everything here reaches into `src/livekit/turn.rs` through `super`, so it is
//! a unit
//! test and not an integration test: private items are in scope.

use super::*;
use crate::agent::{InterviewLoop, ThinkingHold};

/// A hold arms the discard so a turn already in flight cannot speak over
/// the release. Arming it on a turn that has finished is the failure: its
/// completion event has already passed, so nothing disarms it and the first
/// reply after the release is swallowed instead, which is silence exactly
/// where the candidate is waiting to be answered.
#[test]
fn only_a_turn_still_being_produced_leaves_output_to_discard() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    activity.floor = Floor::Speaking;
    assert!(hold_leaves_output_in_flight(&activity, now));
    activity.floor = Floor::AwaitingPlayout;
    assert!(!hold_leaves_output_in_flight(&activity, now));
    activity.floor = Floor::Listening;
    assert!(!hold_leaves_output_in_flight(&activity, now));
    activity.note_output(now);
    assert!(hold_leaves_output_in_flight(&activity, now));

    // A generation silent for a stall is dead, and nothing will end its turn,
    // including the floor its audio took.
    let stalled = now + PROMPT_STALL;
    let just_short = stalled - Duration::from_millis(1);
    activity.note_reply_audible();
    assert_eq!(activity.floor, Floor::Speaking);
    assert!(hold_leaves_output_in_flight(&activity, just_short));
    assert!(!hold_leaves_output_in_flight(&activity, stalled));

    // A tool continuation stalls the same way.
    let mut continuation = RuntimeActivity::new(now);
    continuation.note_tool_response(now);
    assert!(hold_leaves_output_in_flight(&continuation, just_short));
    assert!(!hold_leaves_output_in_flight(&continuation, stalled));

    // A prompt nothing has answered yet still has its reply on the way.
    let mut prompted = RuntimeActivity::new(now);
    prompted.mark_prompted(now, None, false);
    assert!(hold_leaves_output_in_flight(&prompted, stalled));
}

#[test]
fn candidate_exit_skips_wrap_up_before_report() {
    assert!(!should_send_wrap_up("candidate_ended"));
    assert!(should_send_wrap_up("time_up"));

    // Jim is told not to say goodbye before calling the tool, so the wrap-up is
    // the only thing that speaks the closing on its route out.
    assert!(should_send_wrap_up("interview_complete"));

    // An interviewer that stopped answering cannot say goodbye either, and
    // waiting on one would only delay the report.
    assert!(!should_send_wrap_up(INTERVIEWER_UNAVAILABLE));
}

/// The wait is shown only for a reply nothing has started on, once it has
/// gone `REPLY_WAIT_SHOWN`, and not over audio still playing or a review
/// that silence may answer.
#[test]
fn a_late_reply_is_shown_only_while_nothing_answers_it() {
    let start = Instant::now();
    let shown = start + REPLY_WAIT_SHOWN;
    let mut activity = RuntimeActivity::new(start);
    assert!(!activity.reply_visibly_late(shown, false), "nothing owed");

    activity.mark_prompted(start, Some("editor review"), true);
    assert!(
        !activity.reply_visibly_late(shown, false),
        "silence answers it"
    );

    activity.mark_prompted(start, Some("test reaction"), false);
    assert!(!activity.reply_visibly_late(shown - Duration::from_millis(1), false));
    assert!(activity.reply_visibly_late(shown, false));
    assert!(
        !activity.reply_visibly_late(shown, true),
        "audio still playing"
    );

    activity.note_output(Instant::now());
    assert!(
        !activity.reply_visibly_late(shown, false),
        "the reply started"
    );
}

/// Input transcription trails the audio, so the tail of speech a completed
/// turn answered can arrive after it. Arming a deadline on that tail asks for
/// a second answer once the candidate goes quiet; speech after the grace, or
/// after an interruption, is a new turn and is owed a reply.
#[test]
fn a_transcript_tail_after_a_completed_turn_owes_no_second_reply() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.turn_completed_at = Some(start);
    activity.note_candidate_finished(
        start + LATE_TRANSCRIPT_GRACE - Duration::from_millis(1),
        false,
    );
    assert!(!activity.reply_in_flight(), "the tail of the answered turn");
    assert!(!activity.reply_timed_out(start + REPLY_TIMEOUT * 2, false));

    activity.note_candidate_finished(start + LATE_TRANSCRIPT_GRACE, false);
    assert!(activity.reply_in_flight(), "a new turn after the grace");
}

/// An idle-window review runs in a pause and only in a pause.
///
/// Every condition is a way the room is busy, and each costs something
/// different if it is dropped: reviewing while Jim holds the floor spends a
/// call on a stretch still being said, reviewing with a tool response owed
/// races the turn it belongs to, and reviewing a stretch with nothing new in it
/// pays for a second reading of the same silence.
#[test]
fn a_pause_is_read_only_when_the_room_is_actually_idle() {
    let start = Instant::now();
    let idle = start + INTERIM_COOLDOWN + INTERIM_IDLE;
    let quiet = || RuntimeState {
        transcript: (0..INTERIM_MIN_NEW_TURNS)
            .map(|index| format!("Candidate: line {index}"))
            .collect(),
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    assert!(activity.interim_review_due(&quiet(), idle));

    // Each case is the idle room with one thing wrong with it.
    /// One way the room is busy: the thing to break, and why it disqualifies
    /// the pause.
    type Busy = (&'static str, fn(&mut RuntimeActivity, &mut RuntimeState));
    let busy: [Busy; 7] = [
        (
            "Jim is mid-sentence, so the stretch is not finished",
            |activity, _| {
                activity.mark_speaking();
            },
        ),
        (
            "a generation is already owed on this socket",
            |activity, _| {
                activity.socket.tool_response_outstanding = true;
            },
        ),
        (
            "the candidate is waiting on a reply, so the pause is Jim's",
            |activity, _| {
                activity.awaiting_reply_since = Some(Instant::now());
            },
        ),
        // Expressed as "spoke a second ago", not as an instant in the future: a
        // stamp ahead of `now` fails this too, but only because
        // `duration_since` saturates to zero, which is not the rule being
        // pinned.
        (
            "the candidate has only just stopped talking",
            |activity, _| {
                activity.last_user_speech +=
                    INTERIM_COOLDOWN + INTERIM_IDLE - Duration::from_secs(1);
            },
        ),
        ("a stretch already read is not read again", |_, state| {
            state.interim_transcript_lines = state.transcript.len();
        }),
        (
            "the interviewer asked to close, and the end aborts a running review",
            |_, state| {
                state.end_requested = true;
            },
        ),
        (
            "the planned time runs out before the call could return",
            |_, state| {
                let planned = u64::from(state.coding_minutes + state.behavioral_minutes) * 60;
                state.started_at -= Duration::from_secs(planned - 5);
            },
        ),
    ];
    for (why, break_it) in busy {
        let mut activity = RuntimeActivity::new(start);
        let mut state = quiet();
        break_it(&mut activity, &mut state);
        assert!(!activity.interim_review_due(&state, idle), "{why}");
    }

    assert!(
        !activity.interim_review_due(&quiet(), start + INTERIM_IDLE),
        "two reviews in one interview are not two reviews in one minute"
    );
    assert!(
        !activity.interim_review_due(
            &RuntimeState {
                transcript: (0..INTERIM_MIN_NEW_TURNS * 2)
                    .map(|index| format!("Interviewer: line {index}"))
                    .collect(),
                ..RuntimeState::default()
            },
            idle
        ),
        "the interviewer talking to itself is not new evidence about the candidate"
    );

    // Claiming the pause is what spends it: the cooldown is stamped where it is
    // read, so the next tick cannot start a second review.
    assert!(activity.claim_interim_review(&quiet(), idle));
    assert!(!activity.claim_interim_review(&quiet(), idle));
}

#[test]
fn interim_review_stops_at_the_cap() {
    let start = Instant::now();
    let state = RuntimeState {
        transcript: (0..INTERIM_MIN_NEW_TURNS)
            .map(|index| format!("Candidate: line {index}"))
            .collect(),
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::with_interim_review_cap(start, 2);
    let first = start + INTERIM_COOLDOWN + INTERIM_IDLE;

    assert!(activity.claim_interim_review(&state, first));
    assert!(activity.claim_interim_review(&state, first + INTERIM_COOLDOWN));
    assert!(!activity.claim_interim_review(&state, first + INTERIM_COOLDOWN * 2));
}

#[test]
fn interim_review_cap_of_zero_claims_none() {
    let start = Instant::now();
    let state = RuntimeState {
        transcript: (0..INTERIM_MIN_NEW_TURNS)
            .map(|index| format!("Candidate: line {index}"))
            .collect(),
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::with_interim_review_cap(start, 0);

    assert!(!activity.claim_interim_review(&state, start + INTERIM_COOLDOWN + INTERIM_IDLE));
}

/// The idle-window review's constants are eleven numbers in three modules, and
/// three pairs of them are load-bearing on each other. Written down here in the
/// shape `report_network_budget_covers_every_repair_and_retry_per_generation`
/// established: the arithmetic a design depends on is asserted, not described,
/// because a comment saying two numbers must relate is checked by nobody.
///
/// One relationship is missing from this list on purpose. The reserve being
/// smaller than the store is asserted at the declaration, where whoever changes
/// either number is already standing; restating it here would be a second copy
/// of the fact, which is what this test exists to prevent.
#[test]
fn one_review_at_a_time_is_arithmetic_and_not_a_hope() {
    use crate::agent::{INTERIM_CONTEXT_NOTES, MAX_INTERIM_LINES_PER_REVIEW, MAX_INTERIM_NOTES};
    use crate::config::DEFAULT_MAX_INTERIM_REVIEWS;
    use crate::gemini::INTERIM_ATTEMPT_TIMEOUT;

    // A call cannot outlive the wait for the next chance to start one. This is
    // what makes the slot in `SideCall` unable to be occupied when a pause
    // comes due, so the two mechanisms cannot disagree about whether a review
    // is running.
    const { assert!(INTERIM_ATTEMPT_TIMEOUT.as_secs() < INTERIM_COOLDOWN.as_secs()) };

    // What a later review is shown has to leave room for what it may add, or
    // every review is handed a context it cannot help repeating.
    const { assert!(INTERIM_CONTEXT_NOTES + MAX_INTERIM_LINES_PER_REVIEW < MAX_INTERIM_NOTES) };

    // The default quota fits the retained note budget, so no review evicts a
    // previous one before the interview ends. It uses half: the budget is sized
    // for an operator who sets twelve.
    const { assert!(DEFAULT_MAX_INTERIM_REVIEWS * MAX_INTERIM_LINES_PER_REVIEW <= MAX_INTERIM_NOTES) };

    // A pause has to be long enough to be worth reading and short enough to
    // happen; a threshold at or above the cooldown would mean the cooldown
    // never decided anything.
    const { assert!(INTERIM_IDLE.as_secs() < INTERIM_COOLDOWN.as_secs()) };

    // What one call is asked to read, against the seconds it has to read it.
    // Not a throughput claim -- it is the ceiling that keeps the deadline
    // meaningful rather than a coin flip on a long backlog. A review that
    // cannot finish inside INTERIM_ATTEMPT_TIMEOUT spends its prefill and
    // returns nothing, and the window is marked read either way.
    const { assert!(INTERIM_WINDOW_BYTES + INTERIM_CODE_BYTES <= 16 * 1024) };

    // Floored as well as capped. A budget of a couple of kilobytes reads a
    // stretch of interview as a fragment, and a review of a fragment is a note
    // about nothing -- the failure a ceiling on its own cannot see.
    const { assert!(INTERIM_WINDOW_BYTES >= 4 * 1024) };
    const { assert!(INTERIM_CODE_BYTES >= 2 * 1024) };
}

/// A line the model was shown and that has since gone is retracted in so many
/// words; left out, the model keeps holding the stale one.
#[test]
fn an_evidence_line_that_goes_away_is_retracted() {
    let lines = |items: &[&str]| {
        items
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
    };
    let shown = lines(&["tests: not run", "session: ended"]);
    assert_eq!(
        evidence_delta(Some(&shown), &lines(&["tests: not run"])),
        "session: none"
    );
    assert_eq!(
        evidence_delta(
            Some(&shown),
            &lines(&["tests: 1 of 2 passing", "session: ended"])
        ),
        "tests: 1 of 2 passing"
    );
    assert_eq!(evidence_delta(Some(&shown), &shown), "");
    assert_eq!(
        evidence_delta(None, &lines(&["tests: not run"])),
        "tests: not run"
    );
}

/// A review has to be able to return before the planned end: one whose call
/// would run into it is not started, and one a second earlier is.
#[test]
fn no_interim_review_starts_that_the_end_would_abort() {
    let state = RuntimeState {
        transcript: (0..INTERIM_MIN_NEW_TURNS)
            .map(|index| format!("Candidate: line {index}"))
            .collect(),
        ..RuntimeState::default()
    };
    let planned =
        Duration::from_secs(u64::from(state.coding_minutes + state.behavioral_minutes) * 60);
    let last_call = state.started_at + planned - crate::gemini::INTERIM_ATTEMPT_TIMEOUT;
    let activity = RuntimeActivity::new(state.started_at);
    assert!(!activity.interim_review_due(&state, last_call));
    assert!(activity.interim_review_due(&state, last_call - Duration::from_secs(1)));
}

#[test]
fn a_tool_continuation_stays_owed_after_prompt_output() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, None, false);
    activity.note_output(Instant::now());
    activity.socket.tool_response_outstanding = true;

    assert!(activity.prompted_at.is_none());
    assert!(activity.awaiting_reply_since.is_none());
    assert!(activity.owes_reply());

    activity.socket.tool_response_outstanding = false;
    activity.mark_listening();
    assert!(!activity.owes_reply());
}

/// A prompt Gemini never answers must not hold the floor forever: a held floor
/// silences every nudge and keeps a `GoAway` waiting until the server drops the
/// socket from an older checkpoint. Releasing it keeps the reply owed, so the
/// replacement socket still answers.
#[test]
fn a_prompt_with_no_output_returns_the_floor_but_stays_owed() {
    let quiet = Stalls {
        prompt_released: false,
        spend_restart: false,
    };
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    assert!(!activity.owes_reply());

    activity.mark_prompted(start, None, false);
    assert!(activity.owes_reply());
    assert_eq!(
        activity.settle_stalls(start + PROMPT_STALL - Duration::from_millis(1), false),
        quiet
    );
    assert_eq!(activity.floor, Floor::Speaking);

    assert_eq!(
        activity.settle_stalls(start + PROMPT_STALL, false),
        Stalls {
            prompt_released: true,
            spend_restart: true,
        }
    );
    assert_eq!(activity.floor, Floor::Listening);
    assert!(activity.owes_reply(), "the reply was never given");
    assert_eq!(
        activity.settle_stalls(start + PROMPT_STALL * 2, false),
        quiet,
        "a floor already returned is not returned again"
    );

    activity.mark_prompted(start, None, false);
    activity.note_output(Instant::now());
    assert!(!activity.owes_reply());
    assert_eq!(
        activity.settle_stalls(start + PROMPT_STALL, false),
        quiet,
        "a prompt that produced output is being answered, however slowly it plays"
    );
}

/// The candidate's own turn stalls too: they finish, Gemini sends nothing, and
/// a held `GoAway` has no later event to be spent on. The floor is already
/// theirs, so only the restart is released.
#[test]
fn an_unanswered_candidate_turn_lets_a_held_restart_go() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    assert!(
        !activity
            .settle_stalls(start + PROMPT_STALL - Duration::from_millis(1), false)
            .spend_restart
    );
    assert_eq!(
        activity.settle_stalls(start + PROMPT_STALL, false),
        Stalls {
            prompt_released: false,
            spend_restart: true,
        }
    );
    assert!(activity.owes_reply(), "their turn is still unanswered");
}

/// A candidate who speaks after a prompt went unanswered has moved on: their
/// turn is what is owed, not the stale prompt.
#[test]
fn speaking_again_retires_an_unanswered_prompt() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, None, false);
    activity.settle_stalls(start + PROMPT_STALL, false);
    activity.note_candidate_finished(start + PROMPT_STALL * 2, false);
    assert!(activity.prompted_at.is_none());
    assert!(activity.reply_in_flight());

    // A prompt that has produced nothing yet is not a reply under way, so the
    // candidate speaking over it is their turn to answer.
    let mut waiting = RuntimeActivity::new(start);
    waiting.mark_prompted(start, None, false);
    waiting.note_candidate_finished(start + Duration::from_secs(1), false);
    assert!(waiting.prompted_at.is_none());
    assert!(waiting.reply_in_flight());

    // While Gemini is producing, a lagging transcript fragment retires nothing:
    // the prompt went out behind that turn, and its answer may still be on its
    // way.
    let mut speaking = RuntimeActivity::new(start);
    speaking.note_output(Instant::now());
    speaking.mark_prompted(start, None, false);
    speaking.note_candidate_finished(start + Duration::from_secs(1), false);
    assert!(speaking.prompted_at.is_some());
    assert!(!speaking.reply_in_flight());
}

/// Output that follows a prompt sent mid-generation is the earlier
/// generation's tail until that turn ends, so it cannot settle the prompt.
#[test]
fn output_behind_a_prompt_belongs_to_the_earlier_turn() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_output(Instant::now());
    activity.mark_prompted(start, None, false);
    activity.note_output(Instant::now());
    assert!(
        activity.owes_reply(),
        "the old sentence's tail answers nothing"
    );
    activity.note_turn_boundary(Instant::now());
    assert!(
        activity.owes_reply(),
        "the old turn ending is not the answer"
    );
    activity.note_output(Instant::now());
    assert!(!activity.owes_reply());
}

/// A prompt's own turn ending with nothing said is Gemini answering with
/// silence, and replaying it later would ask what it chose not to.
#[test]
fn a_prompt_answered_with_silence_is_not_owed() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, None, false);
    activity.note_turn_boundary(Instant::now());
    assert!(!activity.owes_reply());
}

/// A briefing that never reached the socket is still owed, without taking
/// the floor from a watcher that has nothing generating to wait for.
#[test]
fn an_owed_briefing_leaves_the_floor_alone() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.owe_prompt(start, None);
    assert!(activity.owes_reply());
    assert_eq!(activity.floor, Floor::Listening);
}

fn window(
    trigger_tokens: u32,
    target_tokens: u32,
) -> Option<crate::config::GeminiContextCompression> {
    Some(crate::config::GeminiContextCompression {
        trigger_tokens,
        target_tokens,
    })
}

/// One completed turn whose usage arrived as `observations`, the last on the
/// completing frame.
fn complete_turn(
    activity: &mut RuntimeActivity,
    pair: Option<crate::config::GeminiContextCompression>,
    observations: &[u64],
) {
    for prompt in observations {
        activity.tokens.observe_prompt_tokens(pair, *prompt);
    }
    activity.tokens.observe_turn_complete(pair);
}

#[test]
fn context_refresh_tracks_a_significant_prompt_drop_and_ignores_missing_counts() {
    let pair = window(30_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[20_000]);
    complete_turn(&mut activity, pair, &[0]);
    assert_eq!(activity.tokens.peak_prompt_tokens, 20_000);
    assert!(!activity.tokens.context_refresh_pending);
    complete_turn(&mut activity, pair, &[19_000]);
    assert!(!activity.tokens.context_refresh_pending);
    complete_turn(&mut activity, pair, &[8_000]);
    assert!(activity.tokens.context_refresh_pending);
    complete_turn(&mut activity, pair, &[9_000]);
    assert!(activity.tokens.context_refresh_pending);
}

/// The input of the turn that was cut can make up most of the cut. A context
/// that had reached the trigger and then shrank at all was cut regardless.
#[test]
fn a_cut_hidden_by_new_input_is_still_seen_at_the_trigger() {
    let pair = window(20_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[20_100]);
    complete_turn(&mut activity, pair, &[19_500]);
    assert!(activity.tokens.context_refresh_pending);

    // At the trigger, a context that did not shrink was not cut.
    let pair = window(20_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[20_100]);
    complete_turn(&mut activity, pair, &[20_100]);
    assert!(!activity.tokens.context_refresh_pending);

    // Below the trigger the same small drop is noise.
    let pair = window(20_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[18_000]);
    complete_turn(&mut activity, pair, &[17_500]);
    assert!(!activity.tokens.context_refresh_pending);
}

#[test]
fn context_refresh_waits_for_candidate_output_tools_and_owed_replies() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    assert!(!activity.can_refresh_context(false, false, now));
    activity.tokens.context_refresh_pending = true;
    assert!(activity.can_refresh_context(false, false, now));
    assert!(!activity.can_refresh_context(true, false, now));
    assert!(!activity.can_refresh_context(false, true, now));
    activity.generating = true;
    assert!(!activity.can_refresh_context(false, false, now));
    activity.generating = false;
    activity.socket.tool_response_outstanding = true;
    assert!(!activity.can_refresh_context(false, false, now));
    activity.socket.tool_response_outstanding = false;
    activity.floor = Floor::Speaking;
    assert!(!activity.can_refresh_context(false, false, now));
    activity.floor = Floor::Listening;
    activity.awaiting_reply_since = Some(now);
    assert!(!activity.can_refresh_context(false, false, now));
    activity.awaiting_reply_since = None;
    activity.owe_prompt(now, None);
    assert!(!activity.can_refresh_context(false, false, now));
}

/// Speech reaches Gemini before its transcript reaches the room, so a turn
/// the transcript has not opened yet can already be under way. The microphone
/// is what says so.
#[test]
fn context_refresh_waits_for_the_microphone_to_go_quiet() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.tokens.context_refresh_pending = true;
    activity.candidate_voice_at = Some(now);
    assert!(!activity.can_refresh_context(false, false, now));
    assert!(!activity.can_refresh_context(
        false,
        false,
        now + CHECKPOINT_VOICE_QUIET - Duration::from_millis(1)
    ));
    assert!(activity.can_refresh_context(false, false, now + CHECKPOINT_VOICE_QUIET));
}

#[test]
fn context_refresh_is_opt_in_and_tracks_small_configured_windows() {
    let pair = None;
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[20_000]);
    complete_turn(&mut activity, pair, &[8_000]);
    assert_eq!(activity.tokens.peak_prompt_tokens, 0);
    assert!(!activity.tokens.context_refresh_pending);
    let pair = window(9_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[8_900]);
    complete_turn(&mut activity, pair, &[8_300]);
    assert!(activity.tokens.context_refresh_pending);
}

/// A turn's usage may arrive on a frame of its own, before the one that
/// completes it. The drop is judged at completion, against the largest count
/// seen since the last one.
#[test]
fn usage_on_a_separate_frame_is_judged_at_completion() {
    let pair = window(30_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[20_000]);
    activity.tokens.observe_prompt_tokens(pair, 8_000);
    assert!(!activity.tokens.context_refresh_pending);
    activity.tokens.observe_turn_complete(pair);
    assert!(activity.tokens.context_refresh_pending);

    // A periodic count higher than the completed one is the reference.
    let pair = window(30_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[5_000]);
    complete_turn(&mut activity, pair, &[21_000, 9_000]);
    assert!(activity.tokens.context_refresh_pending);
}

#[test]
fn a_cold_replacement_forgets_the_old_context() {
    let pair = window(20_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[19_000]);
    activity.tokens.context_refresh_pending = true;
    activity.reset_context_observations(false);
    assert!(!activity.tokens.context_refresh_pending);
    complete_turn(&mut activity, pair, &[6_000]);
    assert!(!activity.tokens.context_refresh_pending);
}

/// A resumed socket still holds the provider's context, so a cut in its first
/// turn is measured against the turn before the replacement.
#[test]
fn a_resumed_replacement_keeps_the_baseline() {
    let pair = window(30_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[19_000]);
    activity.tokens.observe_prompt_tokens(pair, 19_500);
    activity.tokens.context_refresh_pending = true;
    activity.reset_context_observations(true);
    assert!(!activity.tokens.context_refresh_pending);
    assert_eq!(activity.tokens.latest_prompt_tokens, None);
    complete_turn(&mut activity, pair, &[6_000]);
    assert!(activity.tokens.context_refresh_pending);
}

#[test]
fn usage_is_summed_numbered_and_labelled_by_what_asked_for_it() {
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.live_session_id = 1_700_000_000;
    activity.live_socket = 2;
    let pair = window(30_000, 8_000);
    let observation = |prompt| crate::gemini::TokenUsage {
        prompt,
        response: 5,
        samples: 1,
        ..Default::default()
    };
    let first = activity.account_live_usage("room-a", "01:02.003", None, pair, observation(100));
    assert!(
        first.starts_with(
            "codetrial live_turn_usage room=room-a session=1700000000 at=01:02.003 socket=2 usage_event=1 cause=candidate prompt_tokens=100 response_tokens=5"
        ),
        "{first}"
    );
    let second =
        activity.account_live_usage("room-a", "01:05.000", Some("watch"), pair, observation(250));
    assert!(
        second.contains(" usage_event=2 cause=watch prompt_tokens=250"),
        "{second}"
    );
    assert_eq!(activity.live_usage.prompt, 350);
    assert_eq!(activity.live_usage.response, 10);
    assert_eq!(activity.live_usage.samples, 2);
    // The compression watch saw both.
    assert_eq!(activity.tokens.peak_prompt_tokens, 250);
    assert_eq!(activity.tokens.latest_prompt_tokens, Some(250));
}

#[test]
fn an_interview_names_its_session_from_the_start() {
    let activity = RuntimeActivity::for_interview(Instant::now(), 3, 1_700_000_000_123);
    assert_eq!(activity.live_session_id, 1_700_000_000_123);
    assert_eq!(activity.max_interim_reviews, 3);
}

/// An interview that has ended, or asked to, owes the model no checkpoint,
/// whatever else would let one go out.
#[test]
fn a_checkpoint_is_due_only_while_the_interview_runs() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    activity.tokens.context_refresh_pending = true;
    let running = RuntimeState::default();
    assert!(activity.checkpoint_due(&running, false, false, now));
    for state in [
        RuntimeState {
            ended: true,
            ..RuntimeState::default()
        },
        RuntimeState {
            end_requested: true,
            ..RuntimeState::default()
        },
    ] {
        assert!(!activity.checkpoint_due(&state, false, false, now));
    }
    assert!(!activity.checkpoint_due(&running, true, false, now));
    assert!(!activity.checkpoint_due(&running, false, true, now));
}

#[test]
fn thinking_suppresses_silence_and_editor_prompts_until_released() {
    let start = Instant::now();
    let now = start + Duration::from_secs(300);
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held {
            since: std::time::Instant::now(),
        },
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    assert!(activity.watch_prompt(&mut state, now).is_none());
    state.thinking_hold = ThinkingHold::Off;
    assert!(activity.watch_prompt(&mut state, now).is_some());
}

#[test]
fn drawing_activity_defers_the_idle_nudge_like_typing() {
    let start = Instant::now();
    let now = start + Duration::from_secs(300);
    let mut state = RuntimeState {
        example_drawing_enabled: true,
        ..Default::default()
    };
    let mut idle = RuntimeActivity::new(start);
    assert!(idle.watch_prompt(&mut state, now).is_some());
    let mut state = RuntimeState {
        example_drawing_enabled: true,
        ..Default::default()
    };
    let payload: serde_json::Value =
        serde_json::from_str(include_str!("../../fixtures/control.json")).unwrap();
    let activity_packet = payload["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "drawing activity")
        .unwrap();
    let result = crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &activity_packet["payload"],
        0.0,
    );
    assert!(result.update_last_code_change);
    let mut activity = RuntimeActivity::new(start);
    if result.update_last_code_change {
        activity.last_code_change = now;
    }
    assert!(
        activity
            .watch_prompt(&mut state, now + CODE_SETTLE + Duration::from_secs(1))
            .is_none()
    );
    assert!(
        activity
            .watch_prompt(&mut state, now + Duration::from_secs(60))
            .is_some()
    );
}

#[test]
fn tentative_spoken_holds_leave_no_declared_gaps_or_reply_over_continued_speech() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.observe_thinking_fragment(&mut state, "Let me think", Instant::now(), 100);
    assert_eq!(state.take_thinking_notice(), None);
    assert!(state.thinking_hold.is_active());
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 0);
    activity.socket.discarding_output = true;
    activity.observe_thinking_fragment(
        &mut state,
        "Let me think about the edge cases",
        Instant::now(),
        101,
    );
    assert_eq!(state.take_thinking_notice(), None);
    assert!(!state.thinking_hold.is_active());
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 0);
    // Gemini may already be answering, into a discard that drops the answer.
    assert!(activity.socket.reply_after_thinking_discard);
    assert!(
        !activity.claim_thinking_reply(&state),
        "not while discarding"
    );

    // Cut off by the candidate still talking: their speech gets its own reply.
    activity.end_discard(true);
    assert!(!activity.claim_thinking_reply(&state));
}

#[test]
fn an_answer_dropped_under_a_hold_that_speech_ended_is_asked_for_again() {
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held {
            since: Instant::now(),
        },
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.socket.discarding_output = true;
    activity.observe_thinking_fragment(&mut state, "I'd use two pointers", Instant::now(), 1);
    assert!(!state.thinking_hold.is_active());

    // The dropped reply's own turn finishes: nothing of it was heard.
    activity.end_discard(false);
    assert!(activity.claim_thinking_reply(&state));
    assert!(!activity.claim_thinking_reply(&state), "asked for once");
}

#[test]
fn only_a_confirmed_request_declares_a_gap_and_ended_sessions_cannot_hold() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.observe_thinking_fragment(
        &mut state,
        "Okay, let me think for a moment",
        Instant::now(),
        100,
    );
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 0);
    activity.confirm_thinking_request(&mut state, Instant::now(), 101);
    assert_eq!(state.take_thinking_notice(), Some(true));
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 1);
    activity.confirm_thinking_request(&mut state, Instant::now(), 102);
    assert_eq!(state.take_thinking_notice(), None);
    for filler in ["okay", "hmm", "so", "well", "right"] {
        activity.observe_thinking_fragment(&mut state, filler, Instant::now(), 103);
        assert_eq!(state.take_thinking_notice(), None);
        assert!(state.thinking_hold.is_active());
    }
    activity.observe_thinking_fragment(&mut state, "Ready", Instant::now(), 104);
    assert_eq!(state.take_thinking_notice(), Some(false));
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 2);
    state.ended = true;
    activity.observe_thinking_fragment(&mut state, "Wait", Instant::now(), 105);
    assert_eq!(state.take_thinking_notice(), None);
    assert!(!state.thinking_hold.is_active());
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 2);
}

#[test]
fn browser_actions_settle_tentative_requests_without_unpaired_hold_rows() {
    for (payload, confirmed, changed) in [
        (
            serde_json::json!({"type":"thinking","thinking":true}),
            true,
            Some(true),
        ),
        // Continue before the request was confirmed: nothing started, so
        // nothing may end in the ledger.
        (
            serde_json::json!({"type":"thinking","thinking":false}),
            false,
            Some(false),
        ),
        (serde_json::json!({"type":"yield_turn"}), false, None),
        (
            serde_json::json!({"type":"end_interview","reason":"time_expired"}),
            false,
            None,
        ),
    ] {
        let mut state = RuntimeState::default();
        let mut activity = RuntimeActivity::new(Instant::now());
        activity.observe_thinking_fragment(&mut state, "Wait", Instant::now(), 100);
        assert!(state.thinking_hold.is_requested());
        let applied = apply_control(&mut activity, &mut state, &payload);
        assert_eq!(applied.thinking_changed, changed, "{payload}");
        assert_eq!(state.thinking_hold.is_declared(), confirmed, "{payload}");
        assert!(confirmed || !state.thinking_hold.is_active(), "{payload}");
        let ended = u32::from(payload["type"] == "end_interview");
        assert_eq!(
            state.evidence_ledger.lifecycle.transitions,
            u32::from(confirmed) + ended,
            "{payload}"
        );
        if confirmed {
            apply_control(
                &mut activity,
                &mut state,
                &serde_json::json!({"type":"thinking","thinking":false}),
            );
            assert_eq!(state.evidence_ledger.lifecycle.transitions, 2);
        }
    }
}

#[test]
fn abandoning_a_request_mid_discard_owes_the_dropped_reply_and_confirming_does_not() {
    for (payload, owed) in [
        (serde_json::json!({"type":"yield_turn"}), true),
        (
            serde_json::json!({"type":"thinking","thinking":true}),
            false,
        ),
    ] {
        let mut state = RuntimeState {
            thinking_hold: ThinkingHold::Requested {
                since: std::time::Instant::now(),
            },
            ..RuntimeState::default()
        };
        let mut activity = RuntimeActivity::new(Instant::now());
        activity.socket.discarding_output = true;
        activity.socket.reply_after_thinking_discard = !owed;
        apply_control(&mut activity, &mut state, &payload);
        assert_eq!(
            activity.socket.reply_after_thinking_discard, owed,
            "{payload}"
        );
    }
}

#[test]
fn a_timed_event_abandons_an_unconfirmed_hold_and_is_delivered_at_once() {
    let mut state = RuntimeState::default();
    state.started_at -=
        Duration::from_secs(u64::from(state.coding_minutes + state.behavioral_minutes) * 60 - 200);
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.observe_thinking_fragment(&mut state, "Wait", Instant::now(), 100);
    assert!(state.thinking_hold.is_requested());
    let payload = serde_json::json!({"type":"time_warning"});
    let result = apply_control(&mut activity, &mut state, &payload);
    assert!(!state.thinking_hold.is_active());
    assert!(
        result
            .generate_reply
            .unwrap()
            .contains("five-minute warning")
    );
    assert_eq!(result.thinking_changed, None);
    // A tentative hold never started in the ledger, so nothing ends there.
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 0);
}

#[test]
fn a_timed_packet_the_reducer_rejects_leaves_a_request_standing() {
    // The interview has only just started, so neither packet is due.
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.observe_thinking_fragment(&mut state, "Wait", Instant::now(), 100);
    for payload in [
        serde_json::json!({"type":"time_warning"}),
        serde_json::json!({"type":"round_transition","round":"behavioral"}),
    ] {
        let result = apply_control(&mut activity, &mut state, &payload);
        assert!(result.generate_reply.is_none(), "{payload}");
        assert!(state.thinking_hold.is_requested(), "{payload}");
    }
}

#[test]
fn a_compound_request_confirms_one_hold_and_supersedes_tentative_reply_debt() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    for text in ["Wait", "Wait, let me", "Wait, let me think"] {
        activity.observe_thinking_fragment(&mut state, text, Instant::now(), 100);
    }
    assert!(state.thinking_hold.is_active());
    activity.socket.reply_after_thinking_discard = true;
    activity.confirm_thinking_request(&mut state, Instant::now(), 101);
    assert_eq!(state.take_thinking_notice(), Some(true));
    assert!(!activity.socket.reply_after_thinking_discard);
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 1);
}

#[test]
fn a_button_hold_ignores_transcripts_of_speech_from_before_the_click() {
    let click = Instant::now();
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(click);
    apply_control(
        &mut activity,
        &mut state,
        &serde_json::json!({"type":"thinking","thinking":true}),
    );
    assert_eq!(state.take_thinking_notice(), Some(true));
    let grace = thinking_transcript_grace(3_000);
    activity.ignore_input_before_hold(click, grace);

    // The transcript of what was said before the click arrives after it.
    let late = click + Duration::from_millis(300);
    activity.observe_thinking_fragment(
        &mut state,
        "I'm not sure how to handle duplicates",
        late,
        400,
    );
    assert_eq!(state.take_thinking_notice(), None);
    assert!(state.thinking_hold.is_declared());
    activity.observe_thinking_fragment(&mut state, "hmm", click + grace, 401);
    assert_eq!(state.take_thinking_notice(), None);
    assert!(
        state.thinking_hold.is_declared(),
        "a filler never ends the hold"
    );
    activity.observe_thinking_fragment(&mut state, "Ready", click + grace, 402);
    assert_eq!(state.take_thinking_notice(), Some(false));
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 2);
}

#[test]
fn an_answer_given_straight_after_the_click_is_not_swallowed() {
    let click = Instant::now();
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held {
            since: std::time::Instant::now(),
        },
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(click);
    let grace = thinking_transcript_grace(3_000);
    activity.ignore_input_before_hold(click, grace);

    // The pre-click transcript inside the window does not stretch it: the
    // answer that follows is transcribed a silence window after it ends, so it
    // lands after the window and releases the hold.
    let before = click + Duration::from_millis(300);
    activity.observe_thinking_fragment(&mut state, "I would sort", before, 1);
    assert_eq!(state.take_thinking_notice(), None);
    let answer = click + Duration::from_millis(3_300);
    activity.observe_thinking_fragment(&mut state, "Actually it is linear", answer, 2);
    assert_eq!(state.take_thinking_notice(), Some(false));
}

#[test]
fn the_pre_click_window_stays_inside_the_silence_window() {
    assert_eq!(thinking_transcript_grace(3_000), THINKING_TRANSCRIPT_GRACE);
    assert_eq!(
        thinking_transcript_grace(1_500),
        Duration::from_millis(1_250)
    );
    assert_eq!(thinking_transcript_grace(1_000), Duration::from_millis(750));
    assert_eq!(thinking_transcript_grace(500), Duration::from_millis(250));
    assert_eq!(thinking_transcript_grace(250), Duration::from_millis(125));
    assert_eq!(thinking_transcript_grace(0), Duration::ZERO);
    for silence_ms in [200, 250, 400, 500, 1_000, 1_500, 3_000, 30_000] {
        let grace = thinking_transcript_grace(silence_ms);
        let silence = Duration::from_millis(u64::from(silence_ms));
        assert!(
            grace >= silence / 2 || grace == THINKING_TRANSCRIPT_GRACE,
            "{silence_ms}"
        );
        assert!(grace < silence, "{silence_ms}");
    }
}

/// The shipped window, where half of it left only 0.75s for the pre-click
/// transcript: one delayed past that by a busy loop is still the tail of what
/// came before the click, and an answer begun after it still releases.
#[test]
fn a_delayed_pre_click_transcript_keeps_the_hold_at_the_default_window() {
    let click = Instant::now();
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held {
            since: std::time::Instant::now(),
        },
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(click);
    let silence_ms = crate::config::DEFAULT_GEMINI_SILENCE_MS;
    activity.ignore_input_before_hold(click, thinking_transcript_grace(silence_ms));

    let delayed = click + Duration::from_millis(900);
    activity.observe_thinking_fragment(&mut state, "and that is my approach", delayed, 1);
    assert_eq!(
        state.take_thinking_notice(),
        None,
        "the tail does not release"
    );

    // The quickest answer after the click: a short word, then the window.
    let answer = click + Duration::from_millis(200 + u64::from(silence_ms));
    activity.observe_thinking_fragment(&mut state, "Actually it is linear", answer, 2);
    assert_eq!(state.take_thinking_notice(), Some(false));
}

#[test]
fn a_silent_declared_hold_ends_in_one_check_in() {
    let start = Instant::now();
    let limit = Duration::from_secs(crate::agent::THINKING_CHECK_IN_S);
    let second = Duration::from_secs(1);
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held { since: start },
        ..RuntimeState::default()
    };
    assert!(!state.claim_thinking_check_in(start, 0));
    assert!(!state.claim_thinking_check_in(start + limit - second, 0));
    assert!(state.claim_thinking_check_in(start + limit, 7));
    assert!(!state.thinking_hold.is_active());
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 1);
    assert!(!state.claim_thinking_check_in(start + limit * 2, 8));

    // A provisional request is not a declared hold.
    state.thinking_hold = ThinkingHold::Requested {
        since: std::time::Instant::now(),
    };
    assert!(!state.claim_thinking_check_in(start + limit * 5, 9));
}

#[test]
fn a_new_hold_does_not_inherit_the_last_ones_clock() {
    let start = Instant::now();
    let limit = Duration::from_secs(crate::agent::THINKING_CHECK_IN_S);
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held { since: start },
        ..RuntimeState::default()
    };
    // Continue, then Thinking again, just short of the first hold's limit.
    let again = start + limit - Duration::from_secs(1);
    state.end_thinking(1);
    state.declare_thinking(again, 2);
    assert!(!state.claim_thinking_check_in(start + limit, 3));
    assert!(state.claim_thinking_check_in(again + limit, 4));
}

#[test]
fn the_check_in_carries_what_the_hold_left_owed() {
    let state = RuntimeState {
        thinking_unheard_reply: true,
        owed_reply_on_resume: Some(Some("Answer the outstanding candidate question.".into())),
        ..RuntimeState::default()
    };
    let prompt = crate::agent::thinking_check_in(&state);
    assert!(prompt.contains("Nothing you said while the candidate was thinking"));
    assert!(prompt.contains("Answer the outstanding candidate question."));
    assert!(prompt.contains("silent for 2 minutes"));
    assert!(prompt.contains("Do not give a hint"));
}

#[test]
fn continuing_while_a_held_reply_is_discarded_owes_one_reply_after_its_boundary() {
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.socket.discarding_output = true;
    activity.defer_thinking_reply("The five-minute warning is due. The candidate is ready.");
    assert!(activity.owes_reply());
    assert!(!activity.claim_thinking_reply(&state));

    // The dropped turn's ending, the way the room loop takes it: it ends the
    // discard and settles nothing sent since.
    assert!(!super::super::session::accept_gemini_event(
        &crate::gemini::GeminiEvent::TurnComplete,
        &mut activity,
        false,
        Instant::now()
    ));
    assert!(!activity.socket.discarding_output);
    assert!(activity.owes_reply());
    assert!(activity.claim_thinking_reply(&state));
    assert!(!activity.claim_thinking_reply(&state));
    assert!(
        activity
            .thinking_reply_prompt()
            .contains("five-minute warning")
    );
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.defer_thinking_reply("Ready");
    assert!(!activity.claim_thinking_reply(&state));
}

#[test]
fn reconnect_only_declares_confirmed_thinking_requests() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.observe_thinking_fragment(&mut state, "Wait", Instant::now(), 100);
    assert!(state.thinking_hold.is_active());
    assert!(!state.thinking_hold.is_declared());
    activity.observe_thinking_fragment(
        &mut state,
        "Wait, actually we sort first",
        Instant::now(),
        101,
    );
    assert!(!state.thinking_hold.is_declared());
    activity.observe_thinking_fragment(&mut state, "Wait", Instant::now(), 102);
    activity.confirm_thinking_request(&mut state, Instant::now(), 103);
    assert_eq!(state.take_thinking_notice(), Some(true));
    assert!(state.thinking_hold.is_declared());
}

#[test]
fn a_tentative_hold_does_not_repeat_an_already_answered_system_prompt() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.mark_prompted(Instant::now(), Some("old five-minute warning"), false);
    activity.note_output(Instant::now());
    activity.note_turn_boundary(Instant::now());
    activity.socket.discarding_output = true;
    activity.observe_thinking_fragment(&mut state, "Wait", Instant::now(), 100);
    let payload = serde_json::json!({"type":"yield_turn"});
    apply_control(&mut activity, &mut state, &payload);
    activity.socket.discarding_output = false;
    assert!(activity.claim_thinking_reply(&state));
    assert!(!activity.owes_prompt());
    let prompt = activity.thinking_reply_prompt();
    assert!(!prompt.contains("old five-minute warning"));
}

#[test]
fn a_request_that_turns_into_reasoning_still_owes_the_note_for_what_it_dropped() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.observe_thinking_fragment(&mut state, "Let me think", Instant::now(), 100);
    assert!(state.thinking_hold.is_active());
    state.thinking_unheard_reply = true;
    activity.observe_thinking_fragment(
        &mut state,
        "Let me think of an example: one",
        Instant::now(),
        200,
    );
    assert!(!state.thinking_hold.is_active());
    assert!(
        state.thinking_unheard_reply,
        "Gemini believes the dropped output was heard"
    );
}

#[test]
fn a_release_the_open_turn_never_answers_is_asked_for_once() {
    let released = Instant::now();
    let due = released + THINKING_REPLY_FALLBACK;
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(released);
    activity.arm_thinking_reply_fallback(released, "The candidate is ready.");
    assert_eq!(
        activity.claim_thinking_reply_fallback(&state, due - Duration::from_millis(1)),
        None
    );

    // Held, ended, or with Jim talking, it waits.
    for held in [
        RuntimeState {
            thinking_hold: ThinkingHold::Held {
                since: Instant::now(),
            },
            ..RuntimeState::default()
        },
        RuntimeState {
            ended: true,
            ..RuntimeState::default()
        },
    ] {
        assert_eq!(activity.claim_thinking_reply_fallback(&held, due), None);
    }
    activity.mark_speaking();
    assert_eq!(activity.claim_thinking_reply_fallback(&state, due), None);
    activity.mark_listening();

    assert_eq!(
        activity
            .claim_thinking_reply_fallback(&state, due)
            .as_deref(),
        Some("The candidate is ready.")
    );
    assert_eq!(
        activity.claim_thinking_reply_fallback(&state, due),
        None,
        "once"
    );
}

#[test]
fn output_or_more_speech_answers_a_release_before_its_fallback() {
    let released = Instant::now();
    let due = released + THINKING_REPLY_FALLBACK;
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(released);
    activity.arm_thinking_reply_fallback(released, "ready");
    activity.note_output(Instant::now());
    activity.note_turn_boundary(Instant::now());
    assert_eq!(activity.claim_thinking_reply_fallback(&state, due), None);

    activity.arm_thinking_reply_fallback(released, "ready");
    activity.note_candidate_finished(released, false);
    assert_eq!(activity.claim_thinking_reply_fallback(&state, due), None);
}

/// What the room loop does with a control packet, minus the room: note
/// whether a request was pending, apply it, and settle that request.
fn apply_control(
    activity: &mut RuntimeActivity,
    state: &mut RuntimeState,
    payload: &serde_json::Value,
) -> crate::agent::DataEventResult {
    let was_requested = state.thinking_hold.is_requested();
    let result = crate::agent::apply_data_event(state, crate::runtime::TOPIC_CONTROL, payload, 0.0);
    activity.settle_thinking_request(was_requested, state, &result);
    result
}

#[test]
fn a_reply_owed_after_a_discard_waits_for_the_floor_and_is_dropped_by_the_ending() {
    let mut activity = RuntimeActivity::new(Instant::now());
    for held in [
        RuntimeState {
            thinking_hold: ThinkingHold::Held {
                since: Instant::now(),
            },
            ..RuntimeState::default()
        },
        RuntimeState {
            ended: true,
            ..RuntimeState::default()
        },
    ] {
        activity.socket.reply_after_thinking_discard = true;
        assert!(!activity.claim_thinking_reply(&held));
        assert!(activity.socket.reply_after_thinking_discard, "still owed");
    }
    assert!(activity.claim_thinking_reply(&RuntimeState::default()));
}

#[test]
fn confirming_needs_a_pending_request_in_a_live_interview() {
    let mut activity = RuntimeActivity::new(Instant::now());
    let mut state = RuntimeState::default();
    activity.confirm_thinking_request(&mut state, Instant::now(), 1);
    assert_eq!(state.thinking_hold, ThinkingHold::Off, "nothing to confirm");
    assert_eq!(state.take_thinking_notice(), None);

    state.thinking_hold = ThinkingHold::Requested {
        since: Instant::now(),
    };
    state.ended = true;
    activity.confirm_thinking_request(&mut state, Instant::now(), 2);
    assert!(
        state.thinking_hold.is_requested(),
        "an ended interview holds nothing"
    );
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 0);
}

#[test]
fn the_watch_tick_declares_a_stale_request_and_drops_its_tentative_debt() {
    let asked = Instant::now();
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Requested { since: asked },
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(asked);
    activity.socket.reply_after_thinking_discard = true;
    let settled = asked + crate::agent::THINKING_REQUEST_SETTLE;
    activity.settle_stale_request(&mut state, settled - Duration::from_millis(1), 1);
    assert!(state.thinking_hold.is_requested());
    assert!(activity.socket.reply_after_thinking_discard);
    activity.settle_stale_request(&mut state, settled, 2);
    assert!(state.thinking_hold.is_declared());
    assert!(!activity.socket.reply_after_thinking_discard);
}

#[test]
fn a_hold_asked_for_aloud_is_owed_its_reply_inside_the_cooldown() {
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(Instant::now());
    let thinking = |on: bool| serde_json::json!({"type":"thinking","thinking":on});
    apply_control(&mut activity, &mut state, &thinking(true));
    assert!(
        apply_control(&mut activity, &mut state, &thinking(false))
            .generate_reply
            .is_some()
    );

    // A moment later the candidate asks aloud, and it is confirmed.
    activity.observe_thinking_fragment(&mut state, "Let me think", Instant::now(), 1);
    activity.confirm_thinking_request(&mut state, Instant::now(), 2);
    assert!(state.thinking_hold.is_declared());
    assert!(
        apply_control(&mut activity, &mut state, &thinking(false))
            .generate_reply
            .is_some(),
        "the button cooldown does not reach a hold asked for aloud"
    );
}

/// An operator's `CODETRIAL_GEMINI_REPLY_TIMEOUT_S` is the deadline the
/// watchdog holds for a generation that stalls. A reply nothing has started
/// toward is not waited on that long, whatever it is set to.
#[test]
fn the_configured_reply_timeout_is_the_one_the_watchdog_holds() {
    let start = Instant::now();
    let timeout = REPLY_TIMEOUT + Duration::from_secs(15);
    let mut activity = RuntimeActivity::new(start).with_reply_timeout(timeout);
    activity.note_candidate_finished(start, false);
    assert!(activity.reply_timed_out(start + FIRST_OUTPUT_DEADLINE, false));

    activity.note_output(Instant::now());
    let progress = activity.last_output_at.unwrap();
    assert!(!activity.reply_timed_out(progress + REPLY_TIMEOUT, false));
    assert!(activity.reply_timed_out(progress + timeout, false));
}

/// The configured first-output deadline replaces the default for a reply
/// nothing has started toward, and never outlasts the reply timeout.
#[test]
fn the_configured_first_output_deadline_is_capped_by_the_reply_timeout() {
    let start = Instant::now();
    let deadline = FIRST_OUTPUT_DEADLINE + Duration::from_secs(20);
    let mut activity = RuntimeActivity::new(start).with_first_output_deadline(deadline);
    activity.note_candidate_finished(start, false);
    assert!(!activity.reply_timed_out(start + FIRST_OUTPUT_DEADLINE, false));
    assert_eq!(
        activity.expired_reply_wait(start + deadline, false),
        Some(deadline)
    );

    let reply_timeout = deadline - Duration::from_secs(5);
    let mut capped = RuntimeActivity::new(start)
        .with_reply_timeout(reply_timeout)
        .with_first_output_deadline(deadline);
    capped.note_candidate_finished(start, false);
    assert_eq!(
        capped.expired_reply_wait(start + reply_timeout, false),
        Some(reply_timeout)
    );
}

/// A completed turn hands the floor to the candidate at once when nothing is
/// left to play, and waits on the queue while audio still plays; either way
/// the tool continuation it carried has arrived.
#[test]
fn a_completed_turn_waits_on_the_queue_only_while_audio_plays() {
    let now = Instant::now();
    for (playing, floor) in [(false, Floor::Listening), (true, Floor::AwaitingPlayout)] {
        let mut activity = RuntimeActivity::new(now);
        activity.note_tool_response(now);
        activity.floor = Floor::Speaking;
        activity.settle_completed_turn(playing);
        assert_eq!(activity.floor, floor, "playing={playing}");
        assert!(!activity.socket.tool_response_outstanding);
    }
}

/// Only the tail of a turn still playing out is spared a reply deadline. With
/// the floor already the candidate's, audio still draining from an earlier
/// cut is no reason to leave their new answer unowed.
#[test]
fn audio_still_draining_spares_no_deadline_once_the_floor_is_the_candidates() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    assert_eq!(activity.floor, Floor::Listening);
    activity.note_candidate_finished(now, true);
    assert!(activity.reply_in_flight());
}

#[test]
fn voice_holds_the_silence_nudge_until_the_socket_transcribes() {
    let start = Instant::now();
    let now = start + Duration::from_secs(300);
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_voice(now - Duration::from_secs(5));
    assert!(activity.watch_prompt(&mut state, now).is_none());

    // Once the socket has transcribed the candidate, the transcript is the
    // evidence and a voice level alone, room noise included, holds nothing.
    let mut activity = RuntimeActivity::new(start);
    activity.transcribed_on_socket = true;
    activity.note_candidate_voice(now - Duration::from_secs(5));
    assert!(activity.watch_prompt(&mut state, now).is_some());
}

#[test]
fn a_replacement_socket_has_to_transcribe_the_candidate_again() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    activity.note_candidate_voice(start + Duration::from_secs(5));
    assert_eq!(activity.last_user_speech, start);

    activity.reset_context_observations(true);
    activity.note_candidate_voice(start + Duration::from_secs(9));
    assert_eq!(activity.last_user_speech, start + Duration::from_secs(9));
}

/// The phase judge follows the answer that earned a tick within seconds, not
/// the interim review's minutes: it waits only for the candidate to stop
/// talking and typing, and for its own short cooldown. It does not wait for the
/// floor, since it takes no turn. It spends nothing when nothing new was said
/// or every step it records is closed, and it stops at its ceiling.
#[test]
fn the_phase_judge_follows_a_candidate_turn_within_seconds() {
    let start = Instant::now();
    let spoke = || RuntimeState {
        transcript: vec!["Candidate: so I merge the overlapping intervals".to_string()],
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    activity.floor = Floor::Speaking;
    assert!(
        !activity.claim_phase_judge(&spoke(), start + PHASE_JUDGE_IDLE / 2),
        "the candidate may still be talking"
    );
    let idle = start + PHASE_JUDGE_IDLE;
    assert!(
        !activity.claim_phase_judge(&RuntimeState::default(), idle),
        "nothing said, nothing to judge"
    );
    assert!(
        activity.claim_phase_judge(&spoke(), idle),
        "the interviewer speaking does not hold it back"
    );
    assert!(
        !activity.claim_phase_judge(&spoke(), idle + PHASE_JUDGE_COOLDOWN / 2),
        "the cooldown is stamped where it is read"
    );
    assert!(activity.claim_phase_judge(&spoke(), idle + PHASE_JUDGE_COOLDOWN));

    // Each of these holds refuses it on its own, whatever else is settled.
    let mut closing = spoke();
    closing.end_requested = true;
    assert!(
        !activity.claim_phase_judge(&closing, idle + PHASE_JUDGE_COOLDOWN * 3),
        "the interviewer has asked to close"
    );
    let typing_at = idle + PHASE_JUDGE_COOLDOWN * 3;
    activity.last_code_change = typing_at - PHASE_JUDGE_IDLE / 2;
    assert!(
        !activity.claim_phase_judge(&spoke(), typing_at),
        "the candidate is still typing"
    );
    activity.last_code_change = start;

    activity.phase_judges = MAX_PHASE_JUDGES;
    assert!(
        !activity.claim_phase_judge(&spoke(), idle + PHASE_JUDGE_COOLDOWN * 4),
        "past the ceiling the interviewer's own calls remain"
    );
    assert!(!activity.reserve_phase_judge(idle));
    activity.phase_judges = MAX_PHASE_JUDGES - 1;
    assert!(activity.reserve_phase_judge(idle));
    assert_eq!(activity.phase_judges, MAX_PHASE_JUDGES);
    assert_eq!(activity.last_phase_judge, Some(idle));
    assert!(!activity.reserve_phase_judge(idle + PHASE_JUDGE_COOLDOWN));
}

/// Without the evidence tool only the judge records STAR, so the coding
/// round's routine judgments must not spend the calls a behavioral round still
/// to come needs. The end and a round transition may: after either nothing is
/// left to keep them for. Nothing is held back once that round has opened,
/// been refused, or was never planned.
#[test]
fn the_coding_round_leaves_judgments_for_the_behavioral_round() {
    let start = Instant::now();
    let spoke = |state: RuntimeState| RuntimeState {
        transcript: vec!["Candidate: so I merge the overlapping intervals".to_string()],
        ..state
    };
    let coding = spoke(RuntimeState::default());
    assert_eq!(coding.interview_loop, InterviewLoop::CodingBehavioral);
    let at = start + PHASE_JUDGE_IDLE;
    let mut activity = RuntimeActivity::new(start);
    activity.phase_judges = MAX_PHASE_JUDGES - BEHAVIORAL_PHASE_JUDGES;
    assert!(
        !activity.claim_phase_judge(&coding, at),
        "the rest is the behavioral round's"
    );
    assert!(
        activity.reserve_phase_judge(at),
        "the end or a transition may use it"
    );
    for state in [
        RuntimeState {
            behavioral_round_started: true,
            round_transition_seen: true,
            ..RuntimeState::default()
        },
        RuntimeState {
            round_transition_seen: true,
            ..RuntimeState::default()
        },
        RuntimeState {
            interview_loop: InterviewLoop::CodingOnly,
            ..RuntimeState::default()
        },
        // A whiteboard interviewer records STAR itself.
        RuntimeState {
            interview_mode: crate::agent::InterviewMode::Whiteboard,
            ..RuntimeState::default()
        },
    ] {
        let mut activity = RuntimeActivity::new(start);
        activity.phase_judges = MAX_PHASE_JUDGES - BEHAVIORAL_PHASE_JUDGES;
        assert!(activity.claim_phase_judge(&spoke(state), at));
    }
}

/// Typing pauses every few seconds while Coding is open, so a judgment asked
/// for by the editor alone waits out a longer cooldown and takes at most its
/// own share of the ceiling. What the candidate says is still judged at the
/// ordinary pace, and the editor cap does not hold it back.
#[test]
fn editor_only_judgments_are_paced_and_capped_apart_from_speech() {
    let problem = crate::agent::get_problem(Some("two-sum"));
    let start = Instant::now();
    let mut state = RuntimeState {
        code: "def solve(nums):\n    return sorted(nums)\n".to_string(),
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(start);
    let mut at = start + PHASE_JUDGE_IDLE;

    // Each accepted claim takes its window, as the room does, so nothing is due
    // again until the candidate types or speaks.
    let mut claim = |state: &mut RuntimeState, at: Instant| {
        let claimed = activity.claim_phase_judge(state, at);
        if claimed {
            crate::agent::take_phase_judge_window(state, problem);
            assert_eq!(crate::agent::phase_judge_need(state), PhaseJudgeDue::No);
        }
        (claimed, activity.editor_phase_judges)
    };
    let edit = |state: &mut RuntimeState| {
        state.code.push_str("# more\n");
        assert_eq!(
            crate::agent::phase_judge_need(state),
            PhaseJudgeDue::EditorOnly,
            "an edit the judge has not read reopens it"
        );
    };

    assert_eq!(
        crate::agent::phase_judge_need(&state),
        PhaseJudgeDue::EditorOnly
    );
    assert_eq!(claim(&mut state, at), (true, 1));
    edit(&mut state);
    at += PHASE_JUDGE_COOLDOWN;
    assert_eq!(
        claim(&mut state, at),
        (false, 1),
        "a typing pause alone waits the editor cooldown"
    );
    at += PHASE_JUDGE_EDITOR_COOLDOWN;
    assert_eq!(claim(&mut state, at), (true, 2));

    // Speech is judged at the ordinary pace, between editor judgments, and is
    // not counted against the editor's share.
    state
        .transcript
        .push("Candidate: it is O of n time".to_string());
    at += PHASE_JUDGE_COOLDOWN;
    assert_eq!(
        crate::agent::phase_judge_need(&state),
        PhaseJudgeDue::Spoken
    );
    assert_eq!(claim(&mut state, at), (true, 2));

    // The editor's share runs out on real claims, each reopened by its own
    // edit, then speech still has the rest of the ceiling.
    for taken in 3..=MAX_EDITOR_PHASE_JUDGES {
        edit(&mut state);
        at += PHASE_JUDGE_EDITOR_COOLDOWN;
        assert_eq!(claim(&mut state, at), (true, taken));
    }
    edit(&mut state);
    at += PHASE_JUDGE_EDITOR_COOLDOWN;
    assert_eq!(
        claim(&mut state, at),
        (false, MAX_EDITOR_PHASE_JUDGES),
        "the editor's share is spent"
    );
    state
        .transcript
        .push("Candidate: and constant extra space".to_string());
    assert_eq!(claim(&mut state, at), (true, MAX_EDITOR_PHASE_JUDGES));
}

/// The provider's tool-assisted turn summed its model invocations: the next
/// single-generation prompt shrank without compression. These are observed
/// counts from a controlled provider-default probe, not an invented cut.
#[test]
fn tool_generation_sums_do_not_schedule_a_false_checkpoint() {
    let pair = window(20_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[4210]);
    activity.note_tool_response(Instant::now());
    let usage = crate::gemini::TokenUsage {
        prompt: 12_494,
        samples: 1,
        ..Default::default()
    };
    activity.account_live_usage("tool-probe", "0:00.000", Some("tool"), pair, usage);
    activity.tokens.observe_turn_complete(pair);
    assert_eq!(activity.live_usage.prompt, 12_494);
    assert_eq!(activity.tokens.peak_prompt_tokens, 4210);
    complete_turn(&mut activity, pair, &[8337]);
    assert!(!activity.tokens.context_refresh_pending);
    complete_turn(&mut activity, pair, &[19_000]);
    complete_turn(&mut activity, pair, &[8_000]);
    assert!(activity.tokens.context_refresh_pending);
}

#[test]
fn a_tool_response_marks_usage_even_when_the_input_cause_changes() {
    let pair = window(20_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[4210]);
    activity.note_tool_response(Instant::now());
    activity.account_live_usage(
        "tool-probe",
        "0:00.000",
        Some("turn"),
        pair,
        crate::gemini::TokenUsage {
            prompt: 12_494,
            samples: 1,
            ..Default::default()
        },
    );
    activity.tokens.observe_turn_complete(pair);
    complete_turn(&mut activity, pair, &[8337]);
    assert!(!activity.tokens.context_refresh_pending);
    activity.note_tool_response(Instant::now());
    activity.reset_context_observations(true);
    complete_turn(&mut activity, pair, &[20_000]);
    complete_turn(&mut activity, pair, &[8_000]);
    assert!(activity.tokens.context_refresh_pending);
}

#[test]
fn tool_cause_alone_marks_an_aggregate_usage_observation() {
    let pair = window(20_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[4210]);
    activity.account_live_usage(
        "tool-probe",
        "0:00.000",
        Some("tool"),
        pair,
        crate::gemini::TokenUsage {
            prompt: 12_494,
            samples: 1,
            ..Default::default()
        },
    );
    activity.tokens.observe_turn_complete(pair);
    complete_turn(&mut activity, pair, &[8337]);
    assert!(!activity.tokens.context_refresh_pending);
    assert_eq!(activity.live_usage.prompt, 12_494);
}

#[test]
fn a_tool_total_below_the_previous_single_prompt_can_prove_a_cut() {
    let pair = window(32_000, 16_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[31_352]);
    activity.note_tool_response(Instant::now());
    complete_turn(&mut activity, pair, &[27_251]);
    assert!(activity.tokens.context_refresh_pending);

    activity.tokens.context_refresh_pending = false;
    complete_turn(&mut activity, pair, &[13_700]);
    assert!(!activity.tokens.context_refresh_pending);
}

#[test]
fn usage_before_a_tool_call_does_not_poison_the_comparable_peak() {
    let pair = window(20_000, 8_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[4210]);
    activity.tokens.observe_prompt_tokens(pair, 12_494);
    activity.note_tool_response(Instant::now());
    complete_turn(&mut activity, pair, &[12_500]);
    complete_turn(&mut activity, pair, &[8337]);
    assert!(!activity.tokens.context_refresh_pending);
}

#[test]
fn a_tool_cut_comparison_waits_for_all_usage_observations() {
    let pair = window(32_000, 16_000);
    let mut activity = RuntimeActivity::new(Instant::now());
    complete_turn(&mut activity, pair, &[31_352]);
    activity.note_tool_response(Instant::now());
    activity.tokens.observe_prompt_tokens(pair, 5000);
    assert!(!activity.tokens.context_refresh_pending);
    complete_turn(&mut activity, pair, &[40_000]);
    assert!(!activity.tokens.context_refresh_pending);
    complete_turn(&mut activity, pair, &[32_000]);
    assert!(!activity.tokens.context_refresh_pending);
}

#[test]
fn only_a_resumed_socket_retains_the_tool_cut_reference() {
    let pair = window(32_000, 16_000);
    for resumed in [false, true] {
        let mut activity = RuntimeActivity::new(Instant::now());
        complete_turn(&mut activity, pair, &[31_352]);
        activity.reset_context_observations(resumed);
        activity.note_tool_response(Instant::now());
        complete_turn(&mut activity, pair, &[27_251]);
        assert_eq!(activity.tokens.context_refresh_pending, resumed);
    }
}

#[test]
fn provider_activity_holds_prompts_until_speech_ends_after_prior_transcription() {
    let start = Instant::now();
    let now = start + Duration::from_secs(300);
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    activity.transcribed_on_socket = true;
    activity.note_candidate_voice_activity(true, now);
    let end = now + Duration::from_secs(82);
    assert!(activity.watch_prompt(&mut state, end).is_none());
    activity.tokens.context_refresh_pending = true;
    assert!(!activity.can_refresh_context(false, false, end));
    assert!(!activity.interim_review_due(&state, end));
    activity.note_candidate_voice_activity(false, end);
    assert!(
        activity
            .watch_prompt(&mut state, end + Duration::from_secs(5))
            .is_none()
    );
    assert!(
        activity
            .watch_prompt(&mut state, end + Duration::from_secs(300))
            .is_some()
    );
}

#[test]
fn replacement_does_not_inherit_provider_activity_even_when_resumed() {
    for resumed in [false, true] {
        let now = Instant::now();
        let mut activity = RuntimeActivity::new(now);
        activity.note_candidate_voice_activity(true, now);
        activity.reset_context_observations(resumed);
        assert!(!activity.candidate_voice_active);
    }
}

fn completed_coding_idle_state(start: Instant) -> RuntimeState {
    let mut state = RuntimeState {
        started_at: start,
        code_execution_disabled: true,
        code: "def solve(): return 1".into(),
        ..RuntimeState::default()
    };
    for phase in [
        "repeat",
        "example",
        "algorithm",
        "coding",
        "test",
        "optimizations",
    ] {
        crate::agent::record_framework_evidence(
            &mut state,
            &serde_json::json!({
            "phase":phase,"source":"candidate_speech","kind":"observed",
            "confidence":90,"summary":"Candidate completed this step."}),
        )
        .unwrap();
    }
    state
}

#[test]
fn completed_coding_idle_backoff_keeps_first_then_doubles_to_an_eight_minute_cap() {
    let start = Instant::now();
    let mut state = completed_coding_idle_state(start);
    let mut activity = RuntimeActivity::new(start);
    activity.last_nudge = start - Duration::from_secs(30);
    assert!(
        activity
            .watch_prompt(&mut state, start + Duration::from_secs(24))
            .is_none()
    );
    let first = activity
        .watch_prompt(&mut state, start + Duration::from_secs(25))
        .unwrap();
    assert!(first.completed_coding_nudge && !first.allows_silence);
    activity.commit_watch_prompt(&first, start + Duration::from_secs(25));
    assert!(
        activity
            .watch_prompt(&mut state, start + Duration::from_secs(84))
            .is_none()
    );
    let second = activity
        .watch_prompt(&mut state, start + Duration::from_secs(85))
        .unwrap();
    activity.commit_watch_prompt(&second, start + Duration::from_secs(85));
    assert!(
        activity
            .watch_prompt(&mut state, start + Duration::from_secs(204))
            .is_none()
    );
    let third = activity
        .watch_prompt(&mut state, start + Duration::from_secs(205))
        .unwrap();
    activity.commit_watch_prompt(&third, start + Duration::from_secs(205));
    let mut sent = 205;
    for gap in [240, 480, 480] {
        assert!(
            activity
                .watch_prompt(&mut state, start + Duration::from_secs(sent + gap - 1))
                .is_none()
        );
        let next = activity
            .watch_prompt(&mut state, start + Duration::from_secs(sent + gap))
            .unwrap();
        activity.commit_watch_prompt(&next, start + Duration::from_secs(sent + gap));
        sent += gap;
    }
}

#[test]
fn completed_coding_idle_backoff_preserves_editor_reviews_and_unfinished_coding() {
    let start = Instant::now();
    let mut state = completed_coding_idle_state(start);
    let mut activity = RuntimeActivity::new(start);
    activity.completed_coding_nudges = 2;
    activity.last_successful_completed_nudge = Some(start);
    activity.last_nudge = start;
    state.evidence_ledger.code.substantive_revision = 1;
    state.evidence_ledger.code.parser_observation = Some(crate::agent::CodeObservation::Parsed);
    let review = activity
        .watch_prompt(&mut state, start + Duration::from_secs(45))
        .unwrap();
    assert!(review.allows_silence && !review.completed_coding_nudge);
    for missing in [false, true] {
        let mut state = completed_coding_idle_state(start);
        if missing {
            state.framework_evidence.pop();
        } else {
            state.framework_evidence[0].kind = crate::agent::EvidenceKind::Skipped;
        }
        let mut activity = RuntimeActivity::new(start);
        activity.completed_coding_nudges = 2;
        let nudge = activity
            .watch_prompt(&mut state, start + Duration::from_secs(30))
            .unwrap();
        assert!(!nudge.completed_coding_nudge && !nudge.allows_silence);
    }
}

#[test]
fn completed_coding_idle_backoff_resets_on_candidate_work_not_socket_or_bookkeeping() {
    let start = Instant::now();
    let mut state = completed_coding_idle_state(start);
    let mut activity = RuntimeActivity::new(start);
    activity.completed_coding_nudges = 2;
    activity.reset_context_observations(true);
    assert_eq!(activity.completed_coding_nudges, 2);
    activity.reset_context_observations(false);
    assert_eq!(activity.completed_coding_nudges, 2);
    let previous_revision = state.evidence_ledger.code.revision;
    activity.observe_candidate_work(&state, previous_revision, state.test_runs);
    assert_eq!(activity.completed_coding_nudges, 2);
    state
        .framework_evidence
        .push(state.framework_evidence[0].clone());
    activity.observe_candidate_work(&state, previous_revision, state.test_runs);
    assert_eq!(activity.completed_coding_nudges, 2);
    state.evidence_ledger.code.revision += 1;
    activity.observe_candidate_work(&state, previous_revision, state.test_runs);
    assert_eq!(activity.completed_coding_nudges, 0);
    activity.completed_coding_nudges = 2;
    state.test_runs += 1;
    activity.observe_candidate_work(
        &state,
        state.evidence_ledger.code.revision,
        state.test_runs - 1,
    );
    assert_eq!(activity.completed_coding_nudges, 0);
    activity.completed_coding_nudges = 2;
    activity.note_candidate_voice_activity(true, start);
    assert_eq!(activity.completed_coding_nudges, 0);
    activity.completed_coding_nudges = 2;
    activity.note_candidate_finished(start, false);
    assert_eq!(activity.completed_coding_nudges, 0);
}

#[test]
fn completed_coding_idle_backoff_preserves_behavioral_and_hold_voice_gates() {
    let start = Instant::now();
    let mut state = completed_coding_idle_state(start);
    let mut activity = RuntimeActivity::new(start);
    activity.completed_coding_nudges = 2;
    // Declared just now, so the check-in is not what answers this tick.
    state.thinking_hold = ThinkingHold::Held {
        since: start + Duration::from_secs(300),
    };
    assert!(
        activity
            .watch_prompt(&mut state, start + Duration::from_secs(300))
            .is_none()
    );
    state.thinking_hold = ThinkingHold::Off;
    activity.note_candidate_voice_activity(true, start + Duration::from_secs(300));
    assert!(
        activity
            .watch_prompt(&mut state, start + Duration::from_secs(301))
            .is_none()
    );
    activity.note_candidate_voice_activity(false, start + Duration::from_secs(301));
    state.behavioral_round_started = true;
    let nudge = activity
        .watch_prompt(&mut state, start + Duration::from_secs(331))
        .unwrap();
    assert!(nudge.behavioral_nudge && !nudge.completed_coding_nudge);
    activity.behavioral_nudged = true;
    assert!(
        activity
            .watch_prompt(&mut state, start + Duration::from_secs(600))
            .is_none()
    );
}

#[test]
fn completed_coding_idle_backoff_observes_real_reducer_edits_and_test_receipts() {
    let now = Instant::now();
    let mut state = completed_coding_idle_state(now);
    let mut activity = RuntimeActivity::new(now);
    let previous = state.code.clone();
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({"code":previous,"language":"python"}),
        99.0,
    );
    // Seed the authoritative ledger using a changed accepted packet.
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({"code":format!("{previous} "),"language":"python"}),
        99.0,
    );
    let previous = state.code.clone();
    let previous_revision = state.evidence_ledger.code.revision;
    let runs = state.test_runs;
    activity.completed_coding_nudges = 2;
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({"code":previous,"language":"python"}),
        99.0,
    );
    activity.observe_candidate_work(&state, previous_revision, runs);
    assert_eq!(
        activity.completed_coding_nudges, 2,
        "identical accepted edit is no new episode"
    );
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({"code":format!("{previous}\n"),"language":"python"}),
        99.0,
    );
    activity.observe_candidate_work(&state, previous_revision, runs);
    assert_eq!(activity.completed_coding_nudges, 0);
    let previous_revision = state.evidence_ledger.code.revision;
    state.code_execution_disabled = false;
    activity.completed_coding_nudges = 2;
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_TEST_RESULTS,
        &serde_json::json!({"passed":0,"total":1,"language":"python"}),
        99.0,
    );
    assert!(state.test_runs > runs);
    activity.observe_candidate_work(&state, previous_revision, runs);
    assert_eq!(activity.completed_coding_nudges, 0);
}

#[test]
fn completed_coding_idle_backoff_ignores_duplicate_inactive_vad_edges() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    activity.completed_coding_nudges = 2;
    activity.note_candidate_voice_activity(false, now);
    activity.note_candidate_voice_activity(false, now);
    assert_eq!(activity.completed_coding_nudges, 2);
    activity.note_candidate_voice_activity(true, now);
    assert_eq!(activity.completed_coding_nudges, 0);
    activity.completed_coding_nudges = 2;
    activity.note_candidate_voice_activity(false, now);
    assert_eq!(activity.completed_coding_nudges, 0);
}

#[test]
fn completed_coding_idle_backoff_tracks_language_only_revision_and_rejects_malformed_edits() {
    let now = Instant::now();
    let mut state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(now);
    let code = "value = 1\n";
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({"code":code,"language":"python"}),
        99.0,
    );
    let previous = state.evidence_ledger.code.revision;
    activity.completed_coding_nudges = 2;
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({"code":code,"language":"javascript"}),
        99.0,
    );
    assert!(state.evidence_ledger.code.revision > previous);
    activity.observe_candidate_work(&state, previous, state.test_runs);
    assert_eq!(activity.completed_coding_nudges, 0);
    let previous = state.evidence_ledger.code.revision;
    activity.completed_coding_nudges = 2;
    crate::agent::apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({"language":"python"}),
        99.0,
    );
    activity.observe_candidate_work(&state, previous, state.test_runs);
    assert_eq!(activity.completed_coding_nudges, 2);
}

#[tokio::test]
async fn completed_coding_idle_later_failed_send_retries_after_thirty_without_spending_success() {
    let start = Instant::now() - Duration::from_secs(210);
    let mut state = completed_coding_idle_state(start);
    let mut activity = RuntimeActivity::new(start);
    let first = activity
        .watch_prompt(&mut state, start + Duration::from_secs(30))
        .unwrap();
    activity.commit_watch_prompt(&first, start + Duration::from_secs(30));
    let second = activity
        .watch_prompt(&mut state, start + Duration::from_secs(90))
        .unwrap();
    activity.commit_watch_prompt(&second, start + Duration::from_secs(90));
    let failed = activity
        .watch_prompt(&mut state, start + Duration::from_secs(210))
        .unwrap();
    assert!(
        crate::livekit::send_watched_prompt(&mut activity, &failed, async {
            Err::<(), _>(std::io::Error::other("closed socket"))
        })
        .await
        .is_err()
    );
    activity.unsend_watch_prompt(&mut state);
    assert_eq!(activity.completed_coding_nudges, 2);
    assert_eq!(
        activity.last_successful_completed_nudge,
        Some(start + Duration::from_secs(90))
    );
    assert!(
        activity
            .watch_prompt(&mut state, start + Duration::from_secs(239))
            .is_none()
    );
    let retry = activity
        .watch_prompt(&mut state, start + Duration::from_secs(240))
        .unwrap();
    assert!(retry.completed_coding_nudge);
    assert_eq!(activity.completed_coding_nudges, 2);
}

#[test]
fn incoming_speech_defers_output_wait_and_gets_a_fresh_reply_budget_after_end() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("resume"), false);
    activity.note_candidate_voice_activity(true, start);
    for seconds in 0..=180 {
        let now = start + Duration::from_secs(seconds);
        activity.note_candidate_voice(now);
        assert!(!activity.reply_timed_out(now, false));
        assert!(!activity.reply_visibly_late(now, false));
        assert!(!activity.generation_stalled(now));
        let stalls = activity.settle_stalls(now, false);
        assert!(!stalls.prompt_released && !stalls.spend_restart);
    }
    let end = start + Duration::from_secs(181);
    activity.note_candidate_voice_activity(false, end);
    assert_eq!(activity.prompted_at, Some(end));
    assert_eq!(activity.prompt_text.as_deref(), Some("resume"));
    assert!(!activity.reply_timed_out(
        end + FIRST_OUTPUT_DEADLINE - Duration::from_millis(1),
        false
    ));
    assert!(activity.reply_timed_out(end + FIRST_OUTPUT_DEADLINE, false));
}

#[test]
fn stale_vad_edges_without_pcm_do_not_keep_renewing_reply_debt() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("resume"), false);
    activity.note_candidate_voice_activity(true, start);
    activity.note_candidate_voice(start);
    let quiet = start + CHECKPOINT_VOICE_QUIET;
    activity.settle_stalls(quiet, false);
    assert_eq!(activity.prompted_at, Some(quiet));
    for seconds in 3..=60 {
        let now = start + Duration::from_secs(seconds);
        activity.note_candidate_voice_activity(true, now);
        activity.note_candidate_voice_activity(false, now);
        activity.settle_stalls(now, false);
        assert_eq!(activity.prompted_at, Some(quiet));
    }
    assert!(activity.reply_timed_out(start + Duration::from_secs(60), false));
}

#[test]
fn incoming_speech_rebases_all_owed_clocks_once_and_reset_discards_acoustic_history() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("resume"), false);
    activity.awaiting_reply_since = Some(start);
    activity.note_tool_response(start);
    activity.generating = true;
    activity.last_output_at = Some(start);
    activity.note_candidate_voice_activity(true, start);
    activity.note_candidate_voice(start);
    let end = start + Duration::from_secs(1);
    activity.note_candidate_voice_activity(false, end);
    assert_eq!(activity.prompted_at, Some(end));
    assert_eq!(activity.awaiting_reply_since, Some(end));
    assert_eq!(activity.tool_response_at, Some(end));
    assert_eq!(activity.last_output_at, Some(end));
    activity.note_candidate_voice_activity(false, end + Duration::from_secs(1));
    assert_eq!(activity.prompted_at, Some(end));
    activity.clear_candidate_voice_activity();
    activity.note_candidate_voice_activity(true, end + Duration::from_secs(2));
    assert!(!activity.incoming_speech_active(end + Duration::from_secs(2)));
    assert!(activity.reply_timed_out(end + activity.reply_timeout, false));
}

#[test]
fn pcm_before_vad_rebases_short_speech_once_without_reusing_noise_edges() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("resume"), false);
    let speech = start + Duration::from_secs(60);
    activity.note_candidate_voice(speech);
    activity.note_candidate_voice_activity(true, speech + Duration::from_millis(10));
    let end = speech + Duration::from_millis(100);
    activity.note_candidate_voice_activity(false, end);
    assert_eq!(activity.prompted_at, Some(end));
    assert!(!activity.reply_timed_out(end, false));
    for millis in [200, 300, 500] {
        let edge = speech + Duration::from_millis(millis);
        activity.note_candidate_voice_activity(true, edge);
        activity.note_candidate_voice_activity(false, edge);
        assert_eq!(activity.prompted_at, Some(end));
    }
    assert!(activity.reply_timed_out(end + activity.reply_timeout, false));
}

#[test]
fn incoming_pcm_without_vad_does_not_suspend_stalled_generation() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.generating = true;
    activity.last_output_at = Some(start);
    let late = start + activity.reply_timeout;
    activity.note_candidate_voice(late);
    assert!(!activity.incoming_speech_active(late));
    assert!(activity.generation_stalled(late));
    assert!(activity.reply_timed_out(late, false));
    activity.note_candidate_voice_activity(true, late);
    assert!(!activity.generation_stalled(late));
    assert!(!activity.reply_timed_out(late, false));
    activity.note_candidate_voice_activity(false, late + Duration::from_secs(1));
    assert_eq!(activity.last_output_at, Some(late + Duration::from_secs(1)));
    assert!(activity.reply_timed_out(
        late + Duration::from_secs(1) + activity.reply_timeout,
        false
    ));
}

/// A speech-start whose end the provider never sent stops holding the
/// interview back once neither the microphone nor the provider renews it, and
/// voiced frames keep a real long answer alive past the same bound.
#[test]
fn a_speech_start_without_its_end_expires_once_its_evidence_is_stale() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_voice_activity(true, start);
    let before = start + STALE_VOICE_ACTIVITY - Duration::from_millis(1);
    activity.settle_stalls(before, false);
    assert!(activity.candidate_voice_active);

    let stale = start + STALE_VOICE_ACTIVITY;
    activity.settle_stalls(stale, false);
    assert!(!activity.candidate_voice_active);
    assert!(activity.candidate_transcription_pending(stale));
    assert!(!activity.candidate_transcription_pending(stale + CHECKPOINT_VOICE_QUIET));

    let mut talking = RuntimeActivity::new(start);
    talking.note_candidate_voice_activity(true, start);
    let mut at = start;
    while at < start + STALE_VOICE_ACTIVITY * 2 {
        at += Duration::from_millis(500);
        talking.note_candidate_voice(at);
        talking.settle_stalls(at, false);
    }
    assert!(talking.candidate_voice_active);
}

/// Continuity rides beside an input only while the next one could be cut:
/// before anything is measured, once a cut is detected, and in the upper half
/// of the band between target and trigger. Without compression, never.
#[test]
fn continuity_is_due_only_while_a_context_cut_is_plausible() {
    let compression = Some(crate::config::GeminiContextCompression {
        trigger_tokens: 24_000,
        target_tokens: 12_000,
    });
    let mut activity = RuntimeActivity::new(Instant::now());
    assert!(!activity.tokens.context_cut_plausible(None));
    assert!(activity.tokens.context_cut_plausible(compression));

    activity.tokens.observe_prompt_tokens(compression, 12_500);
    activity.tokens.observe_turn_complete(compression);
    assert!(!activity.tokens.context_cut_plausible(compression));

    activity.tokens.observe_prompt_tokens(compression, 17_999);
    activity.tokens.observe_turn_complete(compression);
    assert!(!activity.tokens.context_cut_plausible(compression));

    activity.tokens.observe_prompt_tokens(compression, 18_000);
    activity.tokens.observe_turn_complete(compression);
    assert!(activity.tokens.context_cut_plausible(compression));

    activity.tokens.observe_prompt_tokens(compression, 12_100);
    activity.tokens.observe_turn_complete(compression);
    assert!(activity.tokens.context_refresh_pending);
    assert!(activity.tokens.context_cut_plausible(compression));
    activity.tokens.context_refresh_pending = false;
    assert!(!activity.tokens.context_cut_plausible(compression));

    // Growth through a tool turn keeps no comparable count, but it can still
    // bring the context back toward the trigger.
    activity.tokens.observe_prompt_tokens(compression, 13_000);
    activity.tokens.observe_turn_complete(compression);
    activity.note_tool_response(Instant::now());
    activity.tokens.observe_prompt_tokens(compression, 19_000);
    activity.tokens.observe_turn_complete(compression);
    assert!(!activity.tokens.context_refresh_pending);
    assert!(activity.tokens.context_cut_plausible(compression));

    // The next comparable count replaces the bound.
    activity.tokens.observe_prompt_tokens(compression, 13_500);
    activity.tokens.observe_turn_complete(compression);
    assert!(!activity.tokens.context_cut_plausible(compression));
}

/// Voiced frames owe the unfinished reply a wait only while the provider also
/// reports speech; frames alone may be room noise.
#[test]
fn voiced_frames_without_provider_speech_owe_no_wait() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    activity.mark_prompted(now, Some("question"), false);
    assert!(activity.reply_unfinished());
    activity.note_candidate_voice(now);
    assert!(!activity.incoming_speech_owed);
    activity.note_candidate_voice_activity(true, now);
    activity.note_candidate_voice(now);
    assert!(activity.incoming_speech_owed);
}

/// A tool review of a revision already reviewed moves no clocks; the
/// settle window is half-open, so silent delivery is due at exactly
/// `CODE_SETTLE`; and syncing writes the plausibility the state reads.
#[test]
fn review_settling_silent_delivery_and_cut_sync_hold_at_their_edges() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    activity.substantive_revision_at_last_review = 3;
    let reviewed = activity.last_review;
    let interjected = activity.last_interjection;
    activity.settle_tool_editor_review(Some(3), true, now + Duration::from_secs(5));
    assert_eq!(activity.last_review, reviewed);
    assert_eq!(activity.last_interjection, interjected);
    activity.settle_tool_editor_review(Some(4), true, now + Duration::from_secs(5));
    assert_eq!(activity.last_review, now + Duration::from_secs(5));

    let mut state = RuntimeState {
        language: "python".into(),
        code: "def solve():\n    return 1\n".into(),
        ..RuntimeState::default()
    };
    state.evidence_ledger.code.parser_observation = Some(crate::agent::CodeObservation::Parsed);
    state.evidence_ledger.code.substantive_revision = 1;
    let mut speaking = RuntimeActivity::new(now);
    speaking.last_code_change = now;
    speaking.note_candidate_voice_activity(true, now + CODE_SETTLE);
    speaking.note_candidate_voice(now + CODE_SETTLE);
    assert!(
        speaking
            .pending_silent_editor(&state, now + CODE_SETTLE)
            .is_some()
    );

    state.context_compression = Some(crate::config::GeminiContextCompression {
        trigger_tokens: 24_000,
        target_tokens: 12_000,
    });
    activity
        .tokens
        .observe_prompt_tokens(state.context_compression, 12_500);
    activity
        .tokens
        .observe_turn_complete(state.context_compression);
    activity.sync_cut_plausible(&mut state);
    assert!(!state.context_cut_plausible);
}

#[test]
fn latency_recovery_prompt_keeps_the_wait_and_runtime_prompt_debt() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.latency.voice(start);
    activity.live_socket = 2;
    activity.reset_context_observations(true);
    activity.mark_recovery_prompted(start + Duration::from_secs(10), Some("owed"));
    let record = activity
        .latency
        .audio(start + Duration::from_secs(11))
        .unwrap();
    assert_eq!(record["origin"], "last_voiced_pcm");
    assert_eq!(record["elapsed_ms"], 11_000);
    assert!(activity.owes_prompt());
    assert_eq!(activity.prompt_text.as_deref(), Some("owed"));
}

#[test]
fn a_late_transcript_does_not_seed_the_next_latency_trace() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.latency.voice(start);
    activity.latency.audio(start + Duration::from_millis(500));
    let ended = start + Duration::from_secs(1);
    activity
        .latency
        .boundary(super::super::latency::Boundary::TurnComplete, ended);
    activity.turn_completed_at = Some(ended);
    activity.note_latency_transcript(ended + Duration::from_millis(500));
    let record = activity
        .latency
        .audio(ended + Duration::from_secs(30))
        .unwrap();
    assert_eq!(record["origin"], "provider_event");
    assert_eq!(record["elapsed_ms"], 0);
}

#[test]
fn either_confirmation_path_closes_a_provisional_holds_latency_trace() {
    for timed_out in [false, true] {
        let start = Instant::now();
        let mut activity = RuntimeActivity::new(start);
        activity.latency.voice(start);
        let mut state = RuntimeState {
            thinking_hold: ThinkingHold::Requested { since: start },
            ..RuntimeState::default()
        };
        let declared = start + crate::agent::THINKING_REQUEST_SETTLE;
        if timed_out {
            activity.settle_stale_request(&mut state, declared, 1);
        } else {
            activity.confirm_thinking_request(&mut state, declared, 1);
        }
        assert!(state.thinking_hold.is_active());
        assert!(state.end_thinking(2));
        let released = declared + Duration::from_secs(45);
        activity.note_latency_transcript(released);
        let record = activity
            .latency
            .audio(released + Duration::from_millis(500))
            .unwrap();
        assert_eq!(record["origin"], "transcript");
        assert_eq!(record["elapsed_ms"], 500);
    }
}

/// A generation that only called tools leaves the candidate who finished
/// speaking unanswered; one that spoke, or that nobody was waiting on, does
/// not.
#[test]
fn only_a_tool_only_answer_to_a_waiting_candidate_is_unvoiced() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_output(start);
    assert!(!activity.answered_without_speaking(), "nobody was waiting");

    activity.note_turn_boundary(start);
    activity.note_candidate_finished(start, false);
    activity.note_output(start + Duration::from_millis(500));
    assert!(activity.answered_without_speaking());

    activity.note_reply_audible();
    assert!(!activity.answered_without_speaking(), "the reply was heard");

    let mut cut = RuntimeActivity::new(start);
    cut.note_candidate_finished(start, false);
    cut.note_output(start);
    cut.socket.discarding_output = true;
    assert!(
        !cut.answered_without_speaking(),
        "a cut generation is not an answer"
    );
}

#[test]
fn recovery_debt_preserves_the_yield_for_current_and_pending_candidate_input() {
    for pending in [false, true] {
        let start = Instant::now();
        let mut activity = RuntimeActivity::new(start);
        activity.latency.voice(start);
        if pending {
            activity.latency.audio(start + Duration::from_millis(100));
            activity.latency.voice_start();
            activity.latency.voice(start + Duration::from_millis(200));
        }
        activity.latency.yielded(start + Duration::from_millis(300));
        activity.mark_recovery_prompted(start + Duration::from_millis(400), None);
        if pending {
            activity.latency.boundary(
                super::super::latency::Boundary::TurnComplete,
                start + Duration::from_millis(500),
            );
        }
        let record = activity
            .latency
            .audio(start + Duration::from_millis(800))
            .unwrap();
        assert_eq!(record["origin"], "last_voiced_pcm");
        assert_eq!(
            record["candidate_yield_ms"],
            if pending { 100 } else { 300 }
        );
    }
}

/// A reply somebody is waiting on and nothing has started toward is given up
/// on at `FIRST_OUTPUT_DEADLINE`, not the far longer `reply_timeout`; one that
/// has spoken, or a prompt silence answers, is not.
#[test]
fn an_unstarted_reply_is_recovered_at_the_first_output_deadline() {
    let start = Instant::now();
    let mut waiting = RuntimeActivity::new(start);
    assert!(waiting.reply_timeout > FIRST_OUTPUT_DEADLINE);
    waiting.note_candidate_finished(start, false);
    assert!(!waiting.reply_timed_out(
        start + FIRST_OUTPUT_DEADLINE - Duration::from_millis(1),
        false
    ));
    assert!(waiting.reply_timed_out(start + FIRST_OUTPUT_DEADLINE, false));
    assert!(
        !waiting.reply_timed_out(start + FIRST_OUTPUT_DEADLINE, true),
        "audio still playing"
    );

    // Tool calls are output, but the continuation they wait on is not.
    waiting.note_output(start + Duration::from_secs(1));
    assert!(!waiting.reply_timed_out(start + FIRST_OUTPUT_DEADLINE, false));
    let responded = start + Duration::from_secs(2);
    waiting.note_tool_response(responded);
    assert!(!waiting.reply_timed_out(
        responded + FIRST_OUTPUT_DEADLINE - Duration::from_millis(1),
        false
    ));
    assert!(waiting.reply_timed_out(responded + FIRST_OUTPUT_DEADLINE, false));

    // A continuation that began is under the progress rule.
    waiting.note_output(responded + Duration::from_secs(1));
    assert!(!waiting.reply_timed_out(responded + FIRST_OUTPUT_DEADLINE * 2, false));

    let mut prompted = RuntimeActivity::new(start);
    prompted.mark_prompted(start, Some("owed"), false);
    assert!(prompted.reply_timed_out(start + FIRST_OUTPUT_DEADLINE, false));

    let mut optional = RuntimeActivity::new(start);
    optional.mark_prompted(start, Some("review"), true);
    assert!(!optional.reply_timed_out(start + FIRST_OUTPUT_DEADLINE, false));
}

/// A prompt sent after a stalled continuation restarts the deadline, so its
/// own answer is waited on before the socket is replaced.
#[test]
fn a_reply_check_gets_its_own_first_output_deadline() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    activity.note_output(start);
    activity.note_tool_response(start);
    let asked = start + Duration::from_secs(8);
    activity.mark_recovery_prompted(asked, Some("reply due?"));
    assert!(!activity.reply_timed_out(start + FIRST_OUTPUT_DEADLINE, false));
    assert!(activity.reply_timed_out(asked + FIRST_OUTPUT_DEADLINE, false));
}

/// A cold socket's context starts over; a resumed one keeps its size.
#[test]
fn a_cold_socket_forgets_the_last_context_size() {
    for resumed in [false, true] {
        let start = Instant::now();
        let mut activity = RuntimeActivity::new(start);
        activity.latency.observe_usage(9000, 0);
        activity.reset_context_observations(resumed);
        activity.latency.voice(start);
        let record = activity.latency.audio(start).unwrap();
        assert_eq!(
            record["context_tokens"],
            if resumed {
                serde_json::json!(9000)
            } else {
                serde_json::Value::Null
            }
        );
    }
}

/// While Gemini reports the candidate mid-turn, a pause in their speech is
/// not a reply that never came; only the far longer `reply_timeout` applies.
#[test]
fn the_first_output_deadline_waits_out_a_candidate_mid_turn() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("nudge"), false);
    activity.note_candidate_voice_activity(true, start + Duration::from_secs(2));
    assert!(!activity.reply_timed_out(start + FIRST_OUTPUT_DEADLINE * 2, false));
    assert!(activity.reply_timed_out(start + activity.reply_timeout, false));
}

#[test]
fn a_required_prompt_tool_continuation_uses_the_first_output_deadline() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("greeting"), false);
    activity.note_output(start + Duration::from_secs(1));
    let responded = start + Duration::from_secs(2);
    activity.note_tool_response(responded);
    assert!(!activity.reply_in_flight());
    assert!(!activity.owes_prompt());
    assert!(!activity.reply_timed_out(
        responded + FIRST_OUTPUT_DEADLINE - Duration::from_millis(1),
        false
    ));
    assert!(activity.reply_timed_out(responded + FIRST_OUTPUT_DEADLINE, false));
    activity.note_output(responded + Duration::from_secs(1));
    assert!(!activity.reply_timed_out(responded + FIRST_OUTPUT_DEADLINE * 2, false));
    assert!(activity.reply_timed_out(
        responded + Duration::from_secs(1) + activity.reply_timeout,
        false
    ));
}

#[test]
fn interrupted_text_continuation_preserves_an_unheard_required_prompt() {
    let start = Instant::now();
    for audible in [false, true] {
        let mut activity = RuntimeActivity::new(start);
        activity.mark_prompted(start, Some("greeting"), false);
        activity.note_output(start + Duration::from_secs(1));
        activity.note_tool_response(start + Duration::from_secs(2));
        activity.note_output(start + Duration::from_secs(3));
        if audible {
            activity.note_reply_audible();
        }
        let interrupted = start + Duration::from_secs(4);
        let debt = activity.debt_past_interruption(interrupted);
        activity.note_turn_boundary(interrupted);
        activity.settle_completed_turn(false);
        activity.keep_reply_debt(debt);
        assert_eq!(activity.owes_reply(), !audible);
        assert_eq!(
            activity.reply_timed_out(interrupted + FIRST_OUTPUT_DEADLINE, false),
            !audible
        );
    }
}

#[test]
fn interrupted_tools_do_not_require_optional_or_already_audible_replies() {
    let start = Instant::now();
    for (optional, stalled) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut activity = RuntimeActivity::new(start);
        if optional {
            activity.mark_prompted(start, Some("optional review"), true);
        } else {
            activity.note_candidate_finished(start, false);
        }
        activity.note_output(start + Duration::from_secs(1));
        if !optional {
            activity.note_reply_audible();
        }
        activity.note_tool_response(start + Duration::from_secs(2));
        let interrupted = if stalled {
            let at = start + Duration::from_secs(2) + PROMPT_STALL;
            activity.settle_stalls(at, false);
            at
        } else {
            start + Duration::from_secs(3)
        };
        let debt = activity.debt_past_interruption(interrupted);
        activity.note_turn_boundary(interrupted);
        activity.settle_completed_turn(false);
        activity.keep_reply_debt(debt);
        assert!(!activity.owes_reply());
        assert!(!activity.reply_timed_out(interrupted + activity.reply_timeout, false));
    }
}

#[test]
fn interruptions_preserve_rebased_required_prompt_deadlines() {
    let start = Instant::now();
    for queued in [false, true] {
        let mut activity = RuntimeActivity::new(start);
        if queued {
            activity.note_output(start);
        }
        activity.mark_prompted(start, Some("required reaction"), false);
        let released = start + Duration::from_secs(30);
        if queued {
            activity.note_turn_boundary(released);
            activity.settle_completed_turn(false);
        } else {
            activity.resume_reply_wait(released);
        }
        let interrupted = released + Duration::from_secs(1);
        let debt = activity.debt_past_interruption(interrupted);
        activity.note_turn_boundary(interrupted);
        activity.settle_completed_turn(false);
        activity.keep_reply_debt(debt);
        assert!(!activity.reply_timed_out(interrupted, false));
        assert!(!activity.reply_timed_out(
            released + FIRST_OUTPUT_DEADLINE - Duration::from_millis(1),
            false
        ));
        assert!(activity.reply_timed_out(released + FIRST_OUTPUT_DEADLINE, false));
    }
}

#[test]
fn required_prompt_clean_completion_can_intentionally_be_silent() {
    let start = Instant::now();
    for tools in [false, true] {
        let mut activity = RuntimeActivity::new(start);
        activity.mark_prompted(start, Some("required reaction"), false);
        if tools {
            activity.note_output(start);
            activity.note_tool_response(start);
        }
        activity.note_turn_boundary(start + Duration::from_secs(1));
        activity.settle_completed_turn(false);
        assert!(!activity.owes_reply());
        assert!(!activity.reply_timed_out(start + activity.reply_timeout, false));
    }
}

#[test]
fn a_queued_required_prompt_supersedes_the_tool_only_followup() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_finished(start, false);
    activity.note_output(start);
    activity.note_tool_response(start);
    activity.mark_prompted(start + Duration::from_secs(1), Some("test result"), false);
    assert!(!activity.answered_without_speaking());
    activity.note_turn_boundary(start + Duration::from_secs(2));
    activity.settle_completed_turn(false);
    assert!(!activity.reply_in_flight());
    assert_eq!(activity.prompt_text.as_deref(), Some("test result"));
    activity.note_output(start + Duration::from_secs(3));
    assert!(!activity.answered_without_speaking());
}

/// One acknowledgement per candidate wait, however many tool rounds hold the
/// reply back, and none without a wait or during a discard.
#[test]
fn a_tool_acknowledgement_plays_once_per_wait() {
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let mut activity = RuntimeActivity::new(start);
    assert!(!activity.take_tool_ack(at(0)), "nobody is waiting");
    activity.note_candidate_finished(at(0), false);
    assert!(activity.take_tool_ack(at(10)));
    assert!(!activity.take_tool_ack(at(20)), "the same wait");

    // A wait rebased across an interruption is still the same request.
    activity.awaiting_reply_since = Some(at(3_000));
    assert!(!activity.take_tool_ack(at(3_010)), "a rebased wait");

    activity.clear_candidate_reply_wait();
    activity.note_candidate_finished(at(5_000), false);
    activity.socket.discarding_output = true;
    assert!(!activity.take_tool_ack(at(5_010)), "a discarded generation");
    activity.socket.discarding_output = false;

    // Never over the candidate speaking again.
    activity.note_candidate_voice_activity(true, at(5_020));
    activity.note_candidate_voice(at(5_030));
    assert!(
        !activity.take_tool_ack(at(5_040)),
        "the candidate is speaking"
    );
    activity.note_candidate_voice_activity(false, at(5_050));
    assert!(activity.take_tool_ack(at(9_000)), "a new wait, once quiet");

    activity.note_candidate_finished(at(9_500), false);
    activity.clear_candidate_reply_wait();
    activity.awaiting_reply_since = Some(at(9_600));
    assert!(!activity.take_tool_ack(at(9_700)), "an answered request");

    // A cut that keeps the wait keeps the acknowledgement owed with it.
    activity.clear_candidate_reply_wait();
    activity.note_candidate_finished(at(12_000), false);
    let debt = activity.debt_past_interruption(at(12_100));
    activity.clear_candidate_reply_wait();
    activity.keep_reply_debt(debt);
    assert!(
        activity.take_tool_ack(at(12_200)),
        "a wait kept across a cut"
    );
}

/// Microphone frames from before the provider's end of speech belong to the
/// utterance that ended, so they are not the candidate cutting in; only a
/// voiced frame after that end is.
#[test]
fn voiced_frames_before_the_end_of_speech_are_not_an_interruption() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_voice_activity(true, start);
    activity.note_candidate_voice(start + Duration::from_millis(100));
    activity.note_candidate_voice_activity(false, start + Duration::from_millis(200));
    assert!(!activity.candidate_interrupting(start + Duration::from_millis(300)));
    activity.note_candidate_voice(start + Duration::from_millis(400));
    assert!(activity.candidate_interrupting(start + Duration::from_millis(500)));
}

/// An interruption owes the candidate's incoming speech only when the
/// candidate is the one cutting in; an unfinished reply interrupted while
/// nobody speaks owes no new speech.
#[test]
fn an_interruption_with_nobody_speaking_owes_no_incoming_speech() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.note_output(start);
    assert!(activity.reply_unfinished());
    let debt = activity.debt_past_interruption(start + Duration::from_secs(1));
    assert!(!debt.incoming_speech_owed);
    activity.note_candidate_voice_activity(true, start + Duration::from_secs(1));
    let debt = activity.debt_past_interruption(start + Duration::from_secs(2));
    assert!(debt.incoming_speech_owed);
}

/// A voiced frame at exactly the provider's end of speech belongs to the
/// utterance that ended, as the latency trace's end keeps frames at or before
/// it, so it is not the candidate cutting in.
#[test]
fn a_voiced_frame_at_the_end_of_speech_is_not_an_interruption() {
    let start = Instant::now();
    let ended = start + Duration::from_millis(200);
    let mut activity = RuntimeActivity::new(start);
    activity.note_candidate_voice_activity(true, start);
    activity.note_candidate_voice(ended);
    activity.note_candidate_voice_activity(false, ended);
    assert!(!activity.candidate_interrupting(ended + Duration::from_millis(100)));
}

/// A short first-output setting does not cut a replacement's briefing at the
/// short deadline: the recovery gets the default one, timed from its own
/// request, and is replaced only when that runs out.
#[test]
fn a_five_second_limit_does_not_cut_the_replacement_briefing() {
    let start = Instant::now();
    let short = Duration::from_secs(5);
    let mut activity = RuntimeActivity::new(start).with_first_output_deadline(short);
    activity.note_candidate_finished(start, false);
    assert_eq!(
        activity.expired_reply_wait(start + short, false),
        Some(short)
    );
    let briefed = start + Duration::from_secs(6);
    activity.mark_recovery_prompted(briefed, None);
    assert_eq!(
        activity.expired_reply_wait(briefed + Duration::from_millis(5_300), false),
        None
    );
    assert_eq!(
        activity.expired_reply_wait(
            briefed + FIRST_OUTPUT_DEADLINE - Duration::from_millis(1),
            false
        ),
        None
    );
    assert_eq!(
        activity.expired_reply_wait(briefed + FIRST_OUTPUT_DEADLINE, false),
        Some(FIRST_OUTPUT_DEADLINE)
    );
    assert_eq!(
        activity.expired_reply_wait(briefed + FIRST_OUTPUT_DEADLINE, true),
        None
    );

    // A longer setting is kept for a recovery too.
    let long = Duration::from_secs(30);
    let mut activity = RuntimeActivity::new(start).with_first_output_deadline(long);
    activity.mark_recovery_prompted(start, None);
    assert_eq!(
        activity.expired_reply_wait(start + FIRST_OUTPUT_DEADLINE, false),
        None
    );
    assert_eq!(activity.expired_reply_wait(start + long, false), Some(long));
}

/// Once ordinary work supersedes the recovery, the configured deadline is
/// back: a new candidate turn, an ordinary prompt, or a continuation.
#[test]
fn recovery_timeout_does_not_leak_into_superseding_requests() {
    let start = Instant::now();
    let short = Duration::from_secs(5);
    for candidate in [false, true] {
        let mut activity = RuntimeActivity::new(start).with_first_output_deadline(short);
        activity.mark_recovery_prompted(start, None);
        let next = start + Duration::from_secs(1);
        if candidate {
            activity.note_candidate_finished(next, false);
        } else {
            activity.mark_prompted(next, Some("ordinary prompt"), false);
        }
        assert_eq!(
            activity.expired_reply_wait(next + short, false),
            Some(short)
        );
    }
    let mut activity = RuntimeActivity::new(start).with_first_output_deadline(short);
    activity.mark_recovery_prompted(start, None);
    activity.note_output(start + Duration::from_secs(1));
    let responded = start + Duration::from_secs(2);
    activity.note_tool_response(responded);
    assert_eq!(
        activity.expired_reply_wait(responded + short, false),
        Some(short)
    );
}

/// A recovery cut by an interruption keeps its own deadline across the cut.
#[test]
fn an_interruption_keeps_the_recovery_prompts_timeout() {
    let start = Instant::now();
    let mut activity =
        RuntimeActivity::new(start).with_first_output_deadline(Duration::from_secs(5));
    activity.mark_recovery_prompted(start, Some("owed"));
    let interrupted = start + Duration::from_secs(1);
    let debt = activity.debt_past_interruption(interrupted);
    activity.note_turn_boundary(interrupted);
    activity.keep_reply_debt(debt);
    assert_eq!(
        activity.expired_reply_wait(start + Duration::from_secs(6), false),
        None
    );
    assert_eq!(
        activity.expired_reply_wait(start + FIRST_OUTPUT_DEADLINE, false),
        Some(FIRST_OUTPUT_DEADLINE)
    );
}

/// A recovery sent behind a generation still running keeps its request past
/// that generation's output; a tool continuation after it is newer work and is
/// held to the configured deadline, not the recovery's longer one.
#[test]
fn a_continuation_after_a_recovery_behind_a_turn_keeps_the_configured_deadline() {
    let start = Instant::now();
    let short = Duration::from_secs(5);
    let mut activity = RuntimeActivity::new(start).with_first_output_deadline(short);
    activity.note_output(start);
    activity.mark_recovery_prompted(start + Duration::from_secs(1), None);
    activity.note_output(start + Duration::from_secs(2));
    assert!(activity.prompted_at.is_some(), "the recovery is still owed");
    let responded = start + Duration::from_secs(3);
    activity.note_tool_response(responded);
    assert_eq!(
        activity.expired_reply_wait(responded + short, false),
        Some(short)
    );
}

/// The watch is handed back once, with both anchors a first-audio line is
/// timed against. A second reply on the same socket is not a rotation.
#[test]
fn a_replacement_watch_is_taken_once() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    assert!(
        activity.take_rotation_watch().is_none(),
        "a socket that was never replaced has nothing to report"
    );

    let connected = start + Duration::from_millis(1_800);
    activity.begin_rotation_watch(
        true,
        RotationWatch {
            started: start,
            connected,
            resumed: true,
            candidate_spoke_at: None,
        },
    );
    let watch = activity.take_rotation_watch().expect("the watch is taken");
    assert!(watch.resumed);
    assert_eq!(watch.started, start);
    assert_eq!(watch.connected, connected);
    assert!(
        activity.take_rotation_watch().is_none(),
        "only the first audio counts"
    );
}

/// A replacement that itself gets replaced before it was heard is timed from
/// the later one, so the reported gap is never the sum of two attempts.
#[test]
fn a_second_replacement_before_any_audio_restarts_the_timing() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.begin_rotation_watch(
        true,
        RotationWatch {
            started: start,
            connected: start + Duration::from_millis(500),
            resumed: true,
            candidate_spoke_at: None,
        },
    );
    let second = start + Duration::from_secs(3);
    activity.begin_rotation_watch(
        true,
        RotationWatch {
            started: second,
            connected: second + Duration::from_millis(900),
            resumed: false,
            candidate_spoke_at: None,
        },
    );
    let watch = activity.take_rotation_watch().expect("reported");
    assert!(!watch.resumed);
    assert_eq!(watch.started, second);
}

/// A replacement whose briefing was answered with silence is not waited on any
/// longer: the audio that arrives minutes later is an ordinary reply.
#[test]
fn a_turn_that_ends_without_audio_disarms_the_replacement_watch() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.begin_rotation_watch(
        true,
        RotationWatch {
            started: start,
            connected: start + Duration::from_millis(600),
            resumed: false,
            candidate_spoke_at: None,
        },
    );
    activity.settle_completed_turn(false);
    assert!(activity.take_rotation_watch().is_none());
}

/// Candidate speech after a replacement is noted on the watch and does not
/// disarm it: the slow replacements are the ones spoken over, and they have to
/// stay in the log, marked.
#[test]
fn candidate_speech_marks_the_replacement_watch_without_disarming_it() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.begin_rotation_watch(
        true,
        RotationWatch {
            started: start,
            connected: start + Duration::from_millis(600),
            resumed: true,
            candidate_spoke_at: None,
        },
    );
    activity.note_candidate_voice_activity(false, start + Duration::from_secs(1));
    let quiet = activity
        .take_rotation_watch()
        .expect("silence is not speech");
    assert!(quiet.candidate_spoke_at.is_none());

    activity.begin_rotation_watch(
        true,
        RotationWatch {
            started: start,
            connected: start + Duration::from_millis(600),
            resumed: true,
            candidate_spoke_at: None,
        },
    );
    let first = start + Duration::from_secs(2);
    activity.note_candidate_voice_activity(true, first);
    activity.note_candidate_voice_activity(true, start + Duration::from_secs(4));
    let spoken_over = activity
        .take_rotation_watch()
        .expect("speech keeps the watch armed");
    assert_eq!(spoken_over.candidate_spoke_at, Some(first));
}

/// A watch armed by one replacement is gone when the next socket takes over,
/// whether or not that socket asks for a reply of its own.
#[test]
fn a_new_socket_drops_the_previous_replacement_watch() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.begin_rotation_watch(
        true,
        RotationWatch {
            started: start,
            connected: start + Duration::from_millis(600),
            resumed: true,
            candidate_spoke_at: None,
        },
    );
    activity.reset_context_observations(true);
    assert!(activity.take_rotation_watch().is_none());
}

/// A watch is armed only when the caller says the briefing asked for a reply,
/// and a call that says it did not leaves nothing armed, not even a watch an
/// earlier replacement left. Which replacements ask for one is decided by the
/// caller, which no unit test reaches.
#[test]
fn a_replacement_that_asked_for_no_reply_arms_no_watch() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let watch = || RotationWatch {
        started: start,
        connected: start + Duration::from_millis(600),
        resumed: true,
        candidate_spoke_at: None,
    };

    // A watch left armed by an earlier replacement is replaced by nothing too,
    // not carried over.
    activity.begin_rotation_watch(true, watch());
    activity.begin_rotation_watch(false, watch());
    assert!(activity.take_rotation_watch().is_none());
}

/// The provider may send no speech start, so voiced microphone frames mark the
/// watch as well, and the first mark is the one kept.
#[test]
fn voiced_microphone_frames_mark_the_replacement_watch() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.begin_rotation_watch(
        true,
        RotationWatch {
            started: start,
            connected: start + Duration::from_millis(600),
            resumed: true,
            candidate_spoke_at: None,
        },
    );
    let first = start + Duration::from_secs(2);
    activity.note_candidate_voice(first);
    activity.note_candidate_voice(start + Duration::from_secs(3));
    let watch = activity.take_rotation_watch().expect("still armed");
    assert_eq!(watch.candidate_spoke_at, Some(first));
}

/// A prompt sent after the briefing, a nudge or the farewell, is what the next
/// audio answers, so it disarms the watch. The briefing's own prompt does not.
#[test]
fn a_later_prompt_disarms_the_replacement_watch() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let watch = || RotationWatch {
        started: start,
        connected: start + Duration::from_millis(600),
        resumed: true,
        candidate_spoke_at: None,
    };
    activity.mark_recovery_prompted(start, None);
    activity.begin_rotation_watch(true, watch());
    assert!(
        activity.take_rotation_watch().is_some(),
        "the briefing arms its own watch"
    );

    activity.begin_rotation_watch(true, watch());
    activity.mark_prompted(start + Duration::from_secs(5), Some("farewell"), false);
    assert!(activity.take_rotation_watch().is_none());
}

/// Operators read this line, so its fields and their order are pinned: the
/// time since the old socket was given up, since the new one was ready, and
/// since the candidate first spoke, `none` when they did not.
#[test]
fn the_first_audio_line_names_each_wait_and_whether_the_candidate_spoke() {
    let start = Instant::now();
    let mut watch = RotationWatch {
        started: start,
        connected: start + Duration::from_millis(600),
        resumed: true,
        candidate_spoke_at: None,
    };
    let heard = start + Duration::from_millis(2_500);
    assert_eq!(
        watch
            .first_audio_line("9:13.761", heard, 3, "room-a")
            .as_deref(),
        Some(
            "replacement first audio: at=9:13.761 resumed=true since_start_ms=2500 \
         since_connected_ms=1900 candidate_spoke_ms=none socket=3 room=room-a"
        )
    );

    watch.candidate_spoke_at = Some(start + Duration::from_millis(1_200));
    watch.resumed = false;
    assert_eq!(
        watch
            .first_audio_line("0:25.625", heard, 2, "room-b")
            .as_deref(),
        Some(
            "replacement first audio: at=0:25.625 resumed=false since_start_ms=2500 \
         since_connected_ms=1900 candidate_spoke_ms=1200 socket=2 room=room-b"
        )
    );
}

/// Audio that arrives after the longest wait the reply watchdog allows was
/// drawn by something other than the briefing, so no line is written for it.
#[test]
fn audio_after_the_longest_honest_wait_is_not_timed_as_the_replacement() {
    let start = Instant::now();
    let watch = RotationWatch {
        started: start,
        connected: start + Duration::from_millis(600),
        resumed: true,
        candidate_spoke_at: None,
    };
    let at_limit =
        watch.connected + Duration::from_secs(u64::from(crate::config::MAX_GEMINI_REPLY_TIMEOUT_S));
    assert!(watch.first_audio_line("c", at_limit, 2, "r").is_some());
    let past = at_limit + Duration::from_millis(1);
    assert!(watch.first_audio_line("c", past, 2, "r").is_none());
}

const SILENCE: Duration = Duration::from_millis(1_500);

#[test]
fn output_settles_both_startup_progress_and_the_handoff_reply_wait() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.buffered_startup_output_at = Some(start);
    activity.arm_handoff_reply(start);
    let due = start + SILENCE + HANDOFF_REPLY_MARGIN;
    assert!(activity.handoff_reply_due(due, SILENCE, false));

    activity.note_output(start + Duration::from_millis(1));
    assert!(activity.buffered_startup_output_at.is_none());
    assert!(!activity.handoff_reply_due(due, SILENCE, false));
}

/// The listening note requests no reply, so a handoff socket whose candidate
/// has already finished has nothing to answer. It is asked once the silence
/// window and the margin have both passed, and not before.
#[test]
fn a_handoff_socket_is_asked_for_its_reply_after_the_window_and_margin() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    assert!(!activity.handoff_reply_due(start + Duration::from_secs(60), SILENCE, false));

    activity.arm_handoff_reply(start);

    // The margin is a real allowance for the model to start a turn on its own
    // after the window closes, not just the window.
    assert!(!activity.handoff_reply_due(start + SILENCE + Duration::from_secs(2), SILENCE, false));
    let due = SILENCE + HANDOFF_REPLY_MARGIN;
    assert!(!activity.handoff_reply_due(start + due - Duration::from_millis(1), SILENCE, false));
    assert!(activity.handoff_reply_due(start + due, SILENCE, false));
}

/// Counted from the later of the handoff and the candidate stopping, so a
/// candidate who goes on speaking after it is not answered over.
#[test]
fn the_handoff_reply_wait_restarts_when_the_candidate_stops_speaking() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.arm_handoff_reply(start);
    activity.note_candidate_voice_activity(true, start + Duration::from_secs(1));
    let stopped = start + Duration::from_secs(6);
    activity.note_candidate_voice_activity(false, stopped);

    let due = SILENCE + HANDOFF_REPLY_MARGIN;
    assert!(!activity.handoff_reply_due(start + due, SILENCE, false));
    assert!(!activity.handoff_reply_due(stopped + due - Duration::from_millis(1), SILENCE, false));
    assert!(activity.handoff_reply_due(stopped + due, SILENCE, false));
}

/// Not while the interviewer's own audio is playing, while the provider says
/// the candidate is speaking, or while the microphone is carrying their voice.
#[test]
fn the_handoff_reply_is_not_requested_over_audio_or_live_speech() {
    let start = Instant::now();
    let late = start + Duration::from_secs(30);

    let mut activity = RuntimeActivity::new(start);
    activity.arm_handoff_reply(start);
    assert!(!activity.handoff_reply_due(late, SILENCE, true));

    // The provider's edge says speaking and its end has not come, however long
    // the microphone has been quiet: the model is still inside its window.
    activity.note_candidate_voice_activity(true, start + Duration::from_secs(1));
    assert!(!activity.handoff_reply_due(late, SILENCE, false));

    // Voiced audio just now, whatever the provider has said.
    let mut activity = RuntimeActivity::new(start);
    activity.arm_handoff_reply(start);
    activity.note_candidate_voice(late);
    assert!(!activity.handoff_reply_due(late, SILENCE, false));
}

/// Speech that only the microphone shows, because the new socket has not yet
/// sent its own speech edge, still counts as the candidate's last word.
#[test]
fn the_handoff_reply_waits_out_the_quiet_after_microphone_only_speech() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.arm_handoff_reply(start);
    let last_voiced = start + Duration::from_secs(10);
    activity.note_candidate_voice(last_voiced);

    let due = SILENCE + HANDOFF_REPLY_MARGIN;
    assert!(!activity.handoff_reply_due(start + due, SILENCE, false));
    assert!(!activity.handoff_reply_due(
        last_voiced + due - Duration::from_millis(1),
        SILENCE,
        false
    ));
    assert!(activity.handoff_reply_due(last_voiced + due, SILENCE, false));
}

/// Asking for the reply keeps the request the socket still owed and holds the
/// new deadline to the recovery minimum, like a replacement's briefing.
#[test]
fn asking_a_handoff_socket_for_its_reply_keeps_what_it_owed() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.owe_prompt(start, Some("[SYSTEM EVENT] Review the editor.".to_string()));
    activity.arm_handoff_reply(start);
    activity.renew_handoff_prompt(start + Duration::from_secs(5));
    assert!(activity.owes_prompt());
    assert!(activity.recovery_prompt);
    assert_eq!(
        activity.prompt_text.as_deref(),
        Some("[SYSTEM EVENT] Review the editor.")
    );
}

/// Anything that answers the listening note, or makes it stale, ends the wait:
/// output, a later prompt, the end of a turn, or a newer socket.
#[test]
fn the_handoff_reply_wait_ends_when_the_socket_speaks_is_prompted_or_replaced() {
    let start = Instant::now();
    let late = start + Duration::from_secs(60);
    for case in ["output", "prompt", "turn", "socket"] {
        let mut activity = RuntimeActivity::new(start);
        activity.arm_handoff_reply(start);
        match case {
            "output" => activity.note_output(start + Duration::from_secs(1)),
            "prompt" => activity.mark_prompted(start + Duration::from_secs(1), None, false),
            "turn" => activity.settle_completed_turn(false),
            _ => activity.reset_context_observations(true),
        }
        assert!(!activity.handoff_reply_due(late, SILENCE, false), "{case}");
    }
}
