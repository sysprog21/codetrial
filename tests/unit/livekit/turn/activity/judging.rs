use super::*;
use serde_json::json;

#[test]
fn a_held_transition_waits_for_the_judge_until_its_deadline() {
    let now = Instant::now();
    let settle = Duration::from_secs(5);
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(now);
    assert!(activity.take_ready_parked_transition(&state, now).is_none());

    activity.hold_round_transition(&json!({"round": 1}), true, now, settle);
    activity.hold_round_transition(&json!({"round": 2}), false, now + settle, settle);
    let parked = activity
        .take_ready_parked_transition(&state, now)
        .expect("nothing is left to judge, so the hold is ready");
    assert_eq!(parked.payload, json!({"round": 1}));
    assert!(parked.received);
    assert_eq!(parked.deadline, now + settle);
    assert!(activity.take_ready_parked_transition(&state, now).is_none());
}

#[test]
fn a_held_transition_stays_held_while_a_judgment_is_unsettled_and_in_time() {
    let now = Instant::now();
    let settle = Duration::from_secs(5);
    let state = RuntimeState {
        code: "fn main() {}".into(),
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(now);
    activity.hold_round_transition(&json!({}), true, now, settle);
    assert!(crate::agent::phase_judge_due(&state));
    assert!(activity.take_ready_parked_transition(&state, now).is_none());
    assert!(
        activity
            .take_ready_parked_transition(&state, now + settle)
            .is_some()
    );
}
