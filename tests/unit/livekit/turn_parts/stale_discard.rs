// Included in the turn test module so existing test paths stay unchanged.

/// A hold cut off a generation that had stalled, so no discard was armed; its
/// late output, dropped under the hold, then armed one. Both wait on that one
/// old turn, so its last ending has to end both, or the discard eats the first
/// reply after the release.
#[test]
fn the_last_stale_ending_also_ends_a_discard_armed_for_the_same_turn() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    activity.socket.stale_turn_pending = true;
    activity.socket.discarding_output = true;
    assert!(!activity.accept_event(&crate::gemini::GeminiEvent::Interrupted, true, now));
    assert!(
        activity.socket.discarding_output,
        "its turn is not over yet"
    );
    assert!(!activity.accept_event(&crate::gemini::GeminiEvent::TurnComplete, true, now));
    assert!(!activity.socket.discarding_output);
    assert!(!activity.socket.stale_turn_pending);
}

#[test]
fn a_stale_ending_leaves_a_discard_alone_when_none_was_armed_for_it() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    activity.socket.stale_turn_pending = true;
    assert!(!activity.accept_event(&crate::gemini::GeminiEvent::TurnComplete, false, now));
    assert!(!activity.socket.discarding_output);
}
