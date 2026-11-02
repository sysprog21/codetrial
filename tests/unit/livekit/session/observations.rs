use super::*;
use crate::livekit::session::execute_tool_call;
use serde_json::json;

fn call(name: &str, requested: bool) -> GeminiFunctionCall {
    GeminiFunctionCall {
        id: "synthetic".into(),
        name: name.into(),
        args: json!({"requested": requested}),
    }
}

fn timing(sent: bool) -> ToolTiming {
    ToolTiming {
        batch_ms: 10,
        execute_ms: 2,
        board_ms: 3,
        send_ms: 4,
        sent,
    }
}

#[test]
fn only_declared_error_free_responses_are_accepted_but_send_status_stays_separate() {
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::for_interview(Instant::now(), 0, 4);
    activity.latency.bind("synthetic", 4, 1, 1500);
    let answers = vec![
        (call(TOOL_READ_EDITOR, true), json!({"result":"synthetic"})),
        (call(TOOL_READ_BOARD, true), json!({"error":"disabled"})),
        (
            call("unknown_private_text", true),
            json!({"result":"ignored"}),
        ),
    ];
    for sent in [false, true] {
        let record = tool_timing(
            &mut activity,
            &state,
            &answers,
            HintCounters::capture(&state),
            timing(sent),
        );
        assert_eq!(record["accepted_tool_names"], json!([TOOL_READ_EDITOR]));
        assert_eq!(record["sent"], sent);
        assert!(!record.to_string().contains("unknown_private_text"));
        assert_eq!(record["hint_given"], false);
        assert_eq!(record["tool_execute_ms"], 2);
        assert_eq!(record["board_resend_ms"], 3);
        assert_eq!(record["tool_send_ms"], 4);
    }
}

#[test]
fn hint_context_distinguishes_released_volunteered_withheld_and_exhausted_calls() {
    for (ladder, requested, expected) in [
        (&["first", "last"][..], true, true),
        (&["last"][..], true, false),
        (&[][..], true, false),
        (&[][..], false, true),
    ] {
        let mut state = RuntimeState {
            hint_ladder: ladder,
            ..RuntimeState::default()
        };
        let before = HintCounters::capture(&state);
        let call = call(TOOL_LOG_HINT, requested);
        let response = execute_tool_call(&mut state, &call);
        let mut activity = RuntimeActivity::for_interview(Instant::now(), 0, 4);
        activity.latency.bind("synthetic", 4, 1, 1500);
        let record = tool_timing(
            &mut activity,
            &state,
            &[(call, response)],
            before,
            timing(true),
        );
        assert_eq!(record["accepted_tool_names"], json!([TOOL_LOG_HINT]));
        assert_eq!(
            record["hint_given"],
            expected,
            "ladder={} requested={requested}",
            ladder.len()
        );
    }
}
