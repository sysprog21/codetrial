use crate::test_isolation as isolation;

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
            &mut None,
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

#[test]
fn executed_tool_capture_precedes_send_and_telemetry_gates() {
    if isolation::isolated!("CODETRIAL_ACQUISITION_CAPTURE_TEST") {
        return;
    }
    use crate::livekit::probe::acquisition::{Capture, Failure, Limits, install};
    let capture = Capture::new(
        "acquisition-fixture",
        7,
        Limits {
            batches: 3,
            calls: 4,
            bytes: 16384,
        },
    )
    .unwrap();
    let installed = install(&capture).unwrap();
    assert!(install(&capture).is_err());
    let mut state = RuntimeState {
        code: "def solve(items):\n    return 'authored-result-sentinel'\n".into(),
        ..RuntimeState::default()
    };
    state.evidence_ledger.code.parser_observation = Some(crate::agent::CodeObservation::Parsed);
    state.evidence_ledger.code.substantive_revision = 1;
    let now = Instant::now();
    let mut activity = RuntimeActivity::for_interview(now, 0, 7);
    activity.latency.bind("acquisition-fixture", 7, 1, 1500);
    activity.latency.anchor(now);
    activity.latency.prompt(now);
    let call = |id: &str| GeminiFunctionCall {
        id: id.into(),
        name: TOOL_READ_EDITOR.into(),
        args: json!({"fromLine":1}),
    };
    let answers = execute_timed_tool_batch(
        &mut state,
        &mut activity,
        vec![call("actual-received-call-id"), call("second-id")],
        0,
        false,
        &mut None,
    );
    let snapshot = capture.snapshot();
    assert_eq!(
        snapshot.batches.len(),
        1,
        "executed answers must survive without any later send/timing observation"
    );
    let batch = &snapshot.batches[0];
    for (captured, (executed, result)) in batch.calls.iter().zip(&answers) {
        assert_eq!(captured.id, executed.id);
        assert_eq!(captured.name, executed.name);
        assert_eq!(&captured.result, result);
    }
    assert_eq!(batch.calls.len(), 2);
    assert!(
        batch.calls[0].result["result"]
            .as_str()
            .unwrap()
            .contains("authored-result-sentinel")
    );
    assert!(batch.calls[0].result.get("editorReview").is_none());
    assert!(batch.calls[1].result["editorReview"].as_str().is_some());
    assert_eq!(batch.local_send_completed, None);
    assert_eq!(batch.identity.socket, 1);
    assert!(batch.identity.trace_turn.is_some());
    let record = tool_timing(
        &mut activity,
        &state,
        &answers,
        HintCounters::capture(&state),
        timing(false),
    );
    assert!(!record.to_string().contains("actual-received-call-id"));
    assert!(!record.to_string().contains("authored-result-sentinel"));
    activity.latency.set_socket(2);
    execute_timed_tool_batch(
        &mut state,
        &mut activity,
        vec![call("replacement-id")],
        1,
        false,
        &mut None,
    );
    assert_eq!(capture.snapshot().batches[1].identity.socket, 2);
    activity.latency.bind("foreign-room", 8, 3, 1500);
    execute_timed_tool_batch(
        &mut state,
        &mut activity,
        vec![call("foreign-id")],
        1,
        false,
        &mut None,
    );
    assert_eq!(capture.snapshot().failure, Some(Failure::Scope));
    assert_eq!(capture.snapshot().batches.len(), 2);
    drop(installed);
    execute_timed_tool_batch(
        &mut state,
        &mut activity,
        vec![call("uninstalled-id")],
        1,
        false,
        &mut None,
    );
    assert_eq!(capture.snapshot().batches.len(), 2);
    let _reinstalled = install(&capture).unwrap();
}

/// A page from the line a definition starts at stops where the definition
/// ends: part of the buffer, so like any later page it moves no baseline and
/// carries no review instruction, which a full read does.
#[test]
fn a_targeted_editor_read_carries_no_review_and_a_full_one_does() {
    const CODE: &str =
        "import math\n\ndef f(values):\n    return sum(values)\n\n\ndef g():\n    return 1\n";
    let read = |language: &str, args: serde_json::Value| {
        let mut state = RuntimeState {
            language: language.into(),
            code: CODE.into(),
            code_shown: "def f(values):\n    pass\n".into(),
            ..RuntimeState::default()
        };
        state.evidence_ledger.code.parser_observation = Some(crate::agent::CodeObservation::Parsed);
        state.evidence_ledger.code.substantive_revision = 2;
        let call = GeminiFunctionCall {
            id: "read".into(),
            name: crate::runtime::TOOL_READ_EDITOR.into(),
            args,
        };
        let answers = execute_tool_batch_observed(&mut state, vec![call], 1, |_, _, _| {});
        (answers[0].1.clone(), state.code == state.code_shown)
    };
    let (answer, shown) = read("python", json!({"fromLine": 3}));
    let text = answer["result"].as_str().unwrap_or_default();
    assert!(
        text.contains("3| def f(values):") && !text.contains("def g"),
        "{text}"
    );
    assert!(answer.get("editorReview").is_none(), "{answer}");
    assert!(!shown, "a definition read leaves the baseline");

    // A later line no definition starts at is the page it always was, and a
    // buffer with no map pages the same from any line.
    for (language, args) in [
        ("python", json!({"fromLine": 2})),
        ("rust", json!({"fromLine": 3})),
    ] {
        let (answer, shown) = read(language, args.clone());
        let text = answer["result"].as_str().unwrap_or_default();
        assert!(text.contains("def g"), "{args}: {text}");
        assert!(answer.get("editorReview").is_none(), "{args}");
        assert!(!shown, "{args}");
    }

    // A full read, and the parameters the declaration does not offer, read it
    // all, move the baseline and carry the review.
    for args in [
        json!({}),
        json!({"fromLine": 1}),
        json!({"symbol": "f"}),
        json!({"outline": true}),
    ] {
        let (answer, shown) = read("python", args.clone());
        assert!(
            answer["editorReview"].as_str().is_some(),
            "{args}: {answer}"
        );
        assert!(shown, "{args}");
    }
}

/// A test report run on other code says so ahead of everything else, on a
/// definition read as on a full one.
#[test]
fn every_editor_read_says_first_when_its_test_report_is_stale() {
    for args in [json!({}), json!({"fromLine": 3})] {
        let mut state = RuntimeState {
            language: "python".into(),
            code: "import math\n\ndef f(values):\n    return sum(values)\n".into(),
            last_test_run: Some(json!({"passed": 1, "failed": 0})),
            last_test_run_code: Some(crate::agent::TestedCode {
                language: "python".into(),
                code: "other code".into(),
            }),
            ..RuntimeState::default()
        };
        let call = GeminiFunctionCall {
            id: "read".into(),
            name: crate::runtime::TOOL_READ_EDITOR.into(),
            args: args.clone(),
        };
        let answers = execute_tool_batch_observed(&mut state, vec![call], 1, |_, _, _| {});
        let text = answers[0].1["result"].as_str().unwrap_or_default();
        assert!(
            text.starts_with(
                "The displayed browser report belongs to different source or language."
            ),
            "{args}: {text}"
        );
    }
}

#[test]
fn batch_attaches_review_to_a_complete_read_not_a_later_targeted_read() {
    for full_first in [true, false] {
        let mut state = RuntimeState {
            language: "python".into(),
            code: "import math\n\ndef f(values):\n    return sum(values)\n".into(),
            code_shown: "def f(values):\n    pass\n".into(),
            ..RuntimeState::default()
        };
        state.evidence_ledger.code.parser_observation = Some(crate::agent::CodeObservation::Parsed);
        state.evidence_ledger.code.substantive_revision = 2;
        let call = |args| GeminiFunctionCall {
            id: "read".into(),
            name: TOOL_READ_EDITOR.into(),
            args,
        };
        let targeted = json!({"fromLine": 3});
        let calls = if full_first {
            vec![call(json!({})), call(targeted)]
        } else {
            vec![call(targeted), call(json!({}))]
        };
        let answers = execute_tool_batch_observed(&mut state, calls, 1, |_, _, _| {});
        let full = usize::from(!full_first);
        assert!(answers[full].1["editorReview"].as_str().is_some());
        assert!(answers[1 - full].1.get("editorReview").is_none());
    }
}
