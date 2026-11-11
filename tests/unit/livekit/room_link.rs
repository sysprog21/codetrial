use super::*;

/// Dropping a room does not disconnect it, so an exit that is an error has to
/// leave it, or the candidate stays in a room with a silent agent.
#[tokio::test]
async fn a_room_is_left_when_the_result_is_an_error_and_not_otherwise() {
    let (room, sent) = RoomLink::recording("interview-abc");
    let kept: Result<u8, &str> = room.close_on_error(Ok(7)).await;
    assert_eq!(kept, Ok(7), "the result is handed back as it came");
    assert_eq!(sent.take(), [], "a success leaves the room to its own path");

    let failed: Result<u8, &str> = room.close_on_error(Err("gemini would not open")).await;
    assert_eq!(failed, Err("gemini would not open"));
    assert_eq!(sent.take(), [Sent::Left]);
}

/// A notice the interview does not depend on is sent best effort, and the room
/// loop awaits it, so a connection that is reconnecting must not park the loop
/// for the length of the reconnect: audio would not drain and the hard deadline
/// could not fire.
#[tokio::test(start_paused = true)]
async fn a_best_effort_publish_gives_up_on_a_stalled_connection() {
    let (room, sent) = RoomLink::recording("interview-abc");
    let (_entered, _release) = sent.hold_data("thinking_state", None);
    let started = tokio::time::Instant::now();
    crate::livekit::session::publish_control(
        &room,
        &serde_json::json!({"type": "thinking_state", "thinking": true}),
        "the thinking state",
    )
    .await;
    assert_eq!(started.elapsed(), crate::livekit::RECONNECT_NOTICE_LIMIT);
}

/// The exits that fail once the agent has joined the room must go through the
/// helper. A live room cannot be built here, so the wiring is pinned in the
/// source, as the empty-room exit's is: a later edit that drops a call would
/// otherwise leave the candidate with a silent agent and break no test.
#[test]
fn every_error_exit_after_the_join_leaves_the_room() {
    let open = include_str!("../../../src/livekit/open.rs");
    let hub = include_str!("../../../src/livekit.rs");
    for (name, source, call) in [
        (
            "the room and Gemini setup",
            open,
            "room.close_on_error(joined)",
        ),
        (
            "the opening attributes",
            open,
            "room.close_on_error(opened_attributes)",
        ),
        (
            "the wait for the candidate",
            open,
            "room.close_on_error(arrival)",
        ),
        ("the greeting", open, "room.close_on_error(greeted)"),
        ("the room loop", hub, "room.close_on_error(result)"),
    ] {
        assert_eq!(source.matches(call).count(), 1, "{name} leaves on error");
    }
}
