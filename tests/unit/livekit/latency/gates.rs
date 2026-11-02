use super::*;

#[test]
fn gate_invalidation_exists_without_a_trace_for_every_boundary() {
    let (start, mut trace) = traced();
    trace.anchor(start);
    for (boundary, stage) in [
        (Boundary::TurnComplete, "turn_complete"),
        (Boundary::Interrupted, "interrupted"),
        (Boundary::Held, "held"),
        (Boundary::SocketReplaced, "socket_replaced"),
    ] {
        let mut output = Vec::new();
        assert!(
            trace
                .boundary_to(boundary, ms(start, 50), &mut output)
                .is_none()
        );
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.lines().count(), 1);
        let event: Value = serde_json::from_str(
            text.trim_end()
                .strip_prefix("codetrial replay_gate ")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(event["event"], "gate_boundary");
        assert_eq!(event["version"], 1);
        assert_eq!(event["stage"], stage);
        assert_eq!(event["room"], "room");
        assert_eq!(event["session"], 4);
        assert_eq!(event["socket"], 1);
        assert!(event["turn"].is_null());
        assert_eq!(event["interview_ms"], 50);
        assert!(event["clock"]["emitted_unix_ms"].as_u64().is_some());
    }
}
