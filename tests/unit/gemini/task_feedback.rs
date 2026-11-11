use super::*;

/// A refused review is the learner's to be told is unavailable, and the
/// operator's to be told why: a model that keeps failing validation is not a
/// model that never answered. The reason is bounded and on one line, since it
/// is written into a log.
#[test]
fn a_refused_review_attempt_says_which_attempt_and_why_on_one_line() {
    let note = refusal_note("room-a", 1, "the answer was refused: gaps\nwere not closed");
    assert_eq!(
        note,
        "task review room=room-a attempt 1 refused: the answer was refused: gaps\\nwere not closed"
    );
    let long = refusal_note("room-a", 2, &"x".repeat(1000));
    assert!(long.ends_with("..."), "{long}");
    assert!(long.chars().count() < 260, "{}", long.chars().count());
}

/// The answer's size is checked at the limit itself: one at it is read, one
/// past it is refused. A comparison that was off by one in either direction
/// would refuse a review that fits or accept one that does not.
#[test]
fn an_answer_is_read_up_to_the_limit_and_refused_one_past_it() {
    assert!(fits_the_limit(&"x".repeat(MAX_REPORT_RESPONSE_BYTES)));
    assert!(!fits_the_limit(&"x".repeat(MAX_REPORT_RESPONSE_BYTES + 1)));
    assert!(fits_the_limit(""));
}
