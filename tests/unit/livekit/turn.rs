//! The `tests` module of `src/livekit/turn.rs`, which declares this file by
//! path.
//! Everything here reaches into `src/livekit/turn.rs` through `super`, so it is
//! a unit
//! test and not an integration test: private items are in scope.

use super::*;

/// A pause arms the discard so a turn already in flight cannot speak over
/// the resume. Arming it on a turn that has finished is the failure: its
/// completion event has already passed, so nothing disarms it and the first
/// reply after the resume is swallowed instead, which is silence exactly
/// where the candidate is waiting to be answered.
#[test]
fn only_a_turn_still_being_produced_leaves_output_to_discard() {
    assert!(pause_leaves_output_in_flight(Floor::Speaking));
    assert!(
        !pause_leaves_output_in_flight(Floor::AwaitingPlayout),
        "the turn is complete and draining, so no event is coming to disarm this"
    );
    assert!(!pause_leaves_output_in_flight(Floor::Listening));
}

#[test]
fn candidate_exit_skips_wrap_up_before_report() {
    assert!(!should_send_wrap_up("candidate_ended"));
    assert!(should_send_wrap_up("time_up"));

    // Jim is told not to say goodbye before calling the tool, so the wrap-up is
    // the only thing that speaks the closing on its route out.
    assert!(should_send_wrap_up("interview_complete"));
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
    let busy: [Busy; 6] = [
        (
            "Jim is mid-sentence, so the stretch is not finished",
            |activity, _| {
                activity.mark_speaking();
            },
        ),
        (
            "a generation is already owed on this socket",
            |activity, _| {
                activity.tool_response_outstanding = true;
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
        (
            "a paused interview is not idle, it is stopped",
            |_, state| {
                state.paused = true;
            },
        ),
        ("a stretch already read is not read again", |_, state| {
            state.interim_transcript_lines = state.transcript.len();
        }),
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
    use crate::gemini::INTERIM_ATTEMPT_TIMEOUT;

    // A call cannot outlive the wait for the next chance to start one. This is
    // what makes the slot in `InterimReview` unable to be occupied when a pause
    // comes due, so the two mechanisms cannot disagree about whether a review
    // is running.
    const { assert!(INTERIM_ATTEMPT_TIMEOUT.as_secs() < INTERIM_COOLDOWN.as_secs()) };

    // What a later review is shown has to leave room for what it may add, or
    // every review is handed a context it cannot help repeating.
    const { assert!(INTERIM_CONTEXT_NOTES + MAX_INTERIM_LINES_PER_REVIEW < MAX_INTERIM_NOTES) };

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
