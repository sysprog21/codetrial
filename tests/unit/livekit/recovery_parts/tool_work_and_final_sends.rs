// Part of recovery.rs: included in place, so every item here shares that file's
// module.

// Declared before the first run, for every (stage, stop, result) row:
//
//   stage          charges  tool effect  old toolResponse ids      fresh frames
//   resume pending 1        1            [call-1] / [] uncertain   none
//   cold backoff   2        1            [call-1] / [] uncertain   none
//
// The terminal is End or the original deadline. Both break the room loop with
// the terminal intact, so the report path is the one the stop already chose.
// The reply the candidate was owed is set down as a required prompt before the
// first cancellable wait and the stop must not undo it; the continuation flag
// is cleared there too, because the socket it belonged to is gone and no live
// socket will deliver it. These two checks therefore guard against the stop
// undoing them, not against the stop doing the work. Nothing is written after
// the stop, no further open is attempted, and the deadline is not extended.
async fn tool_cancel_matrix(stage: ToolStage) {
    let mut rows = 0;
    for stop in [OpenStop::End, OpenStop::Deadline] {
        for accepted in [true, false] {
            let label = format!("{stage:?} {stop:?} accepted={accepted}");
            let checkpoint = matches!(stage, ToolStage::ResumePending)
                .then(|| serde_json::json!({"resumable": true, "newHandle": "tool-handle"}));
            let (listener, endpoint) = listener().await;
            let mut rig =
                Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), checkpoint).await;
            rig.endpoint = endpoint;
            let old_reader = rig.gemini.inspect_reader_for_test().unwrap();
            wait_for_received_usage(&old_reader).await;
            if !accepted {
                rig.gemini.shutdown().await.unwrap();
            }
            rig.receive_tool_call("call-1").await;
            assert_eq!(rig.turn.state.volunteered_hints, 1, "{label}");
            rig.loops.deadline_at = tokio::time::Instant::now()
                + if matches!(stage, ToolStage::ColdBackoff) {
                    Duration::from_secs(1)
                } else {
                    Duration::from_secs(5)
                };
            let deadline = rig.loops.deadline_at;
            let (probe, mut observed) = mpsc::unbounded_channel();
            rig.loops.recovery_terminal = Some(probe);
            let (unused, _) = mpsc::unbounded_channel();
            let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
            let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
            let (entered, pending) = oneshot::channel();
            let server = tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    assert!(request.len() < 4096);
                    request.push(socket.read_u8().await.unwrap());
                }
                match stage {
                    ToolStage::ResumePending => {
                        entered.send(()).unwrap();
                        closed_tcp(socket).await;
                    }
                    ToolStage::ColdBackoff => {
                        socket
                            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await
                            .unwrap();
                        drop(socket);
                        entered.send(()).unwrap();
                    }
                }
                listener
            });
            let tx = rig.tx.clone();
            let recorder = std::sync::Arc::clone(&rig.recorder);
            let stimulus = async {
                pending.await.unwrap();
                if matches!(stage, ToolStage::ColdBackoff) {
                    let until = tokio::time::timeout(Duration::from_secs(1), async {
                        loop {
                            let mut deadlines = recorder.take_retry_backoffs();
                            if let Some(until) = deadlines.pop() {
                                break until;
                            }
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .unwrap();
                    assert!(deadline < until, "{label}: the stop must beat the backoff");
                }
                stop_pending_open(stop, deadline, &tx, &mut room, &mut observed, &presence_now)
                    .await;
            };
            let (flow, ()) = tokio::join!(rig.run(), stimulus);
            assert!(flow.is_break(), "{label}");
            assert_eq!(rig.loops.deadline_at, deadline, "{label}");
            assert_eq!(
                rig.loops.restarts,
                if matches!(stage, ToolStage::ColdBackoff) {
                    2
                } else {
                    1
                },
                "{label}: the charge survives the stop"
            );
            assert_eq!(rig.turn.state.volunteered_hints, 1, "{label}");
            assert!(rig.turn.activity.owes_prompt(), "{label}: obligation kept");
            assert!(
                !rig.turn.activity.socket.tool_response_outstanding,
                "{label}"
            );
            assert_eq!(rig.turn.activity.live_socket, 1, "{label}");
            assert!(
                rig.gemini
                    .send_text("no socket may take this")
                    .await
                    .is_err(),
                "{label}: nothing is installed after the stop"
            );
            assert!(observed.try_recv().is_err(), "{label}: one terminal");
            assert!(
                !rig.recorder.take().iter().any(|record| matches!(record,
                    room_link::Sent::Data { topic, .. } if topic.as_deref() == Some(crate::runtime::TOPIC_REPORT))),
                "{label}: the stop sends no report itself"
            );
            let old = rig.old_frames().await;
            assert_eq!(
                tool_response_ids(&old),
                if accepted {
                    vec!["call-1".to_string()]
                } else {
                    Vec::new()
                },
                "{label}"
            );
            let listener = server.await.unwrap();
            assert_eq!(
                listener.into_std().unwrap().accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock,
                "{label}: no further open"
            );
            rows += 1;
        }
    }
    assert_eq!(rows, 4);
}

#[tokio::test(flavor = "current_thread")]
async fn pending_tool_work_survives_end_and_deadline_during_a_resume_open() {
    if isolated("pending_tool_work_survives_end_and_deadline_during_a_resume_open") {
        return;
    }
    tool_cancel_matrix(ToolStage::ResumePending).await;
}

#[tokio::test(flavor = "current_thread")]
async fn pending_tool_work_survives_end_and_deadline_during_cold_backoff() {
    if isolated("pending_tool_work_survives_end_and_deadline_during_cold_backoff") {
        return;
    }
    tool_cancel_matrix(ToolStage::ColdBackoff).await;
}

// Declared before the first run. A graceful restart is held while a tool
// continuation is outstanding, even when the candidate has started speaking:
// nothing is retired, nothing is charged, the held restart stays armed, and the
// continuation stays on the live socket that owes it. Expected: the loop
// continues, 0 charges, 1 socket, the restart still armed, the flag still up,
// no frame beyond setup on the live socket, and no tool effect.
#[tokio::test(flavor = "current_thread")]
async fn a_deferred_restart_holds_while_a_tool_continuation_is_outstanding() {
    if isolated("a_deferred_restart_holds_while_a_tool_continuation_is_outstanding") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.prefix = true;
    rig.media.audio_bytes = vec![1, 2];
    rig.media.identity = Some("candidate".into());
    rig.loops.deferred_restart = DeferredRestart(true);
    rig.turn
        .activity
        .note_candidate_voice_activity(true, Instant::now());
    rig.turn.activity.note_tool_response(Instant::now());
    let flow = rig.run().await;
    assert!(flow.is_continue());
    assert_eq!(rig.loops.restarts, 0);
    assert_eq!(rig.turn.activity.live_socket, 1);
    assert!(rig.loops.deferred_restart.is_armed());
    assert!(rig.turn.activity.socket.tool_response_outstanding);
    assert_eq!(rig.turn.state.volunteered_hints, 0);
    assert_eq!(rig.media.audio_bytes, vec![1, 2], "no audio was consumed");
    rig.finish().await;
}

// Declared before the first run. With no continuation outstanding the speech
// prefix runs, and during it the old socket issues `call-2`. Expected: it is
// discarded and counted (discarded_events=1), never executed (effect 0) and
// never answered (no toolResponse on either socket); the transcript fragment is
// kept; the replacement is a single charge; no continuation flag is raised on
// the new socket.
#[tokio::test(flavor = "current_thread")]
async fn a_tool_call_arriving_during_the_speech_prefix_is_discarded_not_executed() {
    if isolated_checked(
        "a_tool_call_arriving_during_the_speech_prefix_is_discarded_not_executed",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.lines().any(|line| line
                    .starts_with("speech handoff: prefix_fragments=1 discarded_events=1 ")),
                "the call must be counted as discarded: {stderr}"
            );
        },
    ) {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.gemini.shutdown().await.unwrap();
    assert!(rig.old.take().unwrap().await.unwrap().is_empty());
    let (old_listener, old_endpoint) = listener().await;
    let old = tokio::spawn(async move {
        use tokio_tungstenite::tungstenite::{Error as WsError, error::ProtocolError};
        let (socket, _) = old_listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut frames = Vec::new();
        let end = socket.next().await.unwrap().unwrap();
        frames.push(serde_json::from_str::<serde_json::Value>(end.to_text().unwrap()).unwrap());
        assert_eq!(frames[0]["realtimeInput"]["audioStreamEnd"], true);
        let call = serde_json::json!({"toolCall": {"functionCalls": [
            {"id": "call-2", "name": "log_hint", "args": {"requested": false}}
        ]}});
        socket
            .send(Message::Text(call.to_string().into()))
            .await
            .unwrap();
        let speech = serde_json::json!({
            "serverContent": {"inputTranscription": {"text": PREFIX_TEXT}},
            "usageMetadata": {"promptTokenCount": 42},
        });
        socket
            .send(Message::Text(speech.to_string().into()))
            .await
            .unwrap();
        while let Some(frame) = socket.next().await {
            match frame {
                Ok(frame) if frame.is_close() => break,
                Ok(frame) => frames.push(serde_json::from_str(frame.to_text().unwrap()).unwrap()),
                Err(WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake)) => break,
                Err(WsError::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("unexpected retired peer error: {error}"),
            }
        }
        frames
    });
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
    rig.gemini = crate::livekit::tests::fixture_session_at(&old_endpoint, &rig.keys, &boot, None)
        .await
        .unwrap();
    rig.old = Some(old);
    rig.expected_old_messages = 2;
    let (fresh, endpoint) = listener().await;
    rig.endpoint = endpoint;
    let server = tokio::spawn(recording_fresh_gemini(fresh, vec![FreshStep::Ready]));
    rig.prefix = true;
    rig.media.audio_bytes = vec![1, 2];
    rig.media.audio_unconfirmed = true;
    rig.media.identity = Some("candidate".into());
    rig.loops.deferred_restart = DeferredRestart(true);
    rig.turn
        .activity
        .note_candidate_voice_activity(true, Instant::now());
    let flow = rig.run().await;
    assert!(flow.is_continue());
    assert_eq!(rig.loops.restarts, 1);
    assert_eq!(rig.turn.state.volunteered_hints, 0, "call-2 never ran");
    assert!(!rig.turn.activity.socket.tool_response_outstanding);
    assert_eq!(rig.turn.activity.live_socket, 2);
    let old = rig.old_frames().await;
    assert_eq!(old.len(), 2, "end of audio and the listening note only");
    assert!(tool_response_ids(&old).is_empty());
    let seen = server.await.unwrap();
    assert_eq!(seen.len(), 1);
    assert!(tool_response_ids(&seen[0].1).is_empty());
    assert!(
        rig.turn
            .state
            .transcript
            .iter()
            .any(|line| line.contains(PREFIX_TEXT)),
        "the prefix transcript is kept"
    );
}

// Non-terminal races on a replacement's final sends, and what replace itself
// owes the room when the replacement fails.

fn test_result_packet(passed: u32, code: &str) -> Packet {
    Packet {
        topic: Some(crate::runtime::TOPIC_TEST_RESULTS.into()),
        sender: Some("candidate".into()),
        payload: serde_json::to_vec(&serde_json::json!({
            "passed": passed, "total": 40, "code": code, "language": "python"
        }))
        .unwrap(),
    }
}

fn code_packet(code: &str) -> Packet {
    Packet {
        sender: Some("candidate".into()),
        topic: Some(crate::runtime::TOPIC_CODE_UPDATE.into()),
        payload: serde_json::to_vec(&serde_json::json!({"code": code, "language": "python"}))
            .unwrap(),
    }
}

fn usage_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|line| line.starts_with("codetrial live_turn_usage "))
        .map(str::to_owned)
        .collect()
}

fn has_field(line: &str, field: &str) -> bool {
    line.split_whitespace().any(|value| value == field)
}

// Declared before running: a Billing refusal of the only key fails the
// replacement after the room's queued work was reduced into the driver but
// before any notice or cleanup could run. replace must hand the round back to
// the loop and count the evictions it never performed.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_replacement_hands_room_work_back_through_replace() {
    if isolated_checked(
        "a_failed_replacement_hands_room_work_back_through_replace",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let ingress = stderr
                .lines()
                .filter(|line| line.starts_with("recovery ingress: "))
                .collect::<Vec<_>>();
            assert_eq!(ingress.len(), 1, "{stderr}");
            for field in [
                "omitted_evictions=2",
                "omitted_contexts=0",
                "dropped_bytes=0",
            ] {
                assert!(has_field(ingress[0], field), "{field}: {}", ingress[0]);
            }
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 1, "old usage drains once: {usage:?}");
            assert!(has_field(&usage[0], "socket=1") && has_field(&usage[0], "cause=tool"));
        },
    ) {
        return;
    }
    let (fresh, endpoint) = listener().await;
    let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
    rig.gemini.input_cause = Some("tool");
    let old = rig.gemini.inspect_reader_for_test().unwrap();
    wait_for_received_usage(&old).await;
    rig.turn.state.coding_minutes = 0;
    rig.turn.state.judged_code = rig.turn.state.code.clone();
    rig.turn.state.judged_language = rig.turn.state.language.clone();
    assert!(!crate::agent::phase_judge_due(&rig.turn.state));
    rig.recorder.seed_evictions(&["duplicate-a", "duplicate-b"]);
    control(
        &rig.tx,
        serde_json::json!({"type":"round_transition", "round":"behavioral"}),
    );
    let server = tokio::spawn(scripted_fresh_gemini(
        fresh,
        vec![FreshStep::Refuse(axum::http::StatusCode::PAYMENT_REQUIRED)],
    ));
    let flow = rig.run().await;
    assert!(flow.is_break());
    assert_eq!(rig.turn.activity.live_exit, LiveOutcome::Billing);
    assert!(old.is_finished());
    assert_eq!(old.pending_usage_count(), 0);
    assert_eq!(
        rig.loops.pending_round_notice,
        Some("skipped"),
        "the round the candidate caused survives the failed replacement"
    );
    let sent = rig.recorder.take();
    assert!(
        !sent.iter().any(|sent| matches!(
            sent,
            room_link::Sent::Data { message, .. } if message["type"] == "round_state"
        )),
        "no notice was published by the failed replacement"
    );
    rig.finish().await;
    let (seen, _) = server.await.unwrap();
    assert_eq!(seen.len(), 1);
}

// The fresh peer for a gated final send. It records what reaches the wire and,
// when told, drops the connection without a closing handshake so the real
// reader finishes and the next write takes the Err branch.
async fn closable_fresh_peer(
    listener: tokio::net::TcpListener,
    usage: u64,
    close: oneshot::Receiver<usize>,
) -> Vec<serde_json::Value> {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
    socket.next().await.unwrap().unwrap();
    socket
        .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            serde_json::json!({"usageMetadata": {"promptTokenCount": usage}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let mut messages = Vec::new();
    let mut close = Some(close);
    loop {
        tokio::select! {
            frame = socket.next() => match frame {
                Some(Ok(frame)) if frame.is_close() => break,
                Some(Ok(frame)) => {
                    messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap());
                }
                _ => break,
            },
            dropped = async { close.as_mut().unwrap().await }, if close.is_some() => {
                close = None;
                if let Ok(prior) = dropped {
                    // Drop only after the frames written before the failure
                    // were read, so a reset cannot discard them unseen.
                    while messages.len() < prior {
                        let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
                            .await
                            .expect("the prior writes reach the peer")
                            .unwrap()
                            .unwrap();
                        messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap());
                    }
                    drop(socket);
                    break;
                }
            }
        }
    }
    messages
}

struct GatedRun {
    flow: ControlFlow<()>,
    fresh: crate::gemini::LiveReaderInspection,
    server: tokio::task::JoinHandle<Vec<serde_json::Value>>,
    failed: bool,
}

impl GatedRun {
    // Releases the committed or dead fresh socket and the retired one, and
    // returns what the fresh peer received.
    async fn finish(self, mut rig: Rig) -> Vec<serde_json::Value> {
        if self.failed {
            drop(rig.gemini.retire());
        } else {
            rig.gemini.shutdown().await.unwrap();
        }
        let wire = tokio::time::timeout(Duration::from_secs(2), self.server)
            .await
            .unwrap()
            .unwrap();
        drop(rig.gemini.retire());
        rig.finish().await;
        wire
    }
}

// Holds the labelled write of the real replacement, runs `during` while it is
// held, then either releases it or first drops the peer and waits for the real
// reader to finish. The fresh socket's received usage is observed before the
// release and is never drained here.
async fn gated_final_send(
    rig: &mut Rig,
    listener: tokio::net::TcpListener,
    phase: &'static str,

    // None releases the write; Some(n) drops the peer after it read the n
    // frames written before the gate, so the held write fails.
    fail_after: Option<usize>,
    during: impl FnOnce(&mpsc::UnboundedSender<Packet>),
) -> GatedRun {
    let fail = fail_after.is_some();
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    rig.loops.recovery_writes.push_back(WriteBarrier {
        phase,
        entered: std::sync::Arc::clone(&entered),
        release: std::sync::Arc::clone(&release),
    });
    let (reader, mut readers) = mpsc::unbounded_channel();
    rig.loops.recovery_reader = Some(reader);
    let (close, closed) = oneshot::channel();
    let server = tokio::spawn(closable_fresh_peer(listener, 29, closed));
    let tx = rig.tx.clone();
    let mut close = Some(close);
    let stimulus = async {
        entered.notified().await;
        let fresh = readers.recv().await.unwrap();
        wait_for_received_usage(&fresh).await;
        assert!(!fresh.is_finished());
        during(&tx);
        if fail {
            close.take().unwrap().send(fail_after.unwrap()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while !fresh.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the dropped peer ends the real reader");
            assert_eq!(fresh.pending_usage_count(), 1);
        }
        release.notify_one();
        fresh
    };
    let (flow, fresh) = tokio::join!(rig.run(), stimulus);
    GatedRun {
        flow,
        fresh,
        server,
        failed: fail,
    }
}

fn session_usage(rig: &mut Rig) {
    let mut context = rig.turn.context(
        &mut rig.output,
        &mut rig.gemini,
        &mut rig.board,
        &mut rig.media,
    );
    session::drain_live_usage(&rig.room, &mut context);
}

// Declared before running. The peer drops the fresh socket while the briefing
// is held, after the candidate's input reached the driver. The briefing then
// fails on its own Err branch, and so must the held-context or action write
// that drain attempts next. Nothing may be settled: no reference named, the
// Thinking debt and the owed prompt remain, the cold brief is demanded again,
// the edit is not recorded as shown, and the test fact stays recorded. The peer
// sees no frame. Old usage drains once at replacement (socket 1, tool); the
// dead fresh socket's usage, received before the failure, is still queued
// afterwards, is drained exactly once by the loop's own drain (socket 2), and
// carries the cause of the last send that was attempted on it: a silent context
// write leaves the candidate cause, a failed reply request leaves recovery.
//   row 0: the hold stays and a test result is held -> flush_contexts Err.
//   row 1: the hold is released and a test result arrives -> flush_actions Err.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_final_send_settles_nothing_it_did_not_deliver() {
    if isolated_checked(
        "a_failed_final_send_settles_nothing_it_did_not_deliver",
        |output| {
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 4, "{usage:?}");
            for (row, cause) in [(0, "candidate"), (1, "recovery")] {
                let (old, fresh) = (&usage[row * 2], &usage[row * 2 + 1]);
                for field in ["socket=1", "cause=tool", "prompt_tokens=11"] {
                    assert!(has_field(old, field), "{old}");
                }
                for field in [
                    "socket=2".to_owned(),
                    format!("cause={cause}"),
                    "prompt_tokens=29".to_owned(),
                ] {
                    assert!(has_field(fresh, &field), "{field}: {fresh}");
                }
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stderr
                    .lines()
                    .filter(|line| line.starts_with("recovery ingress: "))
                    .count(),
                2
            );
        },
    ) {
        return;
    }
    for release in [false, true] {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
            since: Instant::now(),
        };
        rig.turn.state.owed_reply_on_resume =
            Some(Some("[SYSTEM EVENT] Answer the original question.".into()));
        rig.turn
            .activity
            .owe_prompt(Instant::now(), Some("Answer the original question.".into()));
        rig.turn.state.earlier_steps_pending.push("design");
        rig.turn.state.code = "def solve(): return 7".into();
        rig.turn.state.language = "python".into();
        rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);

        // The second write's own gate proves the Err branch under test ran: it
        // is consumed only if drain really attempted that write.
        let next = if release {
            "held_action"
        } else {
            "held_context"
        };
        let (entered, open) = (
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        rig.loops.recovery_writes.push_back(WriteBarrier {
            phase: next,
            entered: std::sync::Arc::clone(&entered),
            release: std::sync::Arc::clone(&open),
        });
        tokio::spawn(async move {
            entered.notified().await;
            open.notify_one();
        });
        let run = gated_final_send(&mut rig, listener, "recovery_brief", Some(0), |tx| {
            if release {
                control(tx, serde_json::json!({"type":"thinking", "thinking":false}));
            }
            tx.send(test_result_packet(3, "def solve(): return 7"))
                .unwrap();
            tx.send(code_packet("def late_editor_marker(): pass"))
                .unwrap();
        })
        .await;
        assert!(run.flow.is_continue(), "release={release}");
        assert!(old.is_finished());
        assert_eq!(old.pending_usage_count(), 0);
        let state = &rig.turn.state;
        assert!(state.needs_cold_brief, "release={release}");
        assert_eq!(state.test_runs, 1);
        assert!(state.code.contains("late_editor_marker"));
        assert!(
            !state.code_shown.contains("late_editor_marker"),
            "an edit the model never received is not shown"
        );
        assert_eq!(state.earlier_steps_pending, ["design"]);
        assert!(state.earlier_steps_named.is_empty());
        assert!(
            state.owed_reply_on_resume.is_some(),
            "a failed send cannot settle the debt: release={release}"
        );
        assert_eq!(state.interviewer_silenced(), !release);
        assert!(rig.turn.activity.owes_prompt());
        if release {
            // The release asked for a newer prompt, so the older owed text is
            // not put back over it.
            assert_ne!(
                rig.turn.activity.prompt_text.as_deref(),
                Some("Answer the original question.")
            );
        }
        assert!(rig.loops.recovery_writes.is_empty());
        assert_eq!(run.fresh.pending_usage_count(), 1);
        session_usage(&mut rig);
        session_usage(&mut rig);
        assert_eq!(run.fresh.pending_usage_count(), 0);
        let thinking = rig
            .recorder
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                room_link::Sent::Data { message, .. } if message["type"] == "thinking_state" => {
                    Some(message["thinking"].as_bool().unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            thinking.last().copied(),
            // A release is published even though the write failed.
            release.then_some(false),
            "the latest Thinking notice survives the failed write: {thinking:?}"
        );
        assert!(run.finish(rig).await.is_empty());
    }
}

/// A cold briefing that fails when nothing was owed must demand a cold brief
/// again and must not invent a reply the candidate was never owed.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_cold_brief_with_nothing_owed_owes_no_reply() {
    if isolated("a_failed_cold_brief_with_nothing_owed_owes_no_reply") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
    rig.gemini.input_cause = Some("tool");
    let old = rig.gemini.inspect_reader_for_test().unwrap();
    wait_for_received_usage(&old).await;
    let run = gated_final_send(&mut rig, listener, "recovery_brief", Some(0), |_| {}).await;
    assert!(run.flow.is_continue());
    assert!(rig.turn.state.needs_cold_brief);
    assert!(!rig.turn.activity.owes_prompt());
    session_usage(&mut rig);
    session_usage(&mut rig);
    assert!(run.finish(rig).await.is_empty());
}

struct FinalSendRow {
    name: &'static str,
    phase: &'static str,

    // Thinking is held with a raw debt and a test summary already queued, so
    // the briefing is silent and drain has a held context to send.
    held: bool,

    // Release Thinking while the held context is on the wire, so the action is
    // the next write.
    release_in_context: bool,
    during: fn(&mpsc::UnboundedSender<Packet>),
}

const LATE_EDIT: &str = "def late_editor_marker(): pass";

// Frames that ask the model to answer: a completed turn, or realtime text that
// a busy socket answers after its current reply.
fn complete_frames(wire: &[serde_json::Value]) -> usize {
    wire.iter()
        .filter(|frame| {
            frame["clientContent"]["turnComplete"] == true
                || frame["realtimeInput"]["text"].is_string()
        })
        .count()
}

fn wire_has(wire: &[serde_json::Value], text: &str) -> bool {
    wire.iter().any(|frame| frame.to_string().contains(text))
}

// Declared before running, per row (wire counts are client frames after the
// fresh setup):
//   brief_edit    one reply brief. An edit alone is not an obligation, so the
//                 briefing is not resent and nothing is discarded; the edit is
//                 not marked shown and stays owed to the next reply.
//   brief_test    two reply briefs, the second carrying the test summary and
//                 consuming the action it made; no third send; discard armed.
//   brief_hold    a reply brief, then a silent one under the new hold: three
//                 frames, one reply, discard armed, hold and its notice last.
//   context_edit  a silent brief and the held context; the edit is news the
//                 hold keeps back, so it is neither sent nor marked shown.
//   action_edit   a silent brief, the held context and the released action
//                 (one reply); the action was prepared before the edit, so the
//                 edit is not marked shown, the debt it carried is settled and
//                 no output is discarded.
//   action_test   as action_edit, but a second summary inside the reaction
//                 cooldown is a recorded run and not a second action: one
//                 reply, no output discarded.
//   brief_round   a round request during the briefing is an obligation: a
//                 second briefing follows, the first reply is discarded, and
//                 the round notice is published once.
//   action_round  a round request during the action makes a second action:
//                 two replies, the debt settled by the last, no discard.
#[tokio::test(flavor = "current_thread")]
async fn input_during_a_final_send_settles_only_what_that_send_carried() {
    if isolated_checked(
        "input_during_a_final_send_settles_only_what_that_send_carried",
        |output| {
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 16, "{usage:?}");
            for pair in usage.as_chunks::<2>().0 {
                assert!(has_field(&pair[0], "socket=1") && has_field(&pair[0], "cause=tool"));
                assert!(has_field(&pair[1], "socket=2") && has_field(&pair[1], "prompt_tokens=29"));
            }
        },
    ) {
        return;
    }
    let rows = [
        FinalSendRow {
            name: "brief_edit",
            phase: "recovery_brief",
            held: false,
            release_in_context: false,
            during: |tx| tx.send(code_packet(LATE_EDIT)).unwrap(),
        },
        FinalSendRow {
            name: "brief_test",
            phase: "recovery_brief",
            held: false,
            release_in_context: false,
            during: |tx| {
                tx.send(test_result_packet(3, "def solve(): return 7"))
                    .unwrap();
            },
        },
        FinalSendRow {
            name: "brief_hold",
            phase: "recovery_brief",
            held: false,
            release_in_context: false,
            during: |tx| control(tx, serde_json::json!({"type":"thinking", "thinking":true})),
        },
        FinalSendRow {
            name: "context_edit",
            phase: "held_context",
            held: true,
            release_in_context: false,
            during: |tx| tx.send(code_packet(LATE_EDIT)).unwrap(),
        },
        FinalSendRow {
            name: "action_edit",
            phase: "held_action",
            held: true,
            release_in_context: true,
            during: |tx| tx.send(code_packet(LATE_EDIT)).unwrap(),
        },
        FinalSendRow {
            name: "brief_round",
            phase: "recovery_brief",
            held: false,
            release_in_context: false,
            during: |tx| {
                control(
                    tx,
                    serde_json::json!({"type":"round_transition", "round":"behavioral"}),
                );
            },
        },
        FinalSendRow {
            name: "action_round",
            phase: "held_action",
            held: true,
            release_in_context: true,
            during: |tx| {
                control(
                    tx,
                    serde_json::json!({"type":"round_transition", "round":"behavioral"}),
                );
            },
        },
        FinalSendRow {
            name: "action_test",
            phase: "held_action",
            held: true,
            release_in_context: true,
            during: |tx| {
                tx.send(test_result_packet(5, "def solve(): return 7"))
                    .unwrap();
            },
        },
    ];
    for row in rows {
        eprintln!("final send row: {}", row.name);
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.turn.state.code = "def solve(): return 7".into();
        rig.turn.state.language = "python".into();
        rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
        rig.turn.state.coding_minutes = 0;
        rig.turn.state.judged_code = rig.turn.state.code.clone();
        rig.turn.state.judged_language = rig.turn.state.language.clone();
        rig.turn.state.owed_reply_on_resume = Some(Some("[SYSTEM EVENT] Answer.".into()));
        rig.turn
            .activity
            .owe_prompt(Instant::now(), Some("Answer.".into()));
        if row.held {
            rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
                since: Instant::now(),
            };
            rig.tx
                .send(test_result_packet(0, "def solve(): return 7"))
                .unwrap();
        }
        if row.release_in_context {
            let (entered, open) = (
                std::sync::Arc::new(tokio::sync::Notify::new()),
                std::sync::Arc::new(tokio::sync::Notify::new()),
            );
            rig.loops.recovery_writes.push_back(WriteBarrier {
                phase: "held_context",
                entered: std::sync::Arc::clone(&entered),
                release: std::sync::Arc::clone(&open),
            });
            let tx = rig.tx.clone();
            tokio::spawn(async move {
                entered.notified().await;
                control(
                    &tx,
                    serde_json::json!({"type":"thinking", "thinking":false}),
                );
                open.notify_one();
            });
        }
        let run = gated_final_send(&mut rig, listener, row.phase, None, row.during).await;
        assert!(run.flow.is_continue(), "{}", row.name);
        assert!(rig.loops.recovery_writes.is_empty(), "{}", row.name);
        assert!(old.is_finished() && old.pending_usage_count() == 0);
        assert_eq!(run.fresh.pending_usage_count(), 1, "{}", row.name);
        session_usage(&mut rig);
        session_usage(&mut rig);
        assert_eq!(run.fresh.pending_usage_count(), 0);
        let discarding = rig.turn.activity.socket.discarding_output;
        let shown = rig.turn.state.code_shown.contains("late_editor_marker");
        let debt = rig.turn.state.owed_reply_on_resume.is_some();
        let silenced = rig.turn.state.interviewer_silenced();
        let runs = rig.turn.state.test_runs;
        let has_code = rig.turn.state.code.contains("late_editor_marker");
        let round_pending = rig.loops.pending_round_notice;
        let rounds = rig
            .recorder
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                room_link::Sent::Data { message, .. } if message["type"] == "round_state" => {
                    Some(message["status"].as_str().unwrap().to_owned())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        // What the next reply would still tell the model about the editor.
        let owed_edit = crate::agent::editor_update_body(&rig.turn.state)
            .is_some_and(|body| body.contains("late_editor_marker"));
        assert_eq!(owed_edit, has_code && !shown, "{}", row.name);
        let wire = run.finish(rig).await;
        let on_wire = wire_has(&wire, "late_editor_marker");
        assert_eq!(shown, on_wire, "{}: shown iff sent: {wire:?}", row.name);
        match row.name {
            "brief_edit" => {
                // An edit alone does not resend the briefing; its delivery
                // stays owed through `code_shown`.
                assert!(has_code && !shown && owed_edit);
                assert_eq!((wire.len(), complete_frames(&wire)), (2, 1));
                assert!(!discarding);
            }
            "brief_test" => {
                assert_eq!(runs, 1);
                assert_eq!((wire.len(), complete_frames(&wire)), (4, 2));
                assert!(wire_has(&wire[3..], "3/40 cases passed"));
                assert!(!wire_has(&wire[..2], "3/40 cases passed"));
                assert!(discarding);
            }
            "brief_hold" => {
                assert_eq!((wire.len(), complete_frames(&wire)), (3, 1));
                assert!(discarding && silenced);
            }
            "context_edit" => {
                assert!(has_code && !shown);
                assert_eq!((wire.len(), complete_frames(&wire)), (2, 0));
                assert!(silenced && !discarding);
            }
            "action_edit" => {
                assert!(has_code && !shown);
                assert_eq!((wire.len(), complete_frames(&wire)), (4, 1));
                assert!(!debt && !silenced && !discarding);
            }
            "action_test" => {
                // The second summary is inside the reaction cooldown, so it is
                // a recorded run and not a second action.
                assert_eq!(runs, 2);
                assert_eq!((wire.len(), complete_frames(&wire)), (4, 1));
                assert!(!debt && !silenced && !discarding);
            }
            "brief_round" => {
                // A round request is an obligation: the first reply is
                // discarded because the corrected briefing follows it.
                assert_eq!((wire.len(), complete_frames(&wire)), (4, 2));
                assert!(discarding && !debt);
                assert_eq!(rounds, ["skipped"]);
            }
            "action_round" => {
                // The first action is not allowed to settle the debt the second
                // one still carries; no output is discarded.
                assert_eq!((wire.len(), complete_frames(&wire)), (6, 2));
                assert!(!debt && !silenced && !discarding);
                assert_eq!(rounds, ["skipped"]);
                assert!(round_pending.is_none());
            }
            other => unreachable!("{other}"),
        }
    }
}

/// A release arrives while the first held write is on the wire, so the
/// action the held test result queued is flushed by the same drain. With
/// `fail` the peer drops the fresh socket at that write; with `buffered` the
/// ingress already holds candidate audio, so the reply is deferred behind it.
/// Nothing is owed when the replacement starts, so the only reply in play is
/// the one the action carries. No newer hold ever silences the interviewer.
async fn held_action_scenario(fail: bool, buffered: bool) -> (Rig, GatedRun) {
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
    rig.gemini.input_cause = Some("tool");
    let old = rig.gemini.inspect_reader_for_test().unwrap();
    wait_for_received_usage(&old).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.code = "def solve(): return 7".into();
    rig.turn.state.language = "python".into();
    rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
    if buffered {
        rig.media.recovery_audio.push(
            AudioFrame {
                data: std::borrow::Cow::Owned(vec![0; 160]),
                sample_rate: 16000,
                num_channels: 1,
                samples_per_channel: 160,
            },
            Instant::now(),
        );
    }
    rig.tx
        .send(test_result_packet(0, "def solve(): return 7"))
        .unwrap();
    let (entered, open) = (
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    rig.loops.recovery_writes.push_back(WriteBarrier {
        phase: "held_context",
        entered: std::sync::Arc::clone(&entered),
        release: std::sync::Arc::clone(&open),
    });
    let tx = rig.tx.clone();
    tokio::spawn(async move {
        entered.notified().await;
        control(
            &tx,
            serde_json::json!({"type":"thinking", "thinking":false}),
        );
        open.notify_one();
    });
    let run = gated_final_send(&mut rig, listener, "held_action", fail.then_some(2), |tx| {
        tx.send(code_packet(LATE_EDIT)).unwrap();
    })
    .await;
    assert!(run.flow.is_continue());
    assert!(!rig.turn.state.interviewer_silenced());
    (rig, run)
}

/// The action leaves the driver when its write is attempted, so a write that
/// fails must leave the next socket briefed from scratch, with the reply
/// still owed.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_held_action_leaves_its_reply_owed_to_the_next_socket() {
    if isolated("a_failed_held_action_leaves_its_reply_owed_to_the_next_socket") {
        return;
    }
    let (rig, _run) = held_action_scenario(true, false).await;
    assert!(rig.turn.state.needs_cold_brief);
    assert!(rig.turn.activity.owes_prompt());
}

/// A reply deferred behind buffered audio is asked for by the audio, unless
/// the audio is only silence and gives Gemini no turn to answer, so the same
/// fallback a released hold arms must be waiting.
#[tokio::test(flavor = "current_thread")]
async fn a_reply_deferred_behind_buffered_audio_arms_the_reply_fallback() {
    if isolated("a_reply_deferred_behind_buffered_audio_arms_the_reply_fallback") {
        return;
    }
    let (rig, _run) = held_action_scenario(false, true).await;
    assert!(rig.turn.activity.socket.thinking_reply_fallback.is_some());

    // Without buffered audio the reply is requested outright, so nothing is
    // left to fall back to.
    let (rig, _run) = held_action_scenario(false, false).await;
    assert!(rig.turn.activity.socket.thinking_reply_fallback.is_none());

    // The reply was marked as a prompt when it was queued and again when it
    // went out, which is two; a send that skips the second leaves it at one.
    assert_eq!(rig.turn.activity.prompt_sequence, 2);
}

// Declared before running. The fresh peer drops while the targeted held write
// is gated, after newer input reached the driver; the write then takes its Err
// branch.
//   held_context: a release arrives. flush_contexts fails (cold brief owed
//     again, the reference restored, only the briefing on the wire); drain
//     still publishes the latest Thinking notice (released) and does not
//     attempt the action that the release made, so the debt stays unsettled.
//   held_action: a newer hold arrives while the older action is on the wire.
//     The action fails (briefing and held context on the wire, no action),
//     its reference is restored, the debt is not settled, and the newer hold's
//     notice is still published after the failure.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_held_write_keeps_newer_input_and_unsettled_debt() {
    if isolated_checked(
        "a_failed_held_write_keeps_newer_input_and_unsettled_debt",
        |output| {
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 4, "{usage:?}");
            for (row, cause) in [(0, "candidate"), (1, "recovery")] {
                let (old, fresh) = (&usage[row * 2], &usage[row * 2 + 1]);
                assert!(has_field(old, "socket=1") && has_field(old, "cause=tool"));
                assert!(has_field(fresh, "socket=2") && has_field(fresh, "prompt_tokens=29"));
                assert!(has_field(fresh, &format!("cause={cause}")), "{fresh}");
            }
        },
    ) {
        return;
    }
    for phase in ["held_context", "held_action"] {
        let action = phase == "held_action";
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
            since: Instant::now(),
        };
        rig.turn.state.owed_reply_on_resume = Some(Some("[SYSTEM EVENT] Answer.".into()));
        rig.turn
            .activity
            .owe_prompt(Instant::now(), Some("Answer.".into()));
        rig.turn.state.earlier_steps_pending.push("design");
        rig.turn.state.code = "def solve(): return 7".into();
        rig.turn.state.language = "python".into();
        rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
        rig.tx
            .send(test_result_packet(0, "def solve(): return 7"))
            .unwrap();
        if action {
            let (entered, open) = (
                std::sync::Arc::new(tokio::sync::Notify::new()),
                std::sync::Arc::new(tokio::sync::Notify::new()),
            );
            rig.loops.recovery_writes.push_back(WriteBarrier {
                phase: "held_context",
                entered: std::sync::Arc::clone(&entered),
                release: std::sync::Arc::clone(&open),
            });
            let tx = rig.tx.clone();
            tokio::spawn(async move {
                entered.notified().await;
                control(
                    &tx,
                    serde_json::json!({"type":"thinking", "thinking":false}),
                );
                open.notify_one();
            });
        }
        let prior = if action { 2 } else { 1 };
        let run = gated_final_send(&mut rig, listener, phase, Some(prior), |tx| {
            control(
                tx,
                serde_json::json!({"type":"thinking", "thinking":action}),
            );
            tx.send(code_packet(LATE_EDIT)).unwrap();
        })
        .await;
        assert!(run.flow.is_continue(), "{phase}");
        assert!(old.is_finished() && old.pending_usage_count() == 0);
        let state = &rig.turn.state;
        assert_eq!(state.needs_cold_brief, !action, "{phase}");

        // The briefing that succeeded named the reference; a failed briefing
        // would have left it pending.
        let (pending, named): (&[&str], &[&str]) = if action {
            (&[], &["design"])
        } else {
            (&["design"], &[])
        };
        assert_eq!(state.earlier_steps_pending, pending, "{phase}");
        assert_eq!(state.earlier_steps_named, named, "{phase}");
        assert!(
            state.owed_reply_on_resume.is_some(),
            "an undelivered action cannot settle the debt: {phase}"
        );
        assert_eq!(state.interviewer_silenced(), action, "{phase}");
        assert!(!state.code_shown.contains("late_editor_marker"));

        // Under the newer hold the reply is carried by the resume debt, not by
        // an owed prompt; released, the prompt itself stays owed.
        assert_eq!(rig.turn.activity.owes_prompt(), !action, "{phase}");
        assert_eq!(run.fresh.pending_usage_count(), 1);
        session_usage(&mut rig);
        session_usage(&mut rig);
        assert_eq!(run.fresh.pending_usage_count(), 0);
        let thinking = rig
            .recorder
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                room_link::Sent::Data { message, .. } if message["type"] == "thinking_state" => {
                    Some(message["thinking"].as_bool().unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            thinking.last(),
            Some(&action),
            "the latest Thinking notice survives the failed write: {phase} {thinking:?}"
        );
        let wire = run.finish(rig).await;
        assert_eq!(wire.len(), if action { 2 } else { 1 }, "{phase}: {wire:?}");
        assert_eq!(
            complete_frames(&wire),
            0,
            "{phase}: nothing asked for a reply"
        );
    }
}

// Declared before running. Under a Thinking hold, forty distinct test results
// (the reaction cooldown is reset between them, as elapsed time would) each
// make a held context. The driver retains the newest 32 and counts 8 omitted;
// one drain writes one frame holding the retained summaries in arrival order
// followed by one sentence naming 8 omitted summaries, and then the pending
// count is spent while the running total stays 8. A second batch of 33 omits
// exactly one more: its frame names 1, never 9, and the total is 9.
#[tokio::test]
async fn omitted_held_contexts_are_counted_once_and_reach_the_wire_once() {
    if isolated("omitted_held_contexts_are_counted_once_and_reach_the_wire_once") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    let old = rig.gemini.retire();
    let (mut fresh, server) = fake_recording_socket(None, 2).await;
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
    let interview = InterviewContext {
        config: &rig.config,
        keys: &rig.keys,
        boot: &boot,
        live_endpoint: Some(&rig.endpoint),
        started_at: Instant::now(),
        candidate_identity: "candidate",
        events: &rig.events,
        slot: None,
    };
    {
        let mut context = rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        );
        let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
        let feed = |driver: &mut Driver<'_, '_, '_>, count: u32| {
            for n in 0..count {
                driver.context.activity.last_test_reaction =
                    Instant::now() - Duration::from_secs(100);
                let packet = test_result_packet(n, &format!("def solve(): return {n}"));
                driver.packet(packet.topic.as_deref(), Some("candidate"), &packet.payload);
            }
        };
        feed(&mut driver, 40);
        assert_eq!(driver.held_contexts.len(), 32);
        assert_eq!((driver.held_omitted, driver.held_omitted_total), (8, 8));
        assert!(driver.pending());
        assert!(!driver.drain(&mut fresh).await.unwrap());
        assert!(driver.held_contexts.is_empty());
        assert_eq!(driver.held_bytes, 0);
        assert_eq!((driver.held_omitted, driver.held_omitted_total), (0, 8));
        assert!(!driver.pending());
        feed(&mut driver, 33);
        assert_eq!((driver.held_omitted, driver.held_omitted_total), (1, 9));
        assert!(!driver.drain(&mut fresh).await.unwrap());
        assert_eq!((driver.held_omitted, driver.held_omitted_total), (0, 9));
        assert!(!driver.pending());
        assert!(driver.context.state.interviewer_silenced());
    }
    let messages = server.await.unwrap();
    assert_eq!(messages.len(), 2);
    let text = |index: usize| {
        messages[index]["clientContent"]["turns"][0]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    for (index, omitted) in [(0, 8), (1, 1)] {
        assert_eq!(messages[index]["clientContent"]["turnComplete"], false);
        let body = text(index);
        let sentence = format!(
            "[SYSTEM EVENT] {omitted} earlier context summaries exceeded recovery retention"
        );
        assert_eq!(
            body.matches("earlier context summaries exceeded").count(),
            1
        );
        assert!(body.contains(&sentence), "{body}");
        // The newest retained summaries, oldest first, then the sentence.
        let runs = body
            .split("(run #")
            .skip(1)
            .map(|rest| rest.split(',').next().unwrap().parse::<u32>().unwrap())
            .collect::<Vec<_>>();
        let expected = if index == 0 {
            (9..=40).collect::<Vec<u32>>()
        } else {
            (42..=73).collect::<Vec<u32>>()
        };
        assert_eq!(runs, expected, "{index}");
        assert!(
            body.rfind("Latest test run").unwrap()
                < body.find("earlier context summaries").unwrap()
        );
    }
    fresh.shutdown().await.unwrap();
    rig.gemini = old;
    rig.finish().await;
}
