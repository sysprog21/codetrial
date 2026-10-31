// Part of recovery.rs: included in place, so every item here shares that file's
// module.

// `physical` replaces the before-write gate with a peer whose receive window is
// tiny and which stops reading, so the targeted write really pends on the
// socket. Only End is exercised there.
//
// `after` instead completes the write and queues the terminal in that same
// poll, so the next boundary (not a gate) must observe it. Deadline is not in
// that matrix: it is a clock test, not a queued input.
async fn write_site_terminal_matrix(site: WriteSite, physical: bool, after: bool) {
    let causes: &[OpenStop] = if physical {
        &[OpenStop::End]
    } else if after {
        &[
            OpenStop::End,
            OpenStop::RoomDisconnected,
            OpenStop::EventsEof,
            OpenStop::Absent,
        ]
    } else {
        &PREFIX_STOPS
    };
    let mut rows = 0;
    for &cause in causes {
        let resumed = site == WriteSite::SilentEditor;
        let mut rig = Rig::with_old_checkpoint("ws://127.0.0.1:9/".into(), Some(11), resumed).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        if matches!(site, WriteSite::ListeningBrief | WriteSite::SilentEditor) {
            // Speech handoff without a prefix finalization. A resumed socket
            // (the editor site) also needs a checkpoint newer than the handoff.
            rig.speech = Some(session::SpeechHandoff {
                checkpoint_after: Instant::now()
                    .checked_sub(Duration::from_secs(30))
                    .unwrap_or_else(Instant::now),
                transcript_fragments: vec![PREFIX_TEXT.into()],
                discarded: 0,
            });
        }
        if resumed {
            rig.turn.state.code = "def solve(): return 7".into();
            rig.turn.state.language = "python".into();
        }
        if matches!(site, WriteSite::HeldContext | WriteSite::HeldAction) {
            rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
                since: Instant::now(),
            };
            rig.turn.state.code = "def solve(): return 7".into();
            rig.turn.state.language = "python".into();
            rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
        }
        if matches!(site, WriteSite::BoardImage) {
            rig.turn.state.interview_mode = crate::agent::InterviewMode::Whiteboard;
            rig.board.seed_recovery_image(if physical {
                vec![0x5a; 24 * 1024 * 1024]
            } else {
                vec![0x5a; 64]
            });
        }
        let listener = tokio::net::TcpSocket::new_v4().unwrap();
        if physical {
            listener.set_recv_buffer_size(1024).unwrap();
        }
        listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = listener.listen(1).unwrap();
        rig.endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let instructions =
            crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45)
                .instructions;
        let expected_before = site.prior_frames() + if after { site.frames() } else { 0 };
        let (body, body_received) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let server = tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::{Error as WsError, error::ProtocolError};
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let setup = socket.next().await.unwrap().unwrap();
            let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
            assert_eq!(
                setup["setup"]["systemInstruction"]["parts"][0]["text"],
                instructions
            );
            assert_eq!(
                setup["setup"]["sessionResumption"]["handle"],
                if resumed {
                    serde_json::json!("matrix-checkpoint")
                } else {
                    serde_json::Value::Null
                }
            );
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    r#"{"usageMetadata":{"promptTokenCount":29}}"#.into(),
                ))
                .await
                .unwrap();
            let mut messages = Vec::new();
            for _ in 0..expected_before {
                let frame = socket.next().await.unwrap().unwrap();
                messages.push(
                    serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap(),
                );
            }
            body.send(()).unwrap();
            finished.await.unwrap();
            if physical {
                // Never read the stalled write; observe only its release.
                let mut socket = socket.into_inner();
                use tokio::io::AsyncReadExt;
                let mut bytes = [0; 65536];
                let mut received = 0;
                loop {
                    match socket.read(&mut bytes).await {
                        Ok(0) => break,
                        Ok(count) => received += count,
                        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                        other => panic!("cancelled write must release the transport: {other:?}"),
                    }
                }
                return (messages, Some(received));
            }
            while let Some(frame) = socket.next().await {
                match frame {
                    Ok(frame) if frame.is_close() => break,
                    Ok(frame) => {
                        messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap());
                    }
                    Err(WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake)) => break,
                    Err(WsError::Io(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                        ) =>
                    {
                        break;
                    }
                    Err(error) => panic!("unexpected fresh peer error: {error}"),
                }
            }
            (messages, None)
        });
        let (setup_notice, release_setup) =
            rig.recorder.hold_data("interviewer_state", Some(false));
        let gate = (!physical && !after).then(|| {
            let entered = std::sync::Arc::new(tokio::sync::Notify::new());
            rig.loops.recovery_writes.push_back(WriteBarrier {
                phase: site.label(),
                entered: std::sync::Arc::clone(&entered),
                release: std::sync::Arc::new(tokio::sync::Notify::new()),
            });
            entered
        });

        // Releasing Thinking while the held context write is gated produces the
        // action after the briefing, so it is the next fresh write.
        let context_gate = (site == WriteSite::HeldAction).then(|| {
            let entered = std::sync::Arc::new(tokio::sync::Notify::new());
            let release = std::sync::Arc::new(tokio::sync::Notify::new());
            rig.loops.recovery_writes.push_front(WriteBarrier {
                phase: "held_context",
                entered: std::sync::Arc::clone(&entered),
                release: std::sync::Arc::clone(&release),
            });
            (entered, release)
        });
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (probe, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(probe);
        let (phase_probe, mut phases) = mpsc::unbounded_channel();
        rig.loops.recovery_phase = Some(phase_probe);
        rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(8);
        let deadline = rig.loops.deadline_at;
        let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        if matches!(cause, OpenStop::Absent) {
            rig.loops.presence.left(*presence_now.lock().unwrap());
            rig.loops.presence.test_now = Some(std::sync::Arc::clone(&presence_now));
        }
        let (unused, _) = mpsc::unbounded_channel();
        let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
        let tx = rig.tx.clone();
        if after {
            let tx = tx.clone();
            // Only the cause under test may touch the room-event channel.
            let sender = match cause {
                OpenStop::EventsEof => room.take(),
                OpenStop::RoomDisconnected => room.clone(),
                _ => None,
            };
            let presence_now = std::sync::Arc::clone(&presence_now);
            rig.loops.recovery_after.push_back(AfterWrite {
                phase: site.label(),
                action: Box::new(move || match cause {
                    OpenStop::End => control(&tx, serde_json::json!({"type":"end_interview"})),
                    OpenStop::RoomDisconnected => sender
                        .unwrap()
                        .send(RoomEvent::Disconnected {
                            reason: DisconnectReason::ClientInitiated,
                        })
                        .unwrap(),
                    OpenStop::EventsEof => drop(sender),
                    OpenStop::Absent => {
                        *presence_now.lock().unwrap() += CANDIDATE_ABSENCE_LIMIT;
                        tx.send(Packet {
                            topic: Some(TOPIC_CONTROL.into()),
                            sender: Some("bystander".into()),
                            payload: b"{}".to_vec(),
                        })
                        .unwrap();
                    }
                    OpenStop::Deadline => unreachable!("not a queued input"),
                }),
            });
        }
        let fresh_state = std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
        let running = async {
            let flow = rig.run().await;
            assert!(old.is_finished());
            assert_eq!(old.pending_usage_count(), 0);
            let inspected = fresh_state.borrow();
            let fresh = inspected
                .as_ref()
                .expect("fresh receipt barrier preceded terminal");
            assert!(
                fresh.is_finished(),
                "write cancellation joins fresh reader in return poll: {site:?} {cause:?}"
            );
            assert_eq!(fresh.pending_usage_count(), 0);
            flow
        };
        let stimulus = async {
            setup_notice.notified().await;
            let fresh = readers.recv().await.unwrap();
            wait_for_received_usage(&fresh).await;
            assert!(!fresh.is_finished());
            assert!(old.is_finished());
            fresh_state.replace(Some(fresh));
            if matches!(site, WriteSite::HeldContext | WriteSite::HeldAction) {
                tx.send(Packet {
                    topic: Some(crate::runtime::TOPIC_TEST_RESULTS.into()),
                    sender: Some("candidate".into()),
                    payload: serde_json::to_vec(&serde_json::json!({
                        "passed": 0, "total": 1, "code":"def solve(): return 7",
                        "language":"python"
                    }))
                    .unwrap(),
                })
                .unwrap();
            }
            release_setup.notify_one();
            if let Some((entered, release)) = &context_gate {
                entered.notified().await;
                control(
                    &tx,
                    serde_json::json!({"type":"thinking", "thinking":false}),
                );
                release.notify_one();
            }
            if after {
                // The completed write queues the terminal itself.
            } else if let Some(entered) = &gate {
                entered.notified().await;
            } else {
                while phases.recv().await.expect("phase probe closed") != site.label() {}
                // A physically blocked write cannot finish on its own.
                tokio::time::sleep(Duration::from_millis(300)).await;
                assert!(phases.try_recv().is_err());
                assert!(observed.try_recv().is_err());
                assert!(!fresh_state.borrow().as_ref().unwrap().is_finished());
            }
            body_received.await.unwrap();
            assert!(tokio::time::Instant::now() < deadline);
            if !after {
                assert!(!fresh_state.borrow().as_ref().unwrap().is_finished());
                stop_pending_open(
                    cause,
                    deadline,
                    &tx,
                    &mut room,
                    &mut observed,
                    &presence_now,
                )
                .await;
            }
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        assert_eq!(rig.loops.deadline_at, deadline);
        assert_eq!(rig.loops.restarts, 1);
        assert_eq!(rig.turn.activity.live_socket, 2);
        assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 1);
        assert_eq!(rig.turn.activity.live_usage.samples, 2);
        assert_eq!(rig.turn.activity.live_usage.prompt, 40);
        if after {
            let terminal = observed.try_recv().expect("the queued terminal was taken");
            let shown = match &terminal {
                Terminal::End { .. } => "End",
                Terminal::Deadline => "Deadline",
                Terminal::Absent => "Absent",
                Terminal::RoomClosed => "RoomClosed",
            };
            assert!(
                matches!(
                    (cause, terminal),
                    (OpenStop::End, Terminal::End { .. })
                        | (OpenStop::Absent, Terminal::Absent)
                        | (
                            OpenStop::RoomDisconnected | OpenStop::EventsEof,
                            Terminal::RoomClosed
                        )
                ),
                "{site:?} {cause:?} {shown}"
            );
        }
        if matches!(site, WriteSite::HeldContext | WriteSite::HeldAction) {
            // The held text may be lost with the discarded socket, but the fact
            // it reported is state the ending owner reads.
            assert_eq!(rig.turn.state.test_runs, 1, "{site:?} {cause:?}");
        }
        assert!(observed.try_recv().is_err());
        assert!(readers.try_recv().is_err());
        assert!(
            rig.gemini
                .send_text("no replacement installed")
                .await
                .is_err()
        );
        assert!(rig.loops.recovery_writes.is_empty());
        rig.finish().await;
        finish.send(()).unwrap();
        let (messages, received) = server.await.unwrap();
        // The unread image was still in flight: the peer never held all of it.
        assert_eq!(received.is_some(), physical);
        assert!(received.is_none_or(|bytes| bytes < 24 * 1024 * 1024));
        assert_eq!(
            messages.len(),
            expected_before,
            "no write after the targeted one may reach the wire: {site:?} {cause:?}"
        );
        rows += 1;
    }
    assert_eq!(rows, causes.len());
}

macro_rules! write_site_test {
    ($name:ident, $site:expr, $physical:expr) => {
        write_site_test!($name, $site, $physical, false);
    };
    ($name:ident, $site:expr, $physical:expr, $after:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            let rows = if $physical {
                1
            } else if $after {
                4
            } else {
                5
            };
            let drain = if $after {
                $site.after_cause()
            } else {
                $site.drain_cause()
            };
            if isolated_checked(stringify!($name), |out| write_site_usage(out, drain, rows)) {
                return;
            }
            write_site_terminal_matrix($site, $physical, $after).await;
        }
    };
}

write_site_test!(brief_write_cancels_before_the_wire, WriteSite::Brief, false);
write_site_test!(
    held_context_write_cancels_before_the_wire,
    WriteSite::HeldContext,
    false
);
write_site_test!(
    held_action_write_cancels_before_the_wire,
    WriteSite::HeldAction,
    false
);
write_site_test!(
    board_image_write_cancels_before_the_wire,
    WriteSite::BoardImage,
    false
);
write_site_test!(
    board_image_write_cancels_under_backpressure,
    WriteSite::BoardImage,
    true
);
write_site_test!(
    listening_brief_write_cancels_before_the_wire,
    WriteSite::ListeningBrief,
    false
);
write_site_test!(
    silent_editor_write_cancels_before_the_wire,
    WriteSite::SilentEditor,
    false
);
write_site_test!(
    brief_completion_yields_to_a_queued_terminal,
    WriteSite::Brief,
    false,
    true
);
write_site_test!(
    held_context_completion_yields_to_a_queued_terminal,
    WriteSite::HeldContext,
    false,
    true
);
write_site_test!(
    held_action_completion_yields_to_a_queued_terminal,
    WriteSite::HeldAction,
    false,
    true
);
write_site_test!(
    board_image_completion_yields_to_a_queued_terminal,
    WriteSite::BoardImage,
    false,
    true
);
write_site_test!(
    listening_brief_completion_yields_to_a_queued_terminal,
    WriteSite::ListeningBrief,
    false,
    true
);
write_site_test!(
    silent_editor_completion_yields_to_a_queued_terminal,
    WriteSite::SilentEditor,
    false,
    true
);

#[tokio::test]
async fn a_held_action_settles_only_the_revision_it_carried() {
    if isolated("a_held_action_settles_only_the_revision_it_carried") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.code = "def solve(): return 7".into();
    rig.turn.state.language = "python".into();
    rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
    let mut gates = Vec::new();
    for phase in ["held_context", "held_action"] {
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        rig.loops.recovery_writes.push_back(WriteBarrier {
            phase,
            entered: std::sync::Arc::clone(&entered),
            release: std::sync::Arc::clone(&release),
        });
        gates.push((entered, release));
    }
    let tx = rig.tx.clone();
    tx.send(Packet {
        topic: Some(crate::runtime::TOPIC_TEST_RESULTS.into()),
        sender: Some("candidate".into()),
        payload: serde_json::to_vec(&serde_json::json!({
            "passed": 0, "total": 1, "code":"def solve(): return 7", "language":"python"
        }))
        .unwrap(),
    })
    .unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut messages = Vec::new();
        while let Some(Ok(frame)) = socket.next().await {
            if frame.is_close() {
                break;
            }
            messages
                .push(serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap());
        }
        messages
    });
    let stimulus = async {
        gates[0].0.notified().await;
        control(
            &tx,
            serde_json::json!({"type":"thinking", "thinking":false}),
        );
        gates[0].1.notify_one();
        gates[1].0.notified().await;
        // A newer hold arrives while the older action is on the wire.
        control(&tx, serde_json::json!({"type":"thinking", "thinking":true}));
        gates[1].1.notify_one();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_continue());
    assert!(rig.turn.state.interviewer_silenced());
    assert!(
        rig.turn.state.owed_reply_on_resume.is_some(),
        "an older action's delivery cannot settle the newer hold's debt"
    );
    assert!(rig.loops.recovery_writes.is_empty());
    rig.finish().await;
    let messages = server.await.unwrap();
    assert!(
        messages.len() >= 3
            && messages
                .iter()
                .any(|message| message.to_string().contains("ready after thinking time")),
        "briefing, context and the older action each reach the wire"
    );
}

enum FreshStep {
    Ready,
    DropAfterSetup,
    CloseAfterSetup(u16, &'static str),
    Refuse(axum::http::StatusCode),
}

struct Accepted {
    key: String,
    handle: Option<serde_json::Value>,
}

// Plays one scripted step per accepted connection and records what the client
// asked for. Connections beyond the script are never accepted. The handshake
// callback's error type is a full HTTP response.
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is a full HTTP response"
)]
async fn scripted_fresh_gemini(
    listener: tokio::net::TcpListener,
    script: Vec<FreshStep>,
) -> (Vec<Accepted>, tokio::net::TcpListener) {
    use tokio_tungstenite::tungstenite::handshake::server;
    let mut seen = Vec::new();
    for step in script {
        let (socket, _) = listener.accept().await.unwrap();
        let mut key = String::new();
        let refuse = match &step {
            FreshStep::Refuse(status) => Some(*status),
            _ => None,
        };
        let accepted = tokio_tungstenite::accept_hdr_async(
            socket,
            |request: &server::Request, response: server::Response| {
                key = request
                    .uri()
                    .query()
                    .unwrap_or_default()
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("key=").map(str::to_owned))
                    .unwrap_or_default();
                if let Some(status) = refuse {
                    let mut refusal = server::ErrorResponse::new(None);
                    *refusal.status_mut() = status;
                    Err(refusal)
                } else {
                    Ok(response)
                }
            },
        )
        .await;
        let Ok(mut socket) = accepted else {
            seen.push(Accepted { key, handle: None });
            continue;
        };
        let setup = socket.next().await.unwrap().unwrap();
        let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
        seen.push(Accepted {
            key,
            handle: Some(setup["setup"]["sessionResumption"]["handle"].clone())
                .filter(|handle| !handle.is_null()),
        });
        match step {
            FreshStep::DropAfterSetup => drop(socket),
            FreshStep::CloseAfterSetup(code, reason) => {
                socket
                    .send(Message::Close(Some(
                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code: code.into(),
                            reason: reason.into(),
                        },
                    )))
                    .await
                    .unwrap();
            }
            FreshStep::Ready => {
                socket
                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                    .await
                    .unwrap();
                while let Some(Ok(frame)) = socket.next().await {
                    if frame.is_close() {
                        break;
                    }
                }
            }
            FreshStep::Refuse(_) => unreachable!("refused during the handshake"),
        }
    }
    (seen, listener)
}

#[derive(Debug, PartialEq)]
enum Replaced {
    Ready,
    Billing,
    Unreachable,
}

struct ReplacementCase {
    name: &'static str,
    update: Option<serde_json::Value>,
    keys: &'static str,
    stale_checkpoint: bool,
    young_resumed: bool,
    restarts: usize,
    script: Vec<FreshStep>,
    // (key, resume handle offered) per accepted connection, in order.
    accepts: Vec<(&'static str, Option<&'static str>)>,
    fallback: Option<&'static str>,
    outcome: Replaced,
    restarts_after: usize,
    // Real time the recovery must spend waiting on a cooling key.
    min_wait: Duration,
}

fn replacement_cases() -> Vec<ReplacementCase> {
    let checkpoint = || Some(serde_json::json!({"resumable": true, "newHandle": "table-handle"}));
    let base = |name| ReplacementCase {
        name,
        update: checkpoint(),
        keys: "sequence-test-key",
        stale_checkpoint: false,
        young_resumed: false,
        restarts: 0,
        script: vec![FreshStep::Ready],
        accepts: vec![("sequence-test-key", Some("table-handle"))],
        fallback: Some("none"),
        outcome: Replaced::Ready,
        restarts_after: 1,
        min_wait: Duration::ZERO,
    };
    vec![
        base("resume_once_on_the_original_key"),
        ReplacementCase {
            update: None,
            accepts: vec![("sequence-test-key", None)],
            fallback: Some("no_handle"),
            ..base("absent_handle_opens_cold")
        },
        ReplacementCase {
            update: Some(serde_json::json!({"resumable": false, "newHandle": "unusable"})),
            accepts: vec![("sequence-test-key", None)],
            fallback: Some("no_handle"),
            ..base("handle_without_resumable_opens_cold")
        },
        ReplacementCase {
            script: vec![FreshStep::DropAfterSetup, FreshStep::Ready],
            accepts: vec![
                ("sequence-test-key", Some("table-handle")),
                ("sequence-test-key", None),
            ],
            fallback: Some("resume_refused"),
            restarts_after: 1,
            ..base("a_dropped_resume_falls_back_cold_once")
        },
        ReplacementCase {
            script: vec![
                FreshStep::CloseAfterSetup(1008, "session handle has expired"),
                FreshStep::Ready,
            ],
            accepts: vec![
                ("sequence-test-key", Some("table-handle")),
                ("sequence-test-key", None),
            ],
            fallback: Some("resume_refused"),
            ..base("a_close_before_setup_complete_is_a_refused_resume_not_retried")
        },
        ReplacementCase {
            stale_checkpoint: true,
            accepts: vec![("sequence-test-key", None)],
            fallback: Some("checkpoint_before_handoff"),
            ..base("a_checkpoint_older_than_the_handoff_is_not_offered")
        },
        ReplacementCase {
            young_resumed: true,
            accepts: vec![("sequence-test-key", None)],
            fallback: Some("resume_suppressed"),
            ..base("a_young_resumed_socket_suppresses_the_next_resume")
        },
        ReplacementCase {
            keys: "other-key,sequence-test-key",
            accepts: vec![("other-key", None)],
            fallback: Some("no_handle"),
            ..base("a_handle_never_crosses_to_another_key")
        },
        ReplacementCase {
            update: None,
            script: vec![FreshStep::Refuse(axum::http::StatusCode::PAYMENT_REQUIRED)],
            accepts: vec![("sequence-test-key", None)],
            fallback: None,
            outcome: Replaced::Billing,
            ..base("a_sole_key_billing_failure_is_final")
        },
        // A sole key has nothing to rotate to: the retry waits, then asks.
        ReplacementCase {
            update: None,
            script: vec![
                FreshStep::Refuse(axum::http::StatusCode::TOO_MANY_REQUESTS),
                FreshStep::Ready,
            ],
            accepts: vec![("sequence-test-key", None), ("sequence-test-key", None)],
            fallback: Some("no_handle"),
            restarts_after: 2,
            min_wait: crate::livekit::COLD_OPEN_BACKOFF,
            ..base("a_sole_key_rate_limit_waits_before_the_same_key")
        },
        ReplacementCase {
            update: None,
            // Distinct names: cooldowns are process-global and outlive a row.
            keys: "cooling-key,backup-key",
            script: vec![
                FreshStep::Refuse(axum::http::StatusCode::TOO_MANY_REQUESTS),
                FreshStep::Ready,
            ],
            accepts: vec![("cooling-key", None), ("backup-key", None)],
            fallback: Some("no_handle"),
            restarts_after: 2,
            min_wait: crate::livekit::COLD_OPEN_BACKOFF,
            // The retry still waits the backoff before moving to the backup.
            ..base("a_rate_limited_key_gives_way_to_the_backup_after_the_backoff")
        },
        // A refused resume falls back cold: one more handshake.
        ReplacementCase {
            script: vec![
                FreshStep::Refuse(axum::http::StatusCode::PAYMENT_REQUIRED),
                FreshStep::Refuse(axum::http::StatusCode::PAYMENT_REQUIRED),
            ],
            accepts: vec![("sequence-test-key", None), ("sequence-test-key", None)],
            fallback: None,
            outcome: Replaced::Billing,
            ..base("billing_on_a_resume_attempt_ends_after_one_cold_try")
        },
        ReplacementCase {
            restarts: crate::livekit::GEMINI_RESTART_LIMIT,
            script: Vec::new(),
            accepts: Vec::new(),
            fallback: None,
            outcome: Replaced::Unreachable,
            restarts_after: crate::livekit::GEMINI_RESTART_LIMIT,
            ..base("spent_charges_refuse_before_any_connection")
        },
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn replacement_classification_follows_the_declared_table() {
    if isolated_checked(
        "replacement_classification_follows_the_declared_table",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let lines = stderr
                .lines()
                .filter(|line| line.starts_with("replacement: "))
                .collect::<Vec<_>>();
            let field = |line: &&str, name: &str| {
                line.split_whitespace()
                    .find_map(|field| field.strip_prefix(name))
                    .map(str::to_owned)
            };
            let observed = lines
                .iter()
                .filter_map(|line| field(line, "fallback="))
                .collect::<Vec<_>>();
            // A replacement that offered no handle has no checkpoint to age.
            let offered = replacement_cases()
                .iter()
                .filter(|case| case.fallback.is_some())
                .map(|case| {
                    case.accepts
                        .first()
                        .is_some_and(|accepted| accepted.1.is_some())
                })
                .collect::<Vec<_>>();
            for (line, offered) in lines.iter().zip(offered) {
                assert_eq!(
                    field(line, "checkpoint_age=").as_deref() == Some("none"),
                    !offered,
                    "{line}"
                );
            }
            let expected = replacement_cases()
                .iter()
                .filter_map(|case| case.fallback)
                .collect::<Vec<_>>();
            assert_eq!(observed, expected);
        },
    ) {
        return;
    }
    let cases = replacement_cases();
    assert_eq!(cases.len(), 13);
    for case in cases {
        eprintln!("replacement case: {}", case.name);
        let mut rig =
            Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), case.update.clone())
                .await;
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEYS", case.keys),
        ])
        .unwrap();
        rig.keys = Arc::new(GeminiKeys::from_config(&config));
        rig.config = config;
        let (fresh, endpoint) = listener().await;
        rig.endpoint = endpoint;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        if case.stale_checkpoint {
            tokio::time::sleep(Duration::from_millis(20)).await;
            rig.speech = Some(session::SpeechHandoff {
                checkpoint_after: Instant::now(),
                transcript_fragments: vec![PREFIX_TEXT.into()],
                discarded: 0,
            });
        }
        if case.young_resumed {
            rig.loops.resume_recovery.connected(true);
        }
        rig.loops.restarts = case.restarts;
        rig.turn.state.earlier_steps_pending.push("design");
        if case.stale_checkpoint {
            // The hold dropped what the model said, and a handoff note is the
            // briefing that owes the candidate that disowning.
            rig.turn.state.thinking_unheard_reply = true;
        }
        if case.outcome != Replaced::Ready {
            rig.turn.state.ended = true;
        }
        let server = tokio::spawn(scripted_fresh_gemini(fresh, case.script));
        let started = Instant::now();
        let flow = rig.run().await;
        assert!(started.elapsed() >= case.min_wait, "{}", case.name);
        match case.outcome {
            Replaced::Ready => {
                assert!(flow.is_continue(), "{}", case.name);
                assert_eq!(rig.turn.activity.live_socket, 2, "{}", case.name);
            }
            Replaced::Billing => {
                assert!(flow.is_break(), "{}", case.name);
                assert_eq!(rig.turn.activity.live_exit, LiveOutcome::Billing);
            }
            Replaced::Unreachable => {
                assert!(flow.is_break(), "{}", case.name);
                assert_eq!(rig.turn.activity.live_exit, LiveOutcome::GeminiUnreachable);
            }
        }
        assert_eq!(rig.loops.restarts, case.restarts_after, "{}", case.name);
        if case.outcome == Replaced::Ready {
            // A resumed socket with nothing owed asks for no reply, and a
            // speech handoff never does; every other cold briefing does.
            assert_eq!(
                rig.turn.state.recovery_reply_pending,
                case.fallback != Some("none") && !case.stale_checkpoint,
                "{}",
                case.name
            );

            // A handoff with nothing owed leaves nothing owed, and the note
            // that carried the unheard debt paid it.
            assert!(
                !case.stale_checkpoint || !rig.turn.activity.owes_prompt(),
                "{}",
                case.name
            );
            assert!(!rig.turn.state.thinking_unheard_reply, "{}", case.name);
            // The reference the briefing carried is named, and none is left.
            assert_eq!(
                rig.turn.state.earlier_steps_named,
                ["design"],
                "{}",
                case.name
            );
            assert!(
                rig.turn.state.earlier_steps_pending.is_empty(),
                "{}",
                case.name
            );
            // A replacement times its first reply only when it asked for one.
            assert_eq!(
                rig.turn.activity.take_rotation_watch().is_some(),
                case.fallback != Some("none") && !case.stale_checkpoint,
                "{}",
                case.name
            );
        }
        rig.finish().await;
        let (seen, listener) = server.await.unwrap();
        let seen = seen
            .iter()
            .map(|accepted| {
                (
                    accepted.key.as_str(),
                    accepted.handle.as_ref().and_then(|handle| handle.as_str()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(seen, case.accepts, "{}", case.name);
        // No attempt beyond the script may be waiting to connect.
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "{}",
            case.name
        );
    }
}

// Characterization of the age-based restart reset (no policy change).
//
// `take_restart_attempt` clears the consecutive-charge count for any replaced
// socket whose budget age reaches `HEALTHY_GEMINI_SOCKET`, with or without
// output, as long as the socket owed no reply (`replaced_socket_age`). So a
// provider that closes an idle socket 61 seconds after opening it, forever,
// never exhausts the budget, while the same pattern at 59 seconds or with an
// unfinished reply ends the interview on the ninth close. This block pins that
// behaviour in logical time (no sleeping) and asserts no progress requirement,
// because none exists. Whether it should is a separate policy decision.
//
// The step below composes the same calls the replacement site makes:
// `replaced_socket_age`, `take_restart_attempt`, and `ResumeRecovery` fed the
// raw socket age. The replacement
// site's `trigger == Watchdog || owed` flag is held false here; an owed reply
// reaches the same zeroed age through `reply_unfinished`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SocketEnd {
    /// The briefing's answer, then a real reply that completes.
    RealOutput,
    /// Nothing happens on the socket.
    Idle,
    /// Only the answer to the recovery briefing completes. The first socket
    /// has no briefing, so its one completion is a real reply; the charge is
    /// already zero there, so nothing observable differs.
    BriefingOnly,
    /// A candidate turn is awaiting a reply when the socket is replaced.
    OwedReply,
}

#[derive(Debug, PartialEq, Eq)]
struct BudgetRun {
    opens: usize,
    refused_at_logical_s: Option<u64>,
    max_charge: usize,
    final_charge: usize,
    age_resets: usize,
    output_resets: usize,
    resumes_allowed: usize,
}

fn run_restart_budget(sockets: usize, life_s: u64, end: SocketEnd) -> BudgetRun {
    let life = Duration::from_secs(life_s);
    let mut run = BudgetRun {
        opens: 0,
        refused_at_logical_s: None,
        max_charge: 0,
        final_charge: 0,
        age_resets: 0,
        output_resets: 0,
        resumes_allowed: 0,
    };
    let mut restarts = 0;
    let mut recovery = ResumeRecovery::default();
    let mut state = RuntimeState::default();
    for socket in 1..=sockets {
        // Every replacement is followed by a briefing; the first socket has
        // none.
        state.recovery_reply_pending = socket > 1;
        let completions = match end {
            SocketEnd::RealOutput => 2,
            SocketEnd::BriefingOnly => 1,
            SocketEnd::Idle | SocketEnd::OwedReply => 0,
        };
        for _ in 0..completions {
            let before = restarts;
            let mut generating = RuntimeActivity::new(Instant::now());
            generating.note_output(Instant::now());
            recovery.note_event(
                &mut restarts,
                &mut state,
                &generating,
                &GeminiEvent::TurnComplete,
            );
            if before > 0 && restarts == 0 {
                run.output_resets += 1;
            }
        }
        let mut activity = RuntimeActivity::new(Instant::now());
        if end == SocketEnd::OwedReply {
            activity.note_candidate_finished(Instant::now(), false);
            assert!(activity.reply_unfinished());
        } else {
            assert!(!activity.reply_unfinished());
        }
        let closed_at = socket as u64 * life_s;
        let before = restarts;
        let age = replaced_socket_age(life, false, &activity);
        if !take_restart_attempt(&mut restarts, age) {
            run.refused_at_logical_s = Some(closed_at);
            break;
        }
        if before > 0 && restarts == 1 {
            run.age_resets += 1;
        }
        run.opens += 1;
        run.max_charge = run.max_charge.max(restarts);
        let resumed = recovery.allow_resume(life);
        run.resumes_allowed += usize::from(resumed);
        recovery.connected(resumed);
    }
    run.final_charge = restarts;
    run
}

#[test]
fn age_based_restart_reset_follows_the_declared_table() {
    use SocketEnd::*;
    assert_eq!(
        GEMINI_RESTART_LIMIT, 8,
        "the table below is written for a limit of 8"
    );
    assert_eq!(HEALTHY_GEMINI_SOCKET, Duration::from_secs(60));

    // Declared before the first run. 59 sockets of `life` seconds each fit in
    // one logical hour (59 * 61 = 3599). Columns: opens, refusal time, peak and
    // final consecutive charge, resets by age, resets by output, resumes
    // allowed.
    struct Row(&'static str, u64, SocketEnd, BudgetRun);
    let row = |name, life, end, opens, refused, max, last, age, output, resumes| {
        Row(
            name,
            life,
            end,
            BudgetRun {
                opens,
                refused_at_logical_s: refused,
                max_charge: max,
                final_charge: last,
                age_resets: age,
                output_resets: output,
                resumes_allowed: resumes,
            },
        )
    };
    let table = [
        row(
            "real_output_at_threshold_plus_one",
            61,
            RealOutput,
            59,
            None,
            1,
            1,
            0,
            58,
            59,
        ),
        row(
            "idle_at_threshold_plus_one",
            61,
            Idle,
            59,
            None,
            1,
            1,
            58,
            0,
            59,
        ),
        row(
            "briefing_only_at_threshold_plus_one",
            61,
            BriefingOnly,
            59,
            None,
            1,
            1,
            58,
            0,
            59,
        ),
        row(
            "owed_reply_at_threshold_plus_one",
            61,
            OwedReply,
            8,
            Some(549),
            8,
            8,
            0,
            0,
            8,
        ),
        row(
            "idle_at_exact_threshold",
            60,
            Idle,
            59,
            None,
            1,
            1,
            58,
            0,
            59,
        ),
        row(
            "real_output_below_threshold",
            59,
            RealOutput,
            59,
            None,
            1,
            1,
            0,
            58,
            30,
        ),
        row(
            "idle_below_threshold",
            59,
            Idle,
            8,
            Some(531),
            8,
            8,
            0,
            0,
            1,
        ),
        row(
            "briefing_only_below_threshold",
            59,
            BriefingOnly,
            8,
            Some(531),
            8,
            8,
            0,
            0,
            1,
        ),
        row(
            "owed_reply_below_threshold",
            59,
            OwedReply,
            8,
            Some(531),
            8,
            8,
            0,
            0,
            1,
        ),
    ];
    assert_eq!(table.len(), 9);
    for Row(name, life, end, expected) in table {
        assert!(59 * life <= 3600, "{name}");
        assert_eq!(run_restart_budget(59, life, end), expected, "{name}");
    }
}

// Pending tool work across a replacement.
//
// One call, `log_hint` volunteered, is the whole tool effect: executing it once
// moves `volunteered_hints` from 0 to 1, so the counter is the side-effect
// count. The wire counters are the ids of the `toolResponse` frames each socket
// received. A tool continuation is owed, never carried: a response goes to the
// socket that asked for it and to no other, and what survives a replacement is
// the reply the candidate is still owed.

fn tool_call(id: &str) -> crate::gemini::GeminiFunctionCall {
    crate::gemini::GeminiFunctionCall {
        id: id.into(),
        name: crate::runtime::TOOL_LOG_HINT.into(),
        args: serde_json::json!({"requested": false}),
    }
}

fn tool_response_ids(frames: &[serde_json::Value]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|frame| frame.pointer("/toolResponse/functionResponses"))
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .filter_map(|response| response["id"].as_str().map(str::to_owned))
        .collect()
}

fn briefing_turns(frames: &[serde_json::Value]) -> Vec<bool> {
    frames
        .iter()
        .filter_map(|frame| frame.pointer("/clientContent/turnComplete"))
        .filter_map(serde_json::Value::as_bool)
        .collect()
}

impl Rig {
    /// Delivers one tool call from the live socket, as the room loop would.
    async fn receive_tool_call(&mut self, id: &str) {
        let mut context = self.turn.context(
            &mut self.output,
            &mut self.gemini,
            &mut self.board,
            &mut self.media,
        );
        session::handle_gemini_event(
            &self.room,
            &mut context,
            crate::gemini::GeminiEvent::ToolCall(vec![tool_call(id)]),
            Interruptible::Yes,
        )
        .await
        .unwrap();
    }

    /// Joins the old socket's peer and returns every frame it received.
    async fn old_frames(&mut self) -> Vec<serde_json::Value> {
        self.gemini.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), self.old.take().unwrap())
            .await
            .unwrap()
            .unwrap()
    }
}

/// Like `scripted_fresh_gemini`, but a ready socket keeps what it is sent.
async fn recording_fresh_gemini(
    listener: tokio::net::TcpListener,
    script: Vec<FreshStep>,
) -> Vec<(Option<String>, Vec<serde_json::Value>)> {
    let mut seen = Vec::new();
    for step in script {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let setup = socket.next().await.unwrap().unwrap();
        let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
        let handle = setup["setup"]["sessionResumption"]["handle"]
            .as_str()
            .map(str::to_owned);
        let mut frames = Vec::new();
        match step {
            FreshStep::DropAfterSetup => drop(socket),
            FreshStep::Ready => {
                socket
                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                    .await
                    .unwrap();
                while let Some(Ok(frame)) = socket.next().await {
                    if frame.is_close() {
                        break;
                    }
                    frames.push(serde_json::from_str(frame.to_text().unwrap()).unwrap());
                }
            }
            _ => unreachable!("the tool matrix scripts only ready and dropped sockets"),
        }
        seen.push((handle, frames));
    }
    seen
}

#[derive(Clone, Copy, Debug)]
enum ToolPath {
    /// The result went out on the live socket before the replacement began.
    AcceptedBeforeRotation,
    /// The result write failed, so Gemini was never given it.
    UncertainSend,
    /// As the first, and the resume attempt is refused.
    AcceptedThenRefusedResume,
    /// As the first, with the generation already over, so the continuation
    /// flag is the only debt the replacement can read.
    ContinuationFlagAlone,
}

// Declared before the first run. `effect` is `volunteered_hints`; `old` and
// `fresh` are the toolResponse ids each socket received; `charges` is the
// consecutive-restart count after the replacement.
//
//   path                       effect  old         fresh           charges
//   accepted before rotation   1       [call-1]    [] resumed      1
//   uncertain result send      1       []          [] resumed      1
//   accepted, resume refused   1       [call-1]    [] / [] cold    1
//   continuation flag alone    1       [call-1]    [] resumed      1
//
// Exactly one briefing on the socket that took over asks for the reply, and it
// is the last briefing sent: the accepted result leaves a continuation the old
// socket will never deliver, and the failed write leaves the generation
// unanswered. Neither result is resent, and neither executes twice.
#[tokio::test(flavor = "current_thread")]
async fn pending_tool_work_is_owed_but_never_resent_across_a_replacement() {
    if isolated("pending_tool_work_is_owed_but_never_resent_across_a_replacement") {
        return;
    }
    let paths = [
        ToolPath::AcceptedBeforeRotation,
        ToolPath::UncertainSend,
        ToolPath::AcceptedThenRefusedResume,
        ToolPath::ContinuationFlagAlone,
    ];
    for path in paths {
        eprintln!("tool path: {path:?}");
        let checkpoint = serde_json::json!({"resumable": true, "newHandle": "tool-handle"});
        let (fresh, endpoint) = listener().await;
        let mut rig =
            Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), Some(checkpoint)).await;
        rig.endpoint = endpoint;
        let old_reader = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old_reader).await;
        if matches!(path, ToolPath::UncertainSend) {
            // A closed writer fails the response write deterministically; the
            // call still runs. This is the failed-write case `on_tool_calls`
            // documents, not a peer found dead through the reader.
            rig.gemini.shutdown().await.unwrap();
        }
        rig.receive_tool_call("call-1").await;
        assert_eq!(rig.turn.state.volunteered_hints, 1, "{path:?}");
        let accepted = !matches!(path, ToolPath::UncertainSend);
        assert_eq!(
            rig.turn.activity.socket.tool_response_outstanding, accepted,
            "{path:?}"
        );
        assert!(rig.turn.activity.generating, "{path:?}");
        if matches!(path, ToolPath::ContinuationFlagAlone) {
            rig.turn.activity.generating = false;
            assert!(rig.turn.activity.awaiting_reply_since.is_none());
            assert!(!rig.turn.activity.owes_prompt());
        }
        let script = if matches!(path, ToolPath::AcceptedThenRefusedResume) {
            vec![FreshStep::DropAfterSetup, FreshStep::Ready]
        } else {
            vec![FreshStep::Ready]
        };
        let server = tokio::spawn(recording_fresh_gemini(fresh, script));
        let flow = rig.run().await;
        assert!(flow.is_continue(), "{path:?}");
        assert_eq!(rig.loops.restarts, 1, "{path:?}");
        assert_eq!(rig.turn.state.volunteered_hints, 1, "{path:?}");
        assert!(
            !rig.turn.activity.socket.tool_response_outstanding,
            "{path:?}: the continuation belongs to the retired socket"
        );
        assert_eq!(rig.turn.activity.live_socket, 2, "{path:?}");
        let old = rig.old_frames().await;
        assert_eq!(
            tool_response_ids(&old),
            if accepted {
                vec!["call-1".to_string()]
            } else {
                Vec::new()
            },
            "{path:?}"
        );
        let seen = server.await.unwrap();
        for (_, frames) in &seen {
            assert!(tool_response_ids(frames).is_empty(), "{path:?}: resent");
        }
        let (handle, frames) = seen.last().unwrap();
        match path {
            ToolPath::AcceptedThenRefusedResume => {
                assert_eq!(seen.len(), 2);
                assert_eq!(seen[0].0.as_deref(), Some("tool-handle"));
                assert_eq!(*handle, None, "{path:?}: the fallback is cold");
            }
            _ => {
                assert_eq!(seen.len(), 1);
                assert_eq!(handle.as_deref(), Some("tool-handle"), "{path:?}");
            }
        }
        let turns = briefing_turns(frames);
        assert_eq!(turns.last(), Some(&true), "{path:?}");
        assert_eq!(turns.iter().filter(|asked| **asked).count(), 1, "{path:?}");
    }
}

#[derive(Clone, Copy, Debug)]
enum ToolStage {
    /// The resume open is in flight when the terminal arrives.
    ResumePending,
    /// The cold open was refused and its retry backoff is running.
    ColdBackoff,
}
