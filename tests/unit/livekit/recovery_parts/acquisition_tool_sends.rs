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

        // Every executed call was received first, and a call that arrived under
        // a discard says so.
        assert_eq!(snapshot.receipts.len(), 1);
        assert_eq!(snapshot.receipts[0].id, "actual-tool-send");
        assert_eq!(snapshot.receipts[0].batch, Some(0));

        // A hold silences the interviewer as a discard does: the call is still
        // answered, and neither kind is to be tied to a later reply.
        assert_eq!(snapshot.receipts[0].discarded, held || discarded);
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

async fn acquisition_reply_rig() -> (
    Rig,
    std::sync::Arc<crate::livekit::probe::acquisition::Capture>,
) {
    use crate::livekit::probe::acquisition::{Capture, Limits};
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.turn.activity.latency.bind("recovery-test", 7, 1, 1500);
    let capture = Capture::new(
        "recovery-test",
        7,
        Limits {
            batches: 2,
            calls: 4,
            bytes: 65536,
        },
    )
    .unwrap();
    (rig, capture)
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

        // Received before the acknowledgement gate, so cancellation at any gate
        // leaves the call on record, executed only if the batch got that far.
        assert_eq!(snapshot.receipts.len(), 1, "{stage:?}");
        assert_eq!(snapshot.receipts[0].id, "cancelled-owner");
        assert_eq!(
            snapshot.receipts[0].batch,
            executed.then_some(0),
            "{stage:?}"
        );
        assert!(!snapshot.receipts[0].discarded);
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

async fn acquisition_event(rig: &mut Rig, event: crate::gemini::GeminiEvent) {
    let mut context = rig.turn.context(
        &mut rig.output,
        &mut rig.gemini,
        &mut rig.board,
        &mut rig.media,
    );
    session::handle_gemini_event(&rig.room, &mut context, event, Interruptible::Yes)
        .await
        .unwrap();
}

#[tokio::test]
async fn acquisition_keeps_the_interviewer_segments_the_page_was_sent() {
    if isolated("acquisition_keeps_the_interviewer_segments_the_page_was_sent") {
        return;
    }
    use crate::gemini::GeminiEvent;
    use crate::livekit::probe::acquisition::install;
    let (mut rig, capture) = acquisition_reply_rig().await;
    let installed = install(&capture).unwrap();
    acquisition_receive_editor(&mut rig, "before-reply").await;
    acquisition_event(
        &mut rig,
        GeminiEvent::OutputTranscript("The answer ".into()),
    )
    .await;
    acquisition_event(&mut rig, GeminiEvent::OutputTranscript("is 42.".into())).await;
    // The candidate's own speech closes with the same turn and is not a reply.
    rig.media.identity = Some("candidate".into());
    acquisition_event(&mut rig, GeminiEvent::InputTranscript("my words".into())).await;
    acquisition_event(&mut rig, GeminiEvent::TurnComplete).await;
    acquisition_event(&mut rig, GeminiEvent::OutputTranscript("Next.".into())).await;
    let _ = rig.old_frames().await;
    let snapshot = capture.snapshot();
    drop(installed);
    assert_eq!(snapshot.failure, None);
    let replies: Vec<_> = snapshot
        .replies
        .iter()
        .map(|reply| (reply.text.as_str(), reply.final_segment, reply.views))
        .collect();

    // The first turn's newest view, sealed by the turn's end; the next turn's
    // segment is still open and is its own reply.
    assert_eq!(
        replies,
        [("The answer is 42.", true, 3), ("Next.", false, 1)]
    );
    assert_ne!(snapshot.replies[0].id, snapshot.replies[1].id);
    assert!(snapshot.replies.iter().all(|reply| !reply.revised));
    // The tool call preceded both, and was answered before either.
    assert!(
        snapshot
            .replies
            .iter()
            .all(|reply| (reply.receipts_before, reply.batches_before) == (1, 1))
    );
}

#[tokio::test]
async fn acquisition_seals_the_line_a_cut_marked_before_the_next_words() {
    if isolated("acquisition_seals_the_line_a_cut_marked_before_the_next_words") {
        return;
    }
    use crate::gemini::GeminiEvent;
    use crate::livekit::probe::acquisition::install;
    let (mut rig, capture) = acquisition_reply_rig().await;
    let installed = install(&capture).unwrap();
    acquisition_event(&mut rig, GeminiEvent::OutputTranscript("Cut words.".into())).await;
    rig.turn
        .state
        .interviewer_delivery
        .cancel(crate::agent::DeliveryCut::Generation, Instant::now());
    acquisition_event(&mut rig, GeminiEvent::OutputTranscript("New words.".into())).await;
    let snapshot = capture.snapshot();
    drop(installed);
    let replies: Vec<_> = snapshot
        .replies
        .iter()
        .map(|reply| (reply.text.as_str(), reply.final_segment))
        .collect();
    assert_eq!(replies, [("Cut words.", true), ("New words.", false)]);
}

#[tokio::test]
async fn acquisition_marks_an_ending_the_guard_swallowed() {
    if isolated("acquisition_marks_an_ending_the_guard_swallowed") {
        return;
    }
    use crate::gemini::GeminiEvent;
    use crate::livekit::probe::acquisition::{Ending, install};
    let (mut rig, capture) = acquisition_reply_rig().await;
    let installed = install(&capture).unwrap();
    acquisition_event(&mut rig, GeminiEvent::OutputTranscript("Said.".into())).await;
    acquisition_event(&mut rig, GeminiEvent::TurnComplete).await;
    acquisition_event(&mut rig, GeminiEvent::OutputTranscript("More.".into())).await;
    acquisition_event(&mut rig, GeminiEvent::Interrupted).await;
    acquisition_event(&mut rig, GeminiEvent::TurnComplete).await;
    let snapshot = capture.snapshot();
    drop(installed);
    let seen: Vec<_> = snapshot
        .boundaries
        .iter()
        .map(|boundary| (boundary.ending, boundary.accepted, boundary.replies_before))
        .collect();

    // The ending that follows an interruption closes nothing: the provider
    // sends it for the generation the interruption already cut.
    assert_eq!(
        seen,
        [
            (Ending::Complete, true, 1),
            (Ending::Interrupted, true, 2),
            (Ending::Complete, false, 2)
        ]
    );
}

#[tokio::test]
async fn acquisition_does_not_take_a_departed_candidates_words_for_a_reply() {
    if isolated("acquisition_does_not_take_a_departed_candidates_words_for_a_reply") {
        return;
    }
    use crate::gemini::GeminiEvent;
    use crate::livekit::probe::acquisition::install;
    let (mut rig, capture) = acquisition_reply_rig().await;
    rig.media.identity = Some("candidate".into());
    let installed = install(&capture).unwrap();
    acquisition_event(&mut rig, GeminiEvent::InputTranscript("my words".into())).await;

    // The candidate leaves before the turn closes, so the close publishes their
    // final segment without an identity, as the agent.
    rig.media.identity = None;
    acquisition_event(&mut rig, GeminiEvent::TurnComplete).await;
    let snapshot = capture.snapshot();
    drop(installed);
    assert_eq!(snapshot.failure, None);
    assert!(snapshot.replies.is_empty(), "{:?}", snapshot.replies);
}

/// The report the fixture leaves behind, from a script that exercises every
/// shape the JS consumer reads: a reply that cites an executed call, a plain
/// completed reply, and an interrupted one whose late ending is swallowed.
/// Wall-clock readings and the byte counts that include them are replaced with
/// zero, because a fixture cannot hold the time the test ran; the consumer's
/// tests put times back.
#[tokio::test]
async fn acquisition_report_is_the_wire_fixture_the_consumer_reads() {
    if isolated("acquisition_report_is_the_wire_fixture_the_consumer_reads") {
        return;
    }
    use crate::gemini::GeminiEvent;
    use crate::livekit::probe::acquisition::{Arming, Limits, install};
    let (mut rig, _) = acquisition_reply_rig().await;
    let arming = Arming::detached(Limits {
        batches: 8,
        calls: 8,
        bytes: 65536,
    });
    arming.on_assignment("recovery-test");
    arming.on_bind("recovery-test", 7);
    let capture = arming.capture().unwrap();
    let installed = install(&capture).unwrap();
    acquisition_receive_editor(&mut rig, "call-1").await;
    for text in ["REPLY-A one. ", "two."] {
        acquisition_event(&mut rig, GeminiEvent::OutputTranscript(text.into())).await;
    }
    acquisition_event(&mut rig, GeminiEvent::TurnComplete).await;
    acquisition_event(&mut rig, GeminiEvent::OutputTranscript("REPLY-B.".into())).await;
    acquisition_event(&mut rig, GeminiEvent::TurnComplete).await;
    acquisition_event(
        &mut rig,
        GeminiEvent::OutputTranscript("REPLY-C cut".into()),
    )
    .await;
    acquisition_event(&mut rig, GeminiEvent::Interrupted).await;
    acquisition_event(&mut rig, GeminiEvent::TurnComplete).await;
    let _ = rig.old_frames().await;
    let mut report = arming.report();
    drop(installed);
    fn zero_clocks(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, inner) in map.iter_mut() {
                    // Byte counts embed the digits of the clock they carry; the
                    // call count does not, and is kept.
                    if key == "emitted_unix_ms"
                        || (key.starts_with("retained_") && key.ends_with("_bytes"))
                    {
                        *inner = 0.into();
                    } else {
                        zero_clocks(inner);
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(zero_clocks),
            _ => {}
        }
    }
    zero_clocks(&mut report);
    let path = "tests/fixtures/acquisition-report.json";
    let rendered = format!("{}\n", serde_json::to_string_pretty(&report).unwrap());
    if std::env::var_os("CODETRIAL_UPDATE_FIXTURES").is_some() {
        std::fs::write(path, &rendered).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        rendered,
        "regenerate with CODETRIAL_UPDATE_FIXTURES=1"
    );
}
