use super::*;
use crate::livekit::session::{execute_tool_batch_observed, execute_tool_call};
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

#[test]
fn closing_context_observes_pending_and_completed_end_without_changing_either() {
    for (requested, ended, expected) in [
        (false, false, false),
        (true, false, true),
        (false, true, true),
    ] {
        let state = RuntimeState {
            end_requested: requested,
            ended,
            ..RuntimeState::default()
        };
        let mut activity = RuntimeActivity::for_interview(Instant::now(), 0, 4);
        let record = tool_timing(
            &mut activity,
            &state,
            &[(call(TOOL_READ_EDITOR, true), json!({"result":"synthetic"}))],
            HintCounters::capture(&state),
            timing(true),
        );
        assert_eq!(record["closing"], expected);
        assert_eq!(state.end_requested, requested);
        assert_eq!(state.ended, ended);
    }
}

#[test]
fn accepted_close_request_is_observed_before_tool_batch_response_timing() {
    const NAME: &str = "livekit::session::observations::tests::accepted_close_request_is_observed_before_tool_batch_response_timing";
    if let Ok(mode) = std::env::var("CODETRIAL_REPLAY_CLOSE_TEST") {
        let refused = mode == "refused";
        let fresh_state = || {
            let mut state = RuntimeState {
                interview_loop: crate::agent::InterviewLoop::CodingBehavioral,
                code: "def solve(nums):\n    return sorted(nums)\n".into(),
                language: "python".into(),
                round_transition_seen: !refused,
                ..RuntimeState::default()
            };
            crate::livekit::tests::receive_test_run(&mut state);
            for phase in ["test", "optimizations"] {
                crate::agent::record_framework_evidence(
                    &mut state,
                    &json!({"phase":phase, "source":crate::livekit::tests::observed_source(phase), "kind":"observed", "confidence":90, "summary":"synthetic evidence"}),
                ).unwrap();
            }
            state
        };
        let mut state = fresh_state();
        let start = Instant::now();
        let mut activity = RuntimeActivity::for_interview(start, 0, 7);
        activity.latency.bind("closing-test", 7, 1, 1500);
        activity.latency.anchor(start);
        let answers = execute_timed_tool_batch(
            &mut state,
            &mut activity,
            vec![
                call(TOOL_END_INTERVIEW, true),
                call(TOOL_END_INTERVIEW, true),
            ],
            0,
            true,
        );
        assert_eq!(answers.len(), 2);
        assert!(
            answers
                .iter()
                .all(|(_, value)| value.get("error").is_some() == refused)
        );
        assert_eq!(state.end_requested, !refused);
        assert!(!state.ended);

        // Which call newly requested the end, not only how many records there
        // were: observing the repeat instead of the first would still give one.
        let mut again = fresh_state();
        let mut flags = Vec::new();
        execute_tool_batch_observed(
            &mut again,
            vec![
                call(TOOL_END_INTERVIEW, true),
                call(TOOL_END_INTERVIEW, true),
            ],
            0,
            |_, _, newly_requested| flags.push(newly_requested),
        );
        eprintln!("close_flags={flags:?}");
        return;
    }
    for mode in ["accepted", "refused"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env("CODETRIAL_REPLAY_CLOSE_TEST", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records = String::from_utf8_lossy(&output.stderr)
            .lines()
            .filter_map(|line| line.strip_prefix("codetrial replay_gate "))
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let flags = String::from_utf8_lossy(&output.stderr)
            .lines()
            .find_map(|line| line.strip_prefix("close_flags=").map(str::to_owned))
            .unwrap();
        assert_eq!(
            flags,
            if mode == "accepted" {
                "[true, false]"
            } else {
                "[false, false]"
            }
        );
        assert_eq!(records.len(), usize::from(mode == "accepted"));
        if records.is_empty() {
            continue;
        }
        assert_eq!(records[0]["stage"], "close_requested");
        assert!(
            records[0]["interview_ms"].as_i64().unwrap()
                <= records[0]["event_end_ms"].as_i64().unwrap()
        );
        assert!(
            records[0]["event_end_ms"].as_i64().unwrap()
                <= records[0]["clock"]["emitted_ms"].as_i64().unwrap()
        );
    }
}
