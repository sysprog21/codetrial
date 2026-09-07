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
}
