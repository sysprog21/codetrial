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

#[test]
fn closing_observation_keeps_the_current_reply_trace_intact() {
    let (start, mut trace) = traced();
    trace.anchor(start);
    trace.tool_response(start, 1, true);
    let before = trace.record("tool_response", start, Outcome::Open);
    let mut output = Vec::new();
    trace.gate_observation_to("closing", ms(start, 40), ms(start, 50), &mut output);
    let text = String::from_utf8(output).unwrap();
    let event: Value = serde_json::from_str(
        text.trim_end()
            .strip_prefix("codetrial replay_gate ")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(event["stage"], "closing");
    assert_eq!(event["interview_ms"], 40);
    assert_eq!(event["event_end_ms"], 50);
    assert_eq!(event["turn"], before["turn"]);
    assert!(trace.is_open());
    let after = trace.record("tool_response", start, Outcome::Open);
    for key in [
        "turn",
        "origin",
        "tool_batches",
        "last_tool_response_ms",
        "obligation",
        "obligation_outcome",
    ] {
        assert_eq!(before[key], after[key], "{key}");
    }
}

#[test]
fn a_queued_boundary_ends_when_it_is_handled_not_when_it_arrived() {
    let (start, mut trace) = traced();
    trace.anchor(start);
    let mut output = Vec::new();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let _ = trace.boundary_to(Boundary::Interrupted, start, &mut output);
    let text = String::from_utf8(output).unwrap();
    let event: Value = serde_json::from_str(
        text.trim_end()
            .strip_prefix("codetrial replay_gate ")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(event["interview_ms"], 0);
    assert!(event["event_end_ms"].as_u64().unwrap() >= 20);
}
