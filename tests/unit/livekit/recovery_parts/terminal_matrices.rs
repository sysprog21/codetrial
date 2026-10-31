// Part of recovery.rs: included in place, so every item here shares that file's
// module.

#[test]
fn republish_after_commit_keeps_retained_whole_frames_before_the_new_stream() {
    for uncertain in [false, true] {
        let now = Instant::now();
        let mut media = CandidateMedia::new();
        media.audio_bytes = vec![1, 2];
        media.audio_unconfirmed = uncertain;
        media.recovery_audio.push(
            AudioFrame {
                data: vec![7; 160].into(),
                sample_rate: 16000,
                num_channels: 1,
                samples_per_channel: 160,
            },
            now,
        );
        media.begin_audio_publication();
        assert_eq!(
            media.audio_bytes,
            if uncertain { vec![] } else { vec![1, 2] }
        );
        assert_eq!(media.quarantined_audio_bytes, if uncertain { 2 } else { 0 });
        assert!(!media.audio_unconfirmed);
        assert!(
            matches!(media.recovery_audio.pop(now), Some(CandidateInput::BufferedFrame(frame)) if frame.data[0] == 7)
        );
        assert!(matches!(
            media.recovery_audio.pop(now),
            Some(CandidateInput::Yield)
        ));
        assert!(media.recovery_audio.pop(now).is_none());
        media.begin_audio_publication();
        assert!(
            media.recovery_audio.is_empty(),
            "startup does not invent an audio-end"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum PendingOpen {
    Upgrade,
    Setup,
    ReconnectedNotice,
}

#[derive(Clone, Copy, Debug)]
enum MatrixStop {
    Deadline,
    RoomDisconnected,
}

async fn observe_matrix_stop(
    cause: MatrixStop,
    deadline: tokio::time::Instant,
    room: &mpsc::UnboundedSender<RoomEvent>,
    observed: &mut mpsc::UnboundedReceiver<Terminal>,
) {
    // All peer/publication barriers precede pause. Cooperative polling keeps
    // Tokio from auto-advancing an unrelated operation or fixture timeout.
    tokio::time::pause();
    match cause {
        MatrixStop::Deadline => {
            tokio::time::advance(deadline.saturating_duration_since(tokio::time::Instant::now()))
                .await;
        }
        MatrixStop::RoomDisconnected => {
            room.send(RoomEvent::Disconnected {
                reason: DisconnectReason::ClientInitiated,
            })
            .unwrap();
        }
    }
    let frozen = tokio::time::Instant::now();
    let mut terminal = None;
    for _ in 0..4096 {
        if let Ok(value) = observed.try_recv() {
            terminal = Some(value);
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(tokio::time::Instant::now(), frozen);
    tokio::time::resume();
    assert!(
        matches!(
            (cause, terminal),
            (MatrixStop::Deadline, Some(Terminal::Deadline))
                | (MatrixStop::RoomDisconnected, Some(Terminal::RoomClosed))
        ),
        "the original terminal cause must win while the phase remains pending"
    );
}

#[tokio::test]
async fn deadline_and_room_disconnect_cancel_each_pending_open_phase() {
    if isolated("deadline_and_room_disconnect_cancel_each_pending_open_phase") {
        return;
    }
    let mut rows = 0;
    for phase in [
        PendingOpen::Upgrade,
        PendingOpen::Setup,
        PendingOpen::ReconnectedNotice,
    ] {
        for cause in [MatrixStop::Deadline, MatrixStop::RoomDisconnected] {
            let (listener, endpoint) = listener().await;
            let mut rig = Rig::new(endpoint).await;
            rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(8);
            let deadline = rig.loops.deadline_at;
            let room = rig._room_sender.clone();
            let (probe, mut observed) = mpsc::unbounded_channel();
            rig.loops.recovery_terminal = Some(probe);
            let (notice, _release) = rig.recorder.hold_data("interviewer_state", Some(false));
            let (ready, entered) = oneshot::channel();
            let (finish, finished) = oneshot::channel();
            let instructions =
                crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45)
                    .instructions;
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                if matches!(phase, PendingOpen::Upgrade) {
                    use tokio::io::AsyncReadExt;
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        assert!(request.len() < 4096);
                        request.push(socket.read_u8().await.unwrap());
                    }
                    ready.send(()).unwrap();
                    closed_tcp(socket).await;
                } else {
                    let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                    let setup = socket.next().await.unwrap().unwrap();
                    let setup: serde_json::Value =
                        serde_json::from_str(setup.to_text().unwrap()).unwrap();
                    assert_eq!(
                        setup["setup"]["systemInstruction"]["parts"][0]["text"],
                        instructions
                    );
                    if matches!(phase, PendingOpen::ReconnectedNotice) {
                        socket
                            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                            .await
                            .unwrap();
                    }
                    ready.send(()).unwrap();
                    closed_tcp(socket.into_inner()).await;
                }

                // Keep the listener through all fixture teardown. A queued
                // extra attempt is observable without a sleep-based negative.
                finished.await.unwrap();
                assert_eq!(
                    listener.into_std().unwrap().accept().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                1usize
            });
            let stimulus = async {
                entered.await.unwrap();
                if matches!(phase, PendingOpen::ReconnectedNotice) {
                    notice.notified().await;
                }
                observe_matrix_stop(cause, deadline, &room, &mut observed).await;
            };
            let (flow, ()) = tokio::join!(rig.run(), stimulus);
            assert!(flow.is_break(), "phase={phase:?} cause={cause:?}");
            assert_eq!(rig.loops.deadline_at, deadline);
            assert_eq!(rig.loops.restarts, 1);
            let completed_setup = u32::from(matches!(phase, PendingOpen::ReconnectedNotice));
            assert_eq!(
                rig.turn.activity.live_socket,
                1 + u64::from(completed_setup)
            );
            assert_eq!(
                rig.turn.state.evidence_ledger.metrics.live_setup_count,
                completed_setup
            );
            assert!(
                rig.gemini
                    .send_text("must remain a closed placeholder")
                    .await
                    .is_err()
            );
            assert!(observed.try_recv().is_err(), "one terminal intent only");
            assert!(!rig.recorder.take().iter().any(|record| matches!(record,
                room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state" && message["reconnecting"] == false)));
            rig.finish().await;
            finish.send(()).unwrap();
            assert_eq!(server.await.unwrap(), 1);
            rows += 1;
        }
    }
    assert_eq!(rows, 6, "every phase/cause row must execute");
}

#[tokio::test]
async fn absence_and_room_disconnect_really_leave_without_a_report_probe() {
    if isolated("absence_and_room_disconnect_really_leave_without_a_report_probe") {
        return;
    }
    for absent in [true, false] {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::new(endpoint).await;
        rig.loops.recovery_terminal = None;
        let room = rig._room_sender.clone();
        let (ready, entered) = oneshot::channel();
        let server = if absent {
            rig.loops.presence.left(
                Instant::now()
                    .checked_sub(CANDIDATE_ABSENCE_LIMIT)
                    .expect("the fixture needs a representable absence interval"),
            );
            drop(ready);
            None
        } else {
            Some(tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket.next().await.unwrap().unwrap();
                ready.send(()).unwrap();
                closed_tcp(socket.into_inner()).await;
            }))
        };
        let (completed, mut completion) = oneshot::channel();
        let running = async {
            let result = rig.run().await;
            completed.send(()).unwrap();
            result
        };
        let stimulus = async {
            if !absent {
                entered.await.unwrap();
                tokio::time::pause();
                let frozen = tokio::time::Instant::now();
                room.send(RoomEvent::Disconnected {
                    reason: DisconnectReason::ClientInitiated,
                })
                .unwrap();
                let mut done = false;
                for _ in 0..4096 {
                    if completion.try_recv().is_ok() {
                        done = true;
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                assert_eq!(tokio::time::Instant::now(), frozen);
                tokio::time::resume();
                assert!(done, "real leave dispatch must not wait for setup timeout");
            } else {
                completion.await.unwrap();
            }
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        assert_eq!(rig.loops.restarts, usize::from(!absent));
        assert_eq!(rig.turn.activity.live_socket, 1);
        let records = rig.recorder.take();
        assert_eq!(
            records
                .iter()
                .filter(|record| matches!(record, room_link::Sent::Left))
                .count(),
            1
        );
        assert!(!records.iter().any(|record| matches!(record,
            room_link::Sent::Data { topic, .. } if topic.as_deref() == Some(crate::runtime::TOPIC_REPORT))));
        rig.finish().await;
        if let Some(server) = server {
            server.await.unwrap();
        }
    }
}

async fn wait_for_received_usage(reader: &crate::gemini::LiveReaderInspection) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while reader.pending_usage_count() == 0 {
            assert!(!reader.is_finished(), "the peer keeps the reader alive");
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real reader must observe the peer's usage before cancellation");
    assert_eq!(reader.pending_usage_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_joins_live_readers_and_accounts_old_and_rejected_usage_once() {
    if isolated_checked(
        "cancellation_joins_live_readers_and_accounts_old_and_rejected_usage_once",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let lines = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            assert_eq!(
                lines.len(),
                3,
                "each socket observation must be logged once"
            );
            for (line, socket, cause, prompt, event) in [
                (lines[0], 1, "tool", 11, 1),
                (lines[1], 1, "tool", 11, 1),
                (lines[2], 2, "candidate", 29, 2),
            ] {
                for field in [
                    format!("socket={socket}"),
                    format!("cause={cause}"),
                    format!("prompt_tokens={prompt}"),
                    format!("usage_event={event}"),
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "missing {field} in {line}"
                    );
                }
            }
        },
    ) {
        return;
    }
    for after_setup in [false, true] {
        let (listener, endpoint) = listener().await;
        let mut unopened = Some(listener);
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        assert!(!old.is_finished());
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (terminal, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(terminal);
        let (entered, _release) = rig
            .recorder
            .hold_data("interviewer_state", Some(!after_setup));
        let server = if after_setup {
            let listener = unopened.take().unwrap();
            Some(tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket.next().await.unwrap().unwrap();
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

                // No close response or peer EOF can finish the reader for the
                // cancellation path. Only transport release ends this wait.
                closed_tcp(socket.into_inner()).await;
            }))
        } else {
            None
        };
        let tx = rig.tx.clone();
        let fresh_state = std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
        let running = async {
            let flow = rig.run().await;

            // Capture ownership state in the very poll returning recovery.
            // Waiting for the other join branch could let Drop's queued abort
            // finish and hide a missing explicit reader join.
            assert!(
                old.is_finished(),
                "old reader must finish before recovery returns"
            );
            assert_eq!(old.pending_usage_count(), 0);
            let fresh = fresh_state.borrow();
            assert_eq!(fresh.is_some(), after_setup);
            if let Some(fresh) = fresh.as_ref() {
                assert!(
                    fresh.is_finished(),
                    "rejected fresh reader must finish before recovery returns"
                );
                assert_eq!(fresh.pending_usage_count(), 0);
            }
            flow
        };
        let stimulus = async {
            entered.notified().await;
            let fresh = if after_setup {
                let fresh = readers.recv().await.unwrap();
                wait_for_received_usage(&fresh).await;
                assert!(!fresh.is_finished());
                Some(fresh)
            } else {
                None
            };
            fresh_state.replace(fresh);

            // Receipt and all real I/O barriers precede this frozen-clock
            // terminal observation. There is no timer allowance for cleanup.
            end_without_advancing_the_wait(&tx, &mut observed).await;
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        if !after_setup {
            assert!(
                readers.try_recv().is_err(),
                "no new setup before the initial notice"
            );
        }
        assert_eq!(
            rig.turn.activity.live_usage.samples,
            if after_setup { 2 } else { 1 }
        );
        assert_eq!(
            rig.turn.activity.live_usage.prompt,
            if after_setup { 40 } else { 11 }
        );
        assert_eq!(rig.loops.restarts, 1);
        assert_eq!(
            rig.turn.activity.live_socket,
            if after_setup { 2 } else { 1 }
        );
        assert_eq!(
            rig.turn.state.evidence_ledger.metrics.live_setup_count,
            u32::from(after_setup)
        );
        rig.finish().await;
        if let Some(server) = server {
            server.await.unwrap();
        }
        if let Some(listener) = unopened {
            assert_eq!(
                listener.into_std().unwrap().accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum OpenStop {
    End,
    Deadline,
    RoomDisconnected,
    EventsEof,
    Absent,
}

async fn stop_pending_open(
    cause: OpenStop,
    deadline: tokio::time::Instant,
    tx: &mpsc::UnboundedSender<Packet>,
    room: &mut Option<mpsc::UnboundedSender<RoomEvent>>,
    observed: &mut mpsc::UnboundedReceiver<Terminal>,
    presence_now: &std::sync::Arc<std::sync::Mutex<Instant>>,
) {
    tokio::time::pause();
    match cause {
        OpenStop::End => control(tx, serde_json::json!({"type":"end_interview"})),
        OpenStop::Deadline => {
            // Account for Tokio's millisecond timer resolution at the deadline.
            tokio::time::advance(
                deadline.saturating_duration_since(tokio::time::Instant::now())
                    + Duration::from_millis(1),
            )
            .await;
        }
        OpenStop::RoomDisconnected => room
            .as_ref()
            .unwrap()
            .send(RoomEvent::Disconnected {
                reason: DisconnectReason::ClientInitiated,
            })
            .unwrap(),
        OpenStop::EventsEof => drop(room.take().unwrap()),
        OpenStop::Absent => {
            let mut now = presence_now.lock().unwrap();
            *now += CANDIDATE_ABSENCE_LIMIT;
            drop(now);
            tx.send(Packet {
                topic: Some(TOPIC_CONTROL.into()),
                sender: Some("bystander".into()),
                payload: b"{}".to_vec(),
            })
            .unwrap();
        }
    }
    let frozen = tokio::time::Instant::now();
    let mut terminal = None;
    for _ in 0..4096 {
        if let Ok(value) = observed.try_recv() {
            terminal = Some(value);
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(tokio::time::Instant::now(), frozen);
    tokio::time::resume();
    assert!(
        matches!(
            (cause, terminal),
            (OpenStop::End, Some(Terminal::End { .. }))
                | (OpenStop::Absent, Some(Terminal::Absent))
                | (OpenStop::Deadline, Some(Terminal::Deadline))
                | (
                    OpenStop::RoomDisconnected | OpenStop::EventsEof,
                    Some(Terminal::RoomClosed)
                )
        ),
        "the terminal must win before the held operation or timeout completes: {cause:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_matrix_joins_readers_at_pending_open_boundaries() {
    if isolated_checked(
        "terminal_matrix_joins_readers_at_pending_open_boundaries",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            let mut expected = Vec::new();
            for after_setup in [false, false, false, true, false, false, false, true] {
                for _ in 0..5 {
                    expected.push((1, "tool", 11, 1));
                    if after_setup {
                        expected.push((2, "candidate", 29, 2));
                    }
                }
            }
            assert_eq!(usage.len(), 50);
            for (line, (socket, cause, prompt, event)) in usage.into_iter().zip(expected) {
                for field in [
                    format!("socket={socket}"),
                    format!("cause={cause}"),
                    format!("prompt_tokens={prompt}"),
                    format!("usage_event={event}"),
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
        },
    ) {
        return;
    }
    let mut rows = 0;
    for checkpoint in [false, true] {
        for phase in [
            None,
            Some(PendingOpen::Upgrade),
            Some(PendingOpen::Setup),
            Some(PendingOpen::ReconnectedNotice),
        ] {
            for cause in [
                OpenStop::End,
                OpenStop::Deadline,
                OpenStop::RoomDisconnected,
                OpenStop::EventsEof,
                OpenStop::Absent,
            ] {
                let (listener, endpoint) = listener().await;
                let mut unopened = Some(listener);
                let mut rig = Rig::with_old_checkpoint(endpoint, Some(11), checkpoint).await;
                rig.gemini.input_cause = Some("tool");
                let old = rig.gemini.inspect_reader_for_test().unwrap();
                wait_for_received_usage(&old).await;
                assert_eq!(rig.gemini.recovery_handle(&rig.keys).is_some(), checkpoint);
                rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(8);
                let deadline = rig.loops.deadline_at;
                let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
                if matches!(cause, OpenStop::Absent) {
                    rig.loops.presence.left(*presence_now.lock().unwrap());
                    rig.loops.presence.test_now = Some(std::sync::Arc::clone(&presence_now));
                }
                let after_setup = matches!(phase, Some(PendingOpen::ReconnectedNotice));
                let (probe, mut observed) = mpsc::unbounded_channel();
                rig.loops.recovery_terminal = Some(probe);
                let (phase_probe, mut phases) = mpsc::unbounded_channel();
                rig.loops.recovery_phase = Some(phase_probe);
                let (reader, mut readers) = mpsc::unbounded_channel();
                rig.loops.recovery_reader = Some(reader);
                let (notice, _release) = rig
                    .recorder
                    .hold_data("interviewer_state", Some(phase.is_none()));
                let (ready, entered) = oneshot::channel();
                let (finish, finished) = oneshot::channel();
                let instructions =
                    crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45)
                        .instructions;
                let server = phase.map(|phase| {
                    let listener = unopened.take().unwrap();
                    tokio::spawn(async move {
                        let (mut socket, _) = listener.accept().await.unwrap();
                        if matches!(phase, PendingOpen::Upgrade) {
                            use tokio::io::AsyncReadExt;
                            let mut request = Vec::new();
                            while !request.ends_with(b"\r\n\r\n") {
                                assert!(request.len() < 4096);
                                request.push(socket.read_u8().await.unwrap());
                            }
                            ready.send(()).unwrap();
                            closed_tcp(socket).await;
                        } else {
                            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                            let setup = socket.next().await.unwrap().unwrap();
                            let setup: serde_json::Value =
                                serde_json::from_str(setup.to_text().unwrap()).unwrap();
                            assert_eq!(
                                setup["setup"]["systemInstruction"]["parts"][0]["text"],
                                instructions
                            );
                            if checkpoint {
                                assert_eq!(
                                    setup["setup"]["sessionResumption"]["handle"],
                                    "matrix-checkpoint"
                                );
                            } else {
                                assert!(setup["setup"]["sessionResumption"]["handle"].is_null());
                            }
                            if after_setup {
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
                            }
                            ready.send(()).unwrap();
                            closed_tcp(socket.into_inner()).await;
                        }
                        finished.await.unwrap();
                        assert_eq!(
                            listener.into_std().unwrap().accept().unwrap_err().kind(),
                            std::io::ErrorKind::WouldBlock
                        );
                    })
                });

                // Move the sole sender out of the rig. EOF must be a real
                // receiver closure, not a Disconnected event pretending to be
                // one.
                let (unused, _) = mpsc::unbounded_channel();
                let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
                let tx = rig.tx.clone();
                let fresh_state =
                    std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
                let running = async {
                    let flow = rig.run().await;
                    assert!(
                        old.is_finished(),
                        "old reader joined in the return poll: {phase:?} {cause:?}"
                    );
                    assert_eq!(old.pending_usage_count(), 0);
                    let fresh = fresh_state.borrow();
                    assert_eq!(fresh.is_some(), after_setup);
                    if let Some(fresh) = fresh.as_ref() {
                        assert!(
                            fresh.is_finished(),
                            "rejected fresh reader joined in the return poll: {cause:?}"
                        );
                        assert_eq!(fresh.pending_usage_count(), 0);
                    }
                    flow
                };
                let stimulus = async {
                    if phase.is_some() {
                        entered.await.unwrap();
                    }
                    if phase.is_none() || after_setup {
                        notice.notified().await;
                    }
                    if after_setup {
                        let fresh = readers.recv().await.unwrap();
                        wait_for_received_usage(&fresh).await;
                        assert!(!fresh.is_finished());
                        fresh_state.replace(Some(fresh));
                    }
                    stop_pending_open(
                        cause,
                        deadline,
                        &tx,
                        &mut room,
                        &mut observed,
                        &presence_now,
                    )
                    .await;
                };
                let (flow, ()) = tokio::join!(running, stimulus);
                assert!(flow.is_break());
                let entered_phases =
                    std::iter::from_fn(|| phases.try_recv().ok()).collect::<Vec<_>>();
                let expected_phases: &[&str] = if phase.is_none() {
                    &[]
                } else if checkpoint {
                    &["close", "resume"]
                } else {
                    &["close"]
                };
                assert_eq!(
                    entered_phases, expected_phases,
                    "phase={phase:?} checkpoint={checkpoint} cause={cause:?}"
                );
                assert_eq!(rig.loops.deadline_at, deadline);
                assert_eq!(rig.loops.restarts, 1);
                assert_eq!(rig.turn.activity.live_socket, 1 + u64::from(after_setup));
                assert_eq!(
                    rig.turn.state.evidence_ledger.metrics.live_setup_count,
                    u32::from(after_setup)
                );
                assert_eq!(
                    rig.turn.activity.live_usage.samples,
                    if after_setup { 2 } else { 1 }
                );
                assert_eq!(
                    rig.turn.activity.live_usage.prompt,
                    if after_setup { 40 } else { 11 }
                );
                assert!(
                    rig.gemini
                        .send_text("must remain a closed placeholder")
                        .await
                        .is_err()
                );
                assert!(readers.try_recv().is_err());
                assert!(observed.try_recv().is_err());
                assert!(!rig.recorder.take().iter().any(|record| matches!(record,
                room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state" && message["reconnecting"] == false)));
                rig.finish().await;
                if let Some(server) = server {
                    finish.send(()).unwrap();
                    server.await.unwrap();
                }
                if let Some(listener) = unopened {
                    assert_eq!(
                        listener.into_std().unwrap().accept().unwrap_err().kind(),
                        std::io::ErrorKind::WouldBlock
                    );
                }
                rows += 1;
            }
        }
    }
    assert_eq!(rows, 40);
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_matrix_cancels_a_physically_blocked_close() {
    if isolated_checked(
        "terminal_matrix_cancels_a_physically_blocked_close",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            assert_eq!(usage.len(), 5);
            for line in usage {
                for field in [
                    "socket=1",
                    "cause=tool",
                    "prompt_tokens=11",
                    "usage_event=1",
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
        },
    ) {
        return;
    }
    let mut rows = 0;
    for cause in [
        OpenStop::End,
        OpenStop::Deadline,
        OpenStop::RoomDisconnected,
        OpenStop::EventsEof,
        OpenStop::Absent,
    ] {
        let (unopened, endpoint) = listener().await;
        let mut rig = Rig::new(endpoint).await;
        rig.gemini.shutdown().await.unwrap();
        rig.old.take().unwrap().await.unwrap();
        let listener = tokio::net::TcpSocket::new_v4().unwrap();
        listener.set_recv_buffer_size(1024).unwrap();
        listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = listener.listen(1).unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let (read, reading) = oneshot::channel();
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    r#"{"usageMetadata":{"promptTokenCount":11}}"#.into(),
                ))
                .await
                .unwrap();
            let mut socket = socket.into_inner();
            reading.await.unwrap();
            let mut bytes = [0; 65536];
            loop {
                match socket.read(&mut bytes).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                    other => panic!("cancelled close must release the transport: {other:?}"),
                }
            }
            Vec::new()
        });
        let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
        rig.gemini = crate::livekit::tests::fixture_session_at(&endpoint, &rig.keys, &boot, None)
            .await
            .unwrap();
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.old = Some(server);
        let mut blocked = false;
        for _ in 0..64 {
            if tokio::time::timeout(
                Duration::from_millis(30),
                rig.gemini
                    .writer_for_test()
                    .send(Message::Binary(vec![0; 256 * 1024].into())),
            )
            .await
            .is_err()
            {
                blocked = true;
                break;
            }
        }
        assert!(
            blocked,
            "the fixture must establish real writer backpressure"
        );

        // Unlike a before-write gate, the peer is not reading an already
        // stalled writer. The original deadline precedes its 2s close timeout.
        rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(1);
        let deadline = rig.loops.deadline_at;
        let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        if matches!(cause, OpenStop::Absent) {
            rig.loops.presence.left(*presence_now.lock().unwrap());
            rig.loops.presence.test_now = Some(std::sync::Arc::clone(&presence_now));
        }
        let (phase, mut phases) = mpsc::unbounded_channel();
        rig.loops.recovery_phase = Some(phase);
        let (probe, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(probe);
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (unused, _) = mpsc::unbounded_channel();
        let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
        let tx = rig.tx.clone();
        let running = async {
            let flow = rig.run().await;
            assert!(
                old.is_finished(),
                "reader must join in the close return poll: {cause:?}"
            );
            assert_eq!(old.pending_usage_count(), 0);
            flow
        };
        let stimulus = async {
            while phases.recv().await != Some("close") {}
            assert!(
                tokio::time::Instant::now() < deadline,
                "the close barrier must precede the original deadline"
            );
            stop_pending_open(
                cause,
                deadline,
                &tx,
                &mut room,
                &mut observed,
                &presence_now,
            )
            .await;
            read.send(()).unwrap();
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        assert_eq!(rig.loops.deadline_at, deadline);
        assert_eq!(rig.loops.restarts, 1);
        assert_eq!(rig.turn.activity.live_socket, 1);
        assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 0);
        assert_eq!(rig.turn.activity.live_usage.samples, 1);
        assert_eq!(rig.turn.activity.live_usage.prompt, 11);
        assert!(readers.try_recv().is_err());
        assert!(observed.try_recv().is_err());
        assert!(
            rig.gemini
                .send_text("no socket may be installed")
                .await
                .is_err()
        );
        assert!(!rig.recorder.take().iter().any(|record| matches!(record,
            room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state" && message["reconnecting"] == false)));
        rig.finish().await;
        assert_eq!(
            unopened.into_std().unwrap().accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        rows += 1;
    }
    assert_eq!(rows, 5);
}

#[derive(Clone, Copy, Debug)]
enum PrefixPhase {
    AudioWrite,
    EndWrite,
    InitialCollect,
    Collect,
    NoteGate,
}

const PREFIX_PHASES: [PrefixPhase; 5] = [
    PrefixPhase::AudioWrite,
    PrefixPhase::EndWrite,
    PrefixPhase::InitialCollect,
    PrefixPhase::Collect,
    PrefixPhase::NoteGate,
];
const PREFIX_STOPS: [OpenStop; 5] = [
    OpenStop::End,
    OpenStop::Deadline,
    OpenStop::RoomDisconnected,
    OpenStop::EventsEof,
    OpenStop::Absent,
];
const PREFIX_TEXT: &str = "Keep distinct indices in the answer.";

impl PrefixPhase {
    fn label(self) -> &'static str {
        match self {
            Self::AudioWrite => "prefix_audio_write",
            Self::EndWrite => "prefix_end_audio_write",
            Self::InitialCollect => "prefix_collect_initial",
            Self::Collect => "prefix_collect",
            Self::NoteGate => "prefix_listening_context",
        }
    }

    fn physical(self) -> bool {
        matches!(self, Self::AudioWrite | Self::EndWrite)
    }

    fn transcribed(self) -> bool {
        matches!(self, Self::Collect | Self::NoteGate)
    }
}

async fn prefix_pending_phase(
    phases: &mut mpsc::UnboundedReceiver<&'static str>,
    target: &'static str,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let phase = phases
                .recv()
                .await
                .expect("the prefix probe must remain owned");
            assert!(
                !matches!(phase, "close" | "resume"),
                "prefix finished before cancellation"
            );
            if phase == target {
                break;
            }
        }
    })
    .await
    .expect("the targeted prefix operation must actually pend");
}

#[tokio::test(flavor = "current_thread")]
async fn prefix_terminal_matrix_preserves_received_evidence_before_any_restart() {
    if isolated_checked(
        "prefix_terminal_matrix_preserves_received_evidence_before_any_restart",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            let mut expected = Vec::new();
            for phase in PREFIX_PHASES {
                for _ in PREFIX_STOPS {
                    expected.push((11, 1));
                    if phase.transcribed() {
                        expected.push((29, 2));
                    }
                }
            }
            assert_eq!(usage.len(), 35);
            for (line, (prompt, event)) in usage.into_iter().zip(expected) {
                for field in [
                    "socket=1".to_string(),
                    "cause=tool".to_string(),
                    format!("prompt_tokens={prompt}"),
                    format!("usage_event={event}"),
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
            let ingress = stderr
                .lines()
                .filter(|line| line.starts_with("recovery ingress: "))
                .collect::<Vec<_>>();
            assert_eq!(ingress.len(), 25);
            for (index, line) in ingress.into_iter().enumerate() {
                let uncertain = if PREFIX_PHASES[index / 5].physical() {
                    2
                } else {
                    0
                };
                for field in [
                    "dropped_bytes=0".to_string(),
                    format!("unconfirmed_bytes={uncertain}"),
                    "unsent_bytes=0".to_string(),
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
            for forbidden in [
                "speech handoff:",
                "recovery setup discarded:",
                "Gemini write stalled",
            ] {
                assert!(
                    !stderr.contains(forbidden),
                    "unexpected completed handoff or timer: {forbidden}"
                );
            }
        },
    ) {
        return;
    }
    let mut rows = 0;
    for phase in PREFIX_PHASES {
        for cause in PREFIX_STOPS {
            eprintln!("prefix matrix: phase={phase:?} cause={cause:?}");
            let (unopened, endpoint) = listener().await;
            let mut rig = Rig::new(endpoint).await;
            rig.gemini.shutdown().await.unwrap();
            rig.old.take().unwrap().await.unwrap();
            let listener = tokio::net::TcpSocket::new_v4().unwrap();
            listener.set_recv_buffer_size(1024).unwrap();
            listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
            let listener = listener.listen(1).unwrap();
            let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
            let (read, reading) = oneshot::channel();
            let (ready, entered) = oneshot::channel();
            let server = tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket.next().await.unwrap().unwrap();
                socket
                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                    .await
                    .unwrap();
                socket
                    .send(Message::Text(
                        r#"{"usageMetadata":{"promptTokenCount":11}}"#.into(),
                    ))
                    .await
                    .unwrap();
                if phase.physical() {
                    let mut socket = socket.into_inner();
                    reading.await.unwrap();
                    let mut bytes = [0; 65536];
                    loop {
                        match socket.read(&mut bytes).await {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {
                                break;
                            }
                            other => {
                                panic!("cancelled prefix must release stalled transport: {other:?}")
                            }
                        }
                    }
                    return Vec::new();
                }
                let end = socket.next().await.unwrap().unwrap();
                let end: serde_json::Value = serde_json::from_str(end.to_text().unwrap()).unwrap();
                assert_eq!(end["realtimeInput"]["audioStreamEnd"], true);
                let mut messages = vec![end];
                if phase.transcribed() {
                    socket.send(Message::Text(serde_json::json!({"serverContent":{"inputTranscription":{"text":PREFIX_TEXT}},"usageMetadata":{"promptTokenCount":29}}).to_string().into())).await.unwrap();
                    if matches!(phase, PrefixPhase::Collect) {
                        let note = socket.next().await.unwrap().unwrap();
                        let note: serde_json::Value =
                            serde_json::from_str(note.to_text().unwrap()).unwrap();
                        assert_eq!(note["clientContent"]["turnComplete"], false);
                        assert!(note.to_string().contains("Keep listening"));
                        messages.push(note);
                    }
                }
                ready.send(()).unwrap();
                closed_tcp(socket.into_inner()).await;
                messages
            });
            let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
            rig.gemini =
                crate::livekit::tests::fixture_session_at(&endpoint, &rig.keys, &boot, None)
                    .await
                    .unwrap();
            rig.gemini.input_cause = Some("tool");
            let old = rig.gemini.inspect_reader_for_test().unwrap();
            wait_for_received_usage(&old).await;
            rig.old = Some(server);
            rig.expected_old_messages = if phase.physical() {
                0
            } else if matches!(phase, PrefixPhase::Collect) {
                2
            } else {
                1
            };
            if phase.physical() {
                let mut blocked = false;
                for _ in 0..64 {
                    if tokio::time::timeout(
                        Duration::from_millis(30),
                        rig.gemini
                            .writer_for_test()
                            .send(Message::Binary(vec![0; 256 * 1024].into())),
                    )
                    .await
                    .is_err()
                    {
                        blocked = true;
                        break;
                    }
                }
                assert!(
                    blocked,
                    "the prefix writer must already be physically stalled"
                );
                rig.media.audio_bytes = vec![1, 2];
                rig.media.audio_unconfirmed = matches!(phase, PrefixPhase::EndWrite);
            }
            rig.prefix = true;
            rig.media.identity = Some("candidate".into());
            rig.loops.deferred_restart = DeferredRestart(true);
            rig.turn
                .activity
                .note_candidate_voice_activity(true, Instant::now());
            let gate = if phase.transcribed() {
                // An unrelated earlier gate must survive the tagged note send.
                rig.loops.recovery_writes.push_back(WriteBarrier {
                    phase: "recovery_brief",
                    entered: std::sync::Arc::new(tokio::sync::Notify::new()),
                    release: std::sync::Arc::new(tokio::sync::Notify::new()),
                });
                let entered = std::sync::Arc::new(tokio::sync::Notify::new());
                let release = std::sync::Arc::new(tokio::sync::Notify::new());
                rig.loops.recovery_writes.push_back(WriteBarrier {
                    phase: "prefix_listening_context",
                    entered: std::sync::Arc::clone(&entered),
                    release: std::sync::Arc::clone(&release),
                });
                Some((entered, release))
            } else {
                None
            };
            rig.loops.deadline_at = tokio::time::Instant::now()
                + Duration::from_secs(if phase.transcribed() { 8 } else { 1 });
            let deadline = rig.loops.deadline_at;
            let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
            if matches!(cause, OpenStop::Absent) {
                rig.loops.presence.left(*presence_now.lock().unwrap());
                rig.loops.presence.test_now = Some(std::sync::Arc::clone(&presence_now));
            }
            let (probe, mut observed) = mpsc::unbounded_channel();
            rig.loops.recovery_terminal = Some(probe);
            let (phase_probe, mut phases) = mpsc::unbounded_channel();
            rig.loops.recovery_phase = Some(phase_probe);
            let (reader, mut readers) = mpsc::unbounded_channel();
            rig.loops.recovery_reader = Some(reader);
            let (unused, _) = mpsc::unbounded_channel();
            let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
            let tx = rig.tx.clone();
            let running = async {
                let flow = rig.run().await;
                assert!(
                    old.is_finished(),
                    "prefix reader must join in return poll: {phase:?} {cause:?}"
                );
                assert_eq!(old.pending_usage_count(), 0);
                flow
            };
            let stimulus = async {
                if let Some((gate_entered, release)) = &gate {
                    tokio::time::timeout(Duration::from_secs(1), gate_entered.notified())
                        .await
                        .expect("the matching prefix note gate must be entered");
                    if matches!(phase, PrefixPhase::Collect) {
                        if matches!(cause, OpenStop::Deadline) {
                            tokio::time::pause();
                            let left =
                                deadline.saturating_duration_since(tokio::time::Instant::now());
                            tokio::time::advance(
                                left.checked_sub(Duration::from_millis(190)).unwrap(),
                            )
                            .await;
                            tokio::time::resume();
                        }
                        release.notify_one();
                    }
                }
                if !phase.physical() {
                    entered.await.unwrap();
                }
                prefix_pending_phase(&mut phases, phase.label()).await;
                assert!(tokio::time::Instant::now() < deadline);
                assert_eq!(
                    old.pending_usage_count(),
                    if phase.transcribed() { 2 } else { 1 }
                );
                assert!(!old.is_finished());
                stop_pending_open(
                    cause,
                    deadline,
                    &tx,
                    &mut room,
                    &mut observed,
                    &presence_now,
                )
                .await;
                if phase.physical() {
                    read.send(()).unwrap();
                }
            };
            let (flow, ()) = tokio::join!(running, stimulus);
            assert!(flow.is_break());
            assert_eq!(rig.loops.deadline_at, deadline);
            assert_eq!(rig.loops.restarts, 0);
            assert_eq!(rig.turn.activity.live_socket, 1);
            assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 0);
            assert_eq!(
                rig.turn.activity.live_usage.samples,
                if phase.transcribed() { 2 } else { 1 }
            );
            assert_eq!(
                rig.turn.activity.live_usage.prompt,
                if phase.transcribed() { 40 } else { 11 }
            );
            assert_eq!(
                rig.turn.state.evidence_ledger.metrics.turn_prompt_count,
                u32::from(phase.transcribed())
            );
            assert_eq!(
                rig.turn
                    .state
                    .transcript
                    .iter()
                    .filter(|line| line.contains(PREFIX_TEXT))
                    .count(),
                usize::from(phase.transcribed())
            );
            assert!(
                rig.gemini
                    .send_text("no socket may be installed")
                    .await
                    .is_err()
            );
            assert_eq!(
                rig.loops.recovery_writes.len(),
                usize::from(phase.transcribed())
            );
            if let Some(unused) = rig.loops.recovery_writes.front() {
                assert_eq!(unused.phase, "recovery_brief");
            }
            assert!(readers.try_recv().is_err());
            assert!(observed.try_recv().is_err());
            assert!(
                phases.try_recv().is_err(),
                "no operation after the target may pend"
            );
            rig.turn.transcripts.flush(Duration::from_secs(1)).await;
            assert_eq!(rig.recorder.take().iter().filter(|record| matches!(record,
                room_link::Sent::Text { topic, text, .. } if topic == crate::runtime::TOPIC_TRANSCRIPTION && text == PREFIX_TEXT)).count(), usize::from(phase.transcribed()));
            rig.finish().await;
            assert_eq!(
                unopened.into_std().unwrap().accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            rows += 1;
        }
    }
    assert_eq!(rows, 25);
}
