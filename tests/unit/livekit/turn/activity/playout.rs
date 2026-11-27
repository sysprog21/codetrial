use super::*;
use crate::livekit::turn::LiveOutcome;

#[test]
fn the_usage_summary_names_the_room_session_and_socket_of_the_activity() {
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.live_session_id = 7;
    activity.live_socket = 3;
    let line = activity.live_usage_line("room-a", "live-model", 60, LiveOutcome::Ok);
    assert!(
        line.starts_with(
            "codetrial live_usage room=room-a session=7 model=live-model elapsed_s=60 outcome=ok sockets=3"
        ),
        "{line}"
    );
}
