use super::*;
use std::time::Duration;

#[test]
fn a_handoff_that_owed_a_reply_owes_it_again_and_waits_on_audio() {
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    assert!(!activity.owes_reply());
    assert!(!activity.handoff_reply_due(now + Duration::from_secs(600), Duration::ZERO, false));

    activity.owe_handoff_reply(now, Some("answer the question".into()));
    assert!(activity.owes_reply());
    assert!(activity.handoff_reply_due(now + Duration::from_secs(600), Duration::ZERO, false));
}
