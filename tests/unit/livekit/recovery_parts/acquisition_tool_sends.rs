async fn acquisition_receive_tool(rig: &mut Rig, id: &str, tool: &str) {
    let mut context = rig.turn.context(
        &mut rig.output,
        &mut rig.gemini,
        &mut rig.board,
        &mut rig.media,
    );
    session::handle_gemini_event(
        &rig.room,
        &mut context,
        crate::gemini::GeminiEvent::ToolCall(vec![crate::gemini::GeminiFunctionCall {
            id: id.into(),
            name: tool.into(),
            args: serde_json::json!({"fromLine":1}),
        }]),
        Interruptible::Yes,
    )
    .await
    .unwrap();
}

async fn acquisition_receive_editor(rig: &mut Rig, id: &str) {
    acquisition_receive_tool(rig, id, crate::runtime::TOOL_READ_EDITOR).await;
}

async fn acquisition_actual_tool_send(failed: bool) {
    use crate::livekit::probe::acquisition::{Capture, Limits, install};
    for (held, discarded) in [(false, false), (true, false), (false, true)] {
        let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
        rig.turn.state.code = "def solve(): return 'send-result-sentinel'".into();
        if held {
            rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
                since: Instant::now(),
            };
        }
        rig.turn.activity.socket.discarding_output = discarded;
        let now = Instant::now();
        rig.turn.activity.latency.bind("recovery-test", 7, 1, 1500);
        rig.turn.activity.latency.anchor(now);
        rig.turn.activity.latency.prompt(now);
        let capture = Capture::new(
            "recovery-test",
            7,
            Limits {
                batches: 1,
                calls: 1,
                bytes: 65536,
            },
        )
        .unwrap();
        let installed = install(&capture).unwrap();
        if failed {
            rig.gemini.shutdown().await.unwrap();
        }
        acquisition_receive_editor(&mut rig, "actual-tool-send").await;
        // Join the real peer before any assertion can end this fixture.
        let frames = rig.old_frames().await;
        let snapshot = capture.snapshot();
        drop(installed);
        assert_eq!(snapshot.failure, None);
        assert_eq!(snapshot.batches.len(), 1);
        let batch = &snapshot.batches[0];
        assert_eq!(batch.calls.len(), 1);
        assert_eq!(batch.calls[0].id, "actual-tool-send");
        assert_eq!(batch.calls[0].name, crate::runtime::TOOL_READ_EDITOR);
        assert!(
            batch.calls[0].result["result"]
                .as_str()
                .unwrap()
                .contains("send-result-sentinel")
        );
        assert_eq!(
            batch.local_send_completed,
            Some(!failed),
            "actual owner await result, independent of held/discarded timing gates"
        );
        assert_eq!(
            snapshot.retained_bytes,
            serde_json::to_vec(&snapshot.batches).unwrap().len()
        );
        let responses = frames
            .iter()
            .filter_map(|frame| frame.pointer("/toolResponse/functionResponses"))
            .filter_map(serde_json::Value::as_array)
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), usize::from(!failed));
        if let Some(response) = responses.first() {
            assert_eq!(response["id"], "actual-tool-send");
            assert!(
                response["response"] == batch.calls[0].result,
                "actual wire result matches captured execution"
            );
        }
    }
}

#[tokio::test]
async fn acquisition_tool_send_ok_records_real_completion() {
    if isolated("acquisition_tool_send_ok_records_real_completion") {
        return;
    }
    acquisition_actual_tool_send(false).await;
}

#[tokio::test]
async fn acquisition_tool_send_error_records_returned_failure() {
    if isolated("acquisition_tool_send_error_records_returned_failure") {
        return;
    }
    acquisition_actual_tool_send(true).await;
}

fn acquisition_gate_capture(
    rig: &mut Rig,
) -> std::sync::Arc<crate::livekit::probe::acquisition::Capture> {
    use crate::livekit::probe::acquisition::{Capture, Limits};
    let now = Instant::now();
    rig.turn.activity.latency.bind("recovery-test", 7, 1, 1500);
    rig.turn.activity.latency.anchor(now);
    rig.turn.activity.latency.prompt(now);
    Capture::new(
        "recovery-test",
        7,
        Limits {
            batches: 2,
            calls: 2,
            bytes: 65536,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn acquisition_controlled_owner_await_cancellation_preserves_execution_boundary() {
    if isolated("acquisition_controlled_owner_await_cancellation_preserves_execution_boundary") {
        return;
    }
    use crate::livekit::probe::acquisition::{
        install,
        owner::{self, Gate, Stage},
    };
    for stage in [
        Stage::Acknowledgement,
        Stage::BoardSettlement,
        Stage::BoardResend,
        Stage::ToolSend,
    ] {
        let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
        let tool = match stage {
            Stage::BoardSettlement => {
                rig.turn.state.example_drawing_enabled = true;
                crate::runtime::TOOL_READ_BOARD
            }
            Stage::BoardResend => {
                rig.turn.state.interview_mode = crate::agent::InterviewMode::Whiteboard;
                rig.board.seed_recovery_image(vec![0x5a; 64]);
                crate::runtime::TOOL_READ_BOARD
            }
            _ => crate::runtime::TOOL_READ_EDITOR,
        };
        let capture = acquisition_gate_capture(&mut rig);
        let installed = install(&capture).unwrap();
        let gate = Gate::new(&capture, 1, stage, None).unwrap();
        let held = owner::install(&gate).unwrap();
        let mut call = Box::pin(acquisition_receive_tool(&mut rig, "cancelled-owner", tool));
        tokio::select! {
            entered = gate.entered() => { entered.unwrap(); },
            () = &mut call => panic!("owner must reach its selected controlled await"),
            () = tokio::time::sleep(Duration::from_secs(2)) => panic!("controlled owner gate did not enter"),
        }
        drop(call);
        let frames = rig.old_frames().await;
        let snapshot = capture.snapshot();
        drop(held);
        drop(installed);
        let executed = matches!(stage, Stage::BoardResend | Stage::ToolSend);
        assert_eq!(snapshot.failure, None);
        assert_eq!(snapshot.batches.len(), usize::from(executed), "{stage:?}");
        if let Some(batch) = snapshot.batches.first() {
            assert_eq!(batch.calls[0].id, "cancelled-owner");
            assert_eq!(
                batch.local_send_completed, None,
                "controlled cancellation is unknown, not Err"
            );
        }
        assert!(
            frames.is_empty(),
            "controlled gate is before underlying I/O polling"
        );
    }
}

#[tokio::test]
async fn acquisition_admitted_ticket_completes_original_capture_after_reinstallation() {
    if isolated("acquisition_admitted_ticket_completes_original_capture_after_reinstallation") {
        return;
    }
    use crate::livekit::probe::acquisition::{
        install,
        owner::{self, Gate, Stage},
    };
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let original = acquisition_gate_capture(&mut rig);
    let replacement = acquisition_gate_capture(&mut rig);
    let installed = install(&original).unwrap();
    let gate = Gate::new(&original, 1, Stage::ToolSend, None).unwrap();
    let held = owner::install(&gate).unwrap();
    let actor = async {
        gate.entered().await.unwrap();
        assert_eq!(original.snapshot().batches.len(), 1);
        drop(installed);
        let replacement_installed = install(&replacement).unwrap();
        gate.release();
        replacement_installed
    };
    let ((), replacement_installed) = tokio::join!(
        acquisition_receive_editor(&mut rig, "original-ticket"),
        actor
    );
    let frames = rig.old_frames().await;
    assert_eq!(tool_response_ids(&frames), ["original-ticket"]);
    let first = original.snapshot();
    let second = replacement.snapshot();
    drop(replacement_installed);
    drop(held);
    assert_eq!(first.failure, None);
    assert_eq!(first.batches[0].local_send_completed, Some(true));
    assert_eq!(first.batches[0].calls[0].id, "original-ticket");
    assert_eq!(second.failure, None);
    assert!(
        second.batches.is_empty(),
        "completion must never resolve the current ACTIVE capture"
    );
}

async fn acquisition_timing_gate_divergence(
    initial: bool,
    transition: crate::livekit::probe::acquisition::owner::Transition,
) {
    use crate::livekit::probe::acquisition::{
        install,
        owner::{self, Gate, Stage, Transition},
    };
    for failed in [false, true] {
        let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
        match transition {
            Transition::Hold(_) => {
                if !initial {
                    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
                        since: Instant::now(),
                    };
                }
            }
            Transition::Discard(_) => rig.turn.activity.socket.discarding_output = !initial,
        }
        let capture = acquisition_gate_capture(&mut rig);
        rig.turn
            .activity
            .turn_timing
            .note_transcript(Instant::now());
        let installed = install(&capture).unwrap();
        let gate = Gate::new(&capture, 1, Stage::ToolSend, Some(transition)).unwrap();
        let held = owner::install(&gate).unwrap();
        if failed {
            rig.gemini.shutdown().await.unwrap();
        }
        let actor = async {
            gate.entered().await.unwrap();
            gate.release();
        };
        let ((), ()) = tokio::join!(
            acquisition_receive_editor(&mut rig, "divergent-gates"),
            actor
        );
        let frames = rig.old_frames().await;
        let snapshot = capture.snapshot();
        drop(held);
        drop(installed);
        let later =
            !rig.turn.state.interviewer_silenced() && !rig.turn.activity.socket.discarding_output;
        assert_eq!(
            later, !initial,
            "the synthetic transition must actually diverge the two gates"
        );
        let timing = rig
            .turn
            .activity
            .turn_timing
            .finish(
                Instant::now(),
                crate::livekit::timing::TimingOutcome::NoAudio,
            )
            .unwrap();
        assert_eq!(
            timing.tool_calls,
            u64::from(initial),
            "initial timed sample independently controls execution accounting"
        );
        assert_eq!(timing.tool_response_errors, u64::from(initial && failed));
        assert_eq!(snapshot.failure, None);
        assert_eq!(snapshot.batches.len(), 1);
        assert_eq!(snapshot.batches[0].local_send_completed, Some(!failed));
        assert_eq!(tool_response_ids(&frames).len(), usize::from(!failed));
    }
}

fn acquisition_check_later_log(output: &std::process::Output, later: bool) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let logged = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("codetrial latency "))
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|record| record["stage"] == "tool_response")
        .count();
    assert_eq!(
        logged,
        if later { 2 } else { 0 },
        "later log gate observed separately from the initial timed sample"
    );
}

#[tokio::test]
async fn acquisition_tool_send_hold_turns_timing_off_after_initial_sample() {
    if isolated_checked(
        "acquisition_tool_send_hold_turns_timing_off_after_initial_sample",
        |out| acquisition_check_later_log(out, false),
    ) {
        return;
    }
    acquisition_timing_gate_divergence(
        true,
        crate::livekit::probe::acquisition::owner::Transition::Hold(true),
    )
    .await;
}

#[tokio::test]
async fn acquisition_tool_send_release_turns_logging_on_after_untimed_execution() {
    if isolated_checked(
        "acquisition_tool_send_release_turns_logging_on_after_untimed_execution",
        |out| acquisition_check_later_log(out, true),
    ) {
        return;
    }
    acquisition_timing_gate_divergence(
        false,
        crate::livekit::probe::acquisition::owner::Transition::Hold(false),
    )
    .await;
}

#[tokio::test]
async fn acquisition_tool_send_discard_turns_logging_off_after_timed_execution() {
    if isolated_checked(
        "acquisition_tool_send_discard_turns_logging_off_after_timed_execution",
        |out| acquisition_check_later_log(out, false),
    ) {
        return;
    }
    acquisition_timing_gate_divergence(
        true,
        crate::livekit::probe::acquisition::owner::Transition::Discard(true),
    )
    .await;
}

#[tokio::test]
async fn acquisition_tool_send_discard_end_turns_logging_on_after_untimed_execution() {
    if isolated_checked(
        "acquisition_tool_send_discard_end_turns_logging_on_after_untimed_execution",
        |out| acquisition_check_later_log(out, true),
    ) {
        return;
    }
    acquisition_timing_gate_divergence(
        false,
        crate::livekit::probe::acquisition::owner::Transition::Discard(false),
    )
    .await;
}

#[tokio::test]
async fn acquisition_consumed_gate_does_not_hold_a_later_tool_batch() {
    if isolated("acquisition_consumed_gate_does_not_hold_a_later_tool_batch") {
        return;
    }
    use crate::livekit::probe::acquisition::{
        install,
        owner::{self, Gate, Stage},
    };
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let capture = acquisition_gate_capture(&mut rig);
    let installed = install(&capture).unwrap();
    let gate = Gate::new(&capture, 1, Stage::ToolSend, None).unwrap();
    let held = owner::install(&gate).unwrap();
    assert!(owner::install(&gate).is_err());
    let actor = async {
        gate.entered().await.unwrap();
        gate.release();
    };
    let ((), ()) = tokio::join!(acquisition_receive_editor(&mut rig, "first-gated"), actor);
    tokio::time::timeout(
        Duration::from_secs(2),
        acquisition_receive_editor(&mut rig, "later-ungated"),
    )
    .await
    .unwrap();
    let frames = rig.old_frames().await;
    let snapshot = capture.snapshot();
    drop(held);
    drop(installed);
    assert_eq!(tool_response_ids(&frames), ["first-gated", "later-ungated"]);
    assert_eq!(snapshot.failure, None);
    assert_eq!(snapshot.batches.len(), 2);
    assert!(
        snapshot
            .batches
            .iter()
            .all(|batch| batch.local_send_completed == Some(true))
    );
}
