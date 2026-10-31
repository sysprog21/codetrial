// Part of recovery.rs: included in place, so every item here shares that file's
// module.

#[tokio::test]
async fn an_end_control_cancels_a_pending_setup_without_setup_complete() {
    if isolated("an_end_control_cancels_a_pending_setup_without_setup_complete") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let tx = rig.tx.clone();
    let (ready, entered) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        assert!(socket.next().await.unwrap().unwrap().is_text());
        ready.send(()).unwrap();
        closed_tcp(socket.into_inner()).await;
    });
    let stimulus = async {
        entered.await.unwrap();
        control(&tx, serde_json::json!({"type":"end_interview"}));
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 0);
    server.await.unwrap();
    rig.finish().await;
}

#[tokio::test]
async fn an_end_control_cancels_the_initial_notice_before_it_is_released() {
    if isolated("an_end_control_cancels_the_initial_notice_before_it_is_released") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let (entered, _release) = rig.recorder.hold_data("interviewer_state", Some(true));
    let tx = rig.tx.clone();
    let stimulus = async {
        entered.notified().await;
        control(&tx, serde_json::json!({"type":"end_interview"}));
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert_eq!(rig.turn.activity.live_socket, 1);
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert!(!rig.recorder.take().iter().any(|record| matches!(record,
        room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state")));
    rig.finish().await;
}

#[tokio::test]
async fn an_end_after_setup_drains_and_discards_the_uncommitted_socket() {
    if isolated("an_end_after_setup_drains_and_discards_the_uncommitted_socket") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let (entered, _release) = rig.recorder.hold_data("interviewer_state", Some(false));
    let tx = rig.tx.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        closed_tcp(socket.into_inner()).await;
    });
    let stimulus = async {
        entered.notified().await;
        control(&tx, serde_json::json!({"type":"end_interview"}));
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert_eq!(
        rig.turn.activity.live_socket, 2,
        "successful setup is still an opened socket"
    );
    assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 1);
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert!(rig.gemini.send_text("must be closed").await.is_err());
    server.await.unwrap();
    rig.finish().await;
}

#[test]
fn ingress_has_independent_frame_byte_and_age_bounds() {
    let now = Instant::now();
    let frame = |samples| AudioFrame {
        data: std::borrow::Cow::Owned(vec![1; samples]),
        sample_rate: 16000,
        num_channels: 1,
        samples_per_channel: samples as u32,
    };
    let mut ring = Ingress::default();
    for _ in 0..4001 {
        ring.push(frame(1), now);
    }
    assert_eq!(ring.frames.len(), 4000);
    assert_eq!(ring.dropped_bytes, 2);
    assert!(ring.pop(now + INGRESS_AGE).is_none());
    assert_eq!(ring.bytes, 0);
    assert_eq!(ring.dropped_bytes, 8002);
    ring.push(frame(INGRESS_BYTES / 2 + 1), now);
    assert!(ring.is_empty());
    ring.push(frame(INGRESS_BYTES / 2), now);
    // Exactly the byte bound is kept: only one past it is refused.
    assert_eq!(ring.bytes, INGRESS_BYTES);
    ring.push(frame(160), now);
    assert_eq!(ring.frames.len(), 1);
    assert_eq!(ring.bytes, 320);
    assert!(
        matches!(ring.pop(now), Some(CandidateInput::BufferedFrame(frame)) if frame.data.len() == 160)
    );
    assert_eq!(ring.bytes, 0, "popping a frame gives its bytes back");
    ring.push(frame(0), now);
    assert!(ring.is_empty());
}

/// The analyzer sums the `recovery ingress` line per room, and the ingress
/// count is cumulative across replacements, so a line must carry only the
/// drops the previous line did not.
#[test]
fn each_ingress_log_line_carries_only_new_drops() {
    let now = Instant::now();
    let frame = |samples| AudioFrame {
        data: std::borrow::Cow::Owned(vec![1; samples]),
        sample_rate: 16000,
        num_channels: 1,
        samples_per_channel: samples as u32,
    };
    let mut ring = Ingress::default();
    assert_eq!(ring.take_unlogged_drops(), 0);
    for _ in 0..4001 {
        ring.push(frame(1), now);
    }
    assert_eq!(ring.take_unlogged_drops(), 2);
    assert_eq!(
        ring.take_unlogged_drops(),
        0,
        "asking again repeats nothing"
    );
    assert!(ring.pop(now + INGRESS_AGE).is_none());
    assert_eq!(ring.take_unlogged_drops(), 8000);
    assert_eq!(ring.dropped_bytes, 8002, "the cumulative count is kept");
}

#[tokio::test]
async fn the_original_deadline_cancels_setup_without_extending_the_interview() {
    if isolated("the_original_deadline_cancels_setup_without_extending_the_interview") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(5);
    let deadline = rig.loops.deadline_at;
    let (ready, entered) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        ready.send(()).unwrap();
        closed_tcp(socket.into_inner()).await;
    });
    let stimulus = async {
        entered.await.unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::time::resume();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::Deadline
    ));
    assert_eq!(rig.loops.deadline_at, deadline);
    assert_eq!(rig.loops.restarts, 1);
    server.await.unwrap();
    rig.finish().await;
}

#[tokio::test]
async fn an_end_control_cancels_retry_backoff_before_the_next_open() {
    if isolated("an_end_control_cancels_retry_backoff_before_the_next_open") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let recorder = std::sync::Arc::clone(&rig.recorder);
    let tx = rig.tx.clone();
    let (probe, mut observed) = mpsc::unbounded_channel();
    rig.loops.recovery_terminal = Some(probe);
    let (ready, rejected) = oneshot::channel();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        let (retry_entered, retry_release) = recorder.hold_data("interviewer_state", Some(true));
        retry_release.notify_one();
        socket
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        ready.send(retry_entered).unwrap();
        // Keep the listener available; an unintended second open is observable.
        listener
    });
    let stimulus = async {
        let retry_entered = rejected.await.unwrap();
        retry_entered.notified().await;
        end_without_advancing_the_wait(&tx, &mut observed).await;
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert_eq!(
        rig.loops.restarts, 2,
        "the retry charge survives cancelled backoff"
    );

    let listener = server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    rig.finish().await;
}

#[tokio::test]
async fn pending_setup_keeps_thinking_and_latest_editor_before_briefing() {
    if isolated("pending_setup_keeps_thinking_and_latest_editor_before_briefing") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.turn.activity.owe_prompt(
        Instant::now(),
        Some("[SYSTEM EVENT] Explain the current editor.".into()),
    );
    let tx = rig.tx.clone();
    let (setup, entered) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        setup.send(()).unwrap();
        released.await.unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let body = socket.next().await.unwrap().unwrap();
        let body: serde_json::Value = serde_json::from_str(body.to_text().unwrap()).unwrap();
        assert_eq!(body["clientContent"]["turnComplete"], false);
        assert!(body.to_string().contains("latest_editor_marker"));
        assert!(socket.next().await.unwrap().unwrap().is_close());
    });
    let stimulus = async {
        entered.await.unwrap();
        control(&tx, serde_json::json!({"type":"thinking", "thinking":true}));
        tx.send(Packet {
            sender: Some("candidate".into()),
            topic: Some(crate::runtime::TOPIC_CODE_UPDATE.into()),
            payload: serde_json::to_vec(
                &serde_json::json!({"code":"fn latest_editor_marker() {}", "language":"rust"}),
            )
            .unwrap(),
        })
        .unwrap();

        // The new setup is allowed only after those queued controls; the
        // driver's biased receive path must reconcile them before the result.
        release.send(()).unwrap();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_continue());
    assert!(rig.turn.state.interviewer_silenced());
    assert!(rig.turn.state.code.contains("latest_editor_marker"));
    assert_eq!(rig.turn.state.code_shown, rig.turn.state.code);
    assert!(rig.turn.state.owed_reply_on_resume.is_some());
    assert!(rig.terminal.try_recv().is_err());
    rig.finish().await;
    server.await.unwrap();
}

#[tokio::test]
async fn cancelling_prefix_collection_keeps_received_transcript_and_usage() {
    if isolated("cancelling_prefix_collection_keeps_received_transcript_and_usage") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.gemini.shutdown().await.unwrap();
    assert!(rig.old.take().unwrap().await.unwrap().is_empty());
    let (listener, endpoint) = listener().await;
    let (ready, entered) = oneshot::channel();
    let server = tokio::spawn(async move {
        use tokio_tungstenite::tungstenite::{Error as WsError, error::ProtocolError};
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let end = socket.next().await.unwrap().unwrap();
        let end: serde_json::Value = serde_json::from_str(end.to_text().unwrap()).unwrap();
        assert_eq!(end["realtimeInput"]["audioStreamEnd"], true);
        socket.send(Message::Text(r#"{"serverContent":{"inputTranscription":{"text":"Keep distinct indices in the answer."}},"usageMetadata":{"promptTokenCount":42}}"#.into())).await.unwrap();
        let note = socket.next().await.unwrap().unwrap();
        let note: serde_json::Value = serde_json::from_str(note.to_text().unwrap()).unwrap();
        assert_eq!(note["clientContent"]["turnComplete"], false);
        ready.send(()).unwrap();
        match socket.next().await {
            None | Some(Err(WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake))) => {}
            Some(Err(WsError::Io(error)))
                if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("cancelled prefix must release its old transport: {other:?}"),
        }
        vec![end, note]
    });
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
    rig.gemini = crate::livekit::tests::fixture_session_at(&endpoint, &rig.keys, &boot, None)
        .await
        .unwrap();
    rig.old = Some(server);
    rig.expected_old_messages = 2;
    rig.prefix = true;
    rig.media.audio_bytes = vec![1, 2];
    rig.media.audio_unconfirmed = true;
    rig.media.identity = Some("candidate".into());
    rig.loops.deferred_restart = DeferredRestart(true);
    rig.turn
        .activity
        .note_candidate_voice_activity(true, Instant::now());
    let tx = rig.tx.clone();
    let stimulus = async {
        entered.await.unwrap();
        control(&tx, serde_json::json!({"type":"end_interview"}));
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert!(
        rig.turn
            .state
            .transcript
            .iter()
            .any(|line| line.contains("Keep distinct indices"))
    );
    assert_eq!(rig.turn.activity.live_usage.samples, 1);
    assert_eq!(rig.turn.activity.live_usage.prompt, 42);
    assert_eq!(rig.loops.restarts, 0, "no replacement open began");
    rig.finish().await;
}

#[tokio::test]
async fn recovery_preserves_never_sent_tail_and_does_not_replay_an_uncertain_write() {
    if isolated("recovery_preserves_never_sent_tail_and_does_not_replay_an_uncertain_write") {
        return;
    }
    use base64::{Engine, engine::general_purpose::STANDARD};
    for uncertain in [false, true] {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::new(endpoint).await;
        rig.media.audio_bytes = vec![1, 2];
        rig.media.audio_unconfirmed = uncertain;
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            for _ in 0..2 {
                socket.next().await.unwrap().unwrap();
            }
            let audio = socket.next().await.unwrap().unwrap();
            let audio: serde_json::Value = serde_json::from_str(audio.to_text().unwrap()).unwrap();
            let bytes = STANDARD
                .decode(audio["realtimeInput"]["audio"]["data"].as_str().unwrap())
                .unwrap();
            let mut expected = if uncertain { Vec::new() } else { vec![1, 2] };
            expected.extend([4, 3].repeat(160));
            assert_eq!(bytes, expected);
            assert!(socket.next().await.unwrap().unwrap().is_close());
        });
        assert!(rig.run().await.is_continue());
        assert_eq!(
            rig.media.audio_bytes,
            if uncertain { Vec::new() } else { vec![1, 2] }
        );
        let input = CandidateInput::Frame(AudioFrame {
            data: vec![0x0304; 160].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 160,
        });
        on_candidate_input(
            &mut rig.turn.context(
                &mut rig.output,
                &mut rig.gemini,
                &mut rig.board,
                &mut rig.media,
            ),
            input,
            1500,
        )
        .await;
        assert!(!rig.media.audio_unconfirmed);
        assert!(rig.media.audio_bytes.is_empty());
        rig.finish().await;
        server.await.unwrap();
    }
}

#[tokio::test]
async fn buffered_audio_keeps_its_order_and_yield_without_refreshing_live_voice() {
    if isolated("buffered_audio_keeps_its_order_and_yield_without_refreshing_live_voice") {
        return;
    }
    use base64::{Engine, engine::general_purpose::STANDARD};
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let arrived = Instant::now() - Duration::from_secs(5);
    rig.media.voiced_at = Some(arrived);
    rig.media.recovery_audio.push(
        AudioFrame {
            data: vec![7; 80].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 80,
        },
        arrived,
    );
    rig.media.recovery_audio.yielded(arrived);
    rig.media.recovery_audio.push(
        AudioFrame {
            data: vec![9; 160].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 160,
        },
        arrived,
    );
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        for _ in 0..2 {
            socket.next().await.unwrap().unwrap();
        }
        let mut values = Vec::new();
        for _ in 0..3 {
            let frame = socket.next().await.unwrap().unwrap();
            values
                .push(serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap());
        }
        assert_eq!(
            STANDARD
                .decode(
                    values[0]["realtimeInput"]["audio"]["data"]
                        .as_str()
                        .unwrap()
                )
                .unwrap(),
            [7, 0].repeat(80)
        );
        assert_eq!(values[1]["realtimeInput"]["audioStreamEnd"], true);
        assert_eq!(
            STANDARD
                .decode(
                    values[2]["realtimeInput"]["audio"]["data"]
                        .as_str()
                        .unwrap()
                )
                .unwrap(),
            [9, 0].repeat(160)
        );
        assert!(socket.next().await.unwrap().unwrap().is_close());
    });
    assert!(rig.run().await.is_continue());
    for _ in 0..3 {
        let input = next_candidate_input(&mut rig.media.audio, &mut rig.media.recovery_audio).await;
        on_candidate_input(
            &mut rig.turn.context(
                &mut rig.output,
                &mut rig.gemini,
                &mut rig.board,
                &mut rig.media,
            ),
            input,
            1500,
        )
        .await;
    }
    assert_eq!(rig.media.voiced_at, Some(arrived));
    assert!(rig.media.recovery_audio.is_empty());
    rig.finish().await;
    server.await.unwrap();
}

async fn end_without_advancing_the_wait(
    tx: &mpsc::UnboundedSender<Packet>,
    observed: &mut mpsc::UnboundedReceiver<Terminal>,
) {
    tokio::time::pause();
    let frozen = tokio::time::Instant::now();
    control(tx, serde_json::json!({"type":"end_interview"}));
    let mut terminal = None;
    for _ in 0..1024 {
        if let Ok(value) = observed.try_recv() {
            terminal = Some(value);
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        tokio::time::Instant::now(),
        frozen,
        "the operation timeout must remain unspent"
    );
    tokio::time::resume();
    assert!(
        matches!(terminal, Some(Terminal::End { .. })),
        "end was not handled while the operation was pending"
    );
}

#[tokio::test]
async fn end_cancels_a_close_while_its_writer_and_timeout_remain_blocked() {
    if isolated("end_cancels_a_close_while_its_writer_and_timeout_remain_blocked") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
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
        let mut socket = socket.into_inner();
        reading.await.unwrap();
        let mut bytes = [0; 65536];
        loop {
            match socket.read(&mut bytes).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                other => panic!("closing the retired TCP stream failed: {other:?}"),
            }
        }
        Vec::new()
    });
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
    rig.gemini = crate::livekit::tests::fixture_session_at(&endpoint, &rig.keys, &boot, None)
        .await
        .unwrap();
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
    let (phase, mut phases) = mpsc::unbounded_channel();
    rig.loops.recovery_phase = Some(phase);
    let (probe, mut observed) = mpsc::unbounded_channel();
    rig.loops.recovery_terminal = Some(probe);
    let tx = rig.tx.clone();
    let stimulus = async {
        while phases.recv().await != Some("close") {}
        end_without_advancing_the_wait(&tx, &mut observed).await;
        read.send(()).unwrap();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert_eq!(rig.turn.activity.live_socket, 1);
    rig.finish().await;
}

#[test]
fn trimming_audio_carries_the_boundary_before_retained_later_frames() {
    let at = Instant::now();
    let frame = |value| AudioFrame {
        data: vec![value; 160].into(),
        sample_rate: 16000,
        num_channels: 1,
        samples_per_channel: 160,
    };
    let mut ring = Ingress::default();
    ring.push(frame(7), at);
    ring.yielded(at);
    ring.push(frame(9), at + Duration::from_secs(5));
    assert!(matches!(
        ring.pop(at),
        Some(CandidateInput::BufferedFrame(_))
    ));
    assert!(matches!(
        ring.pop(at + INGRESS_AGE),
        Some(CandidateInput::Yield)
    ));
    assert!(
        matches!(ring.pop(at + INGRESS_AGE), Some(CandidateInput::BufferedFrame(frame)) if frame.data[0] == 9)
    );
    assert!(ring.is_empty());
}

#[tokio::test]
async fn a_release_during_a_notice_requests_a_reply_when_only_markers_are_buffered() {
    if isolated("a_release_during_a_notice_requests_a_reply_when_only_markers_are_buffered") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.owed_reply_on_resume =
        Some(Some("[SYSTEM EVENT] Answer the original question.".into()));
    rig.turn.state.announce_thinking();
    rig.turn.state.earlier_steps_pending.push("design");
    let old = rig.gemini.retire();
    let (mut fresh, server) = fake_recording_socket(None, 2).await;
    let (entered, release) = rig.recorder.hold_data("thinking_state", None);
    let tx = rig.tx.clone();
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
        let stimulus = async {
            entered.notified().await;
            control(
                &tx,
                serde_json::json!({"type":"thinking", "thinking":false}),
            );
            release.notify_one();
        };
        let (replied, ()) = tokio::join!(driver.drain(&mut fresh), stimulus);
        assert!(replied.unwrap());
        assert!(!driver.context.state.interviewer_silenced());
        assert!(!driver.ingress.has_audio());
        assert_eq!(driver.context.state.earlier_steps_named, ["design"]);
        assert!(driver.context.activity.owes_prompt());
        assert!(driver.context.state.owed_reply_on_resume.is_none());
        assert!(!driver.pending());
    }
    let messages = server.await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["clientContent"]["turnComplete"], false);
    assert_eq!(messages[1]["clientContent"]["turnComplete"], true);
    let body = messages[1].to_string();
    assert!(body.contains("original question"));
    assert_eq!(
        body.matches("Earlier step references carried with this request")
            .count(),
        1
    );
    fresh.shutdown().await.unwrap();
    rig.gemini = old;
    rig.finish().await;
}

/// `pending` is what keeps the drain loop going, so each kind of unfinished
/// work must make it true on its own, and a failure the driver already
/// reported must not keep it true.
#[tokio::test]
async fn pending_is_true_for_each_kind_of_unfinished_work_and_false_once_it_failed() {
    if isolated("pending_is_true_for_each_kind_of_unfinished_work_and_false_once_it_failed") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
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
    let mut context = rig.turn.context(
        &mut rig.output,
        &mut rig.gemini,
        &mut rig.board,
        &mut rig.media,
    );
    let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
    assert!(!driver.pending(), "nothing is waiting");

    // A queued action is work, unless a hold means it is not to be sent yet.
    driver.action = Some(Action {
        sequence: 0,
        text: "reply".into(),
        earlier: Vec::new(),
    });
    assert!(driver.pending());
    driver.context.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    assert!(!driver.pending(), "a held action waits for the release");
    driver.context.state.thinking_hold = crate::agent::ThinkingHold::default();
    driver.action = None;
    assert!(!driver.pending());

    // A Thinking notice is work until it has been tried and failed.
    driver.context.state.thinking_notice = Some(true);
    assert!(driver.pending());
    driver.failed_thinking = Some(true);
    assert!(!driver.pending(), "the same notice already failed");
    driver.failed_thinking = Some(false);
    assert!(driver.pending(), "a different notice has not been tried");
    driver.context.state.thinking_notice = None;
    assert!(!driver.pending(), "no notice, nothing to publish");
    driver.failed_thinking = None;

    // The checklist is work when the page has not been told it, and not once
    // that exact checklist failed to publish.
    driver.context.state.framework_published = vec!["design"];
    assert!(driver.pending());
    driver.failed_framework = Some(Vec::new());
    assert!(!driver.pending(), "this checklist already failed");
    driver.failed_framework = Some(vec!["design"]);
    assert!(driver.pending(), "a different checklist has not been tried");
    drop(driver);
    rig.finish().await;
}

/// What a packet may change in the driver is exactly what it names: a packet
/// that does nothing moves no revision, a Thinking hold the interviewer was
/// not holding the floor through owes nothing unheard, and a code edit that
/// only switches the language still queues the reply it asks for.
#[tokio::test]
async fn a_recovery_packet_changes_only_what_it_names() {
    if isolated("a_recovery_packet_changes_only_what_it_names") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
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
    let mut context = rig.turn.context(
        &mut rig.output,
        &mut rig.gemini,
        &mut rig.board,
        &mut rig.media,
    );
    let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
    let control = |driver: &mut Driver<'_, '_, '_>, body: &[u8]| {
        driver.packet(Some(TOPIC_CONTROL), Some("candidate"), body);
    };

    // A packet that names nothing moves no revision, and leaves a reply the
    // candidate was already waiting on exactly where it was.
    driver.context.activity.awaiting_reply_since = Some(Instant::now() - Duration::from_secs(10));
    let waiting = driver.context.activity.awaiting_reply_since;
    control(&mut driver, br#"{"type":"unknown"}"#);
    assert_eq!(driver.revision, 0);
    assert_eq!(driver.obligation_revision, 0);
    assert_eq!(driver.context.activity.awaiting_reply_since, waiting);
    driver.context.activity.awaiting_reply_since = None;

    // Taking the floor back with Thinking owes the unheard note only when the
    // interviewer held the floor, and records no reply debt of its own.
    assert_eq!(driver.context.activity.floor, Floor::Listening);
    control(&mut driver, br#"{"type":"thinking","thinking":true}"#);
    assert_eq!(
        driver.revision, 1,
        "a packet with an effect moves the revision"
    );
    assert!(!driver.context.state.thinking_unheard_reply);
    assert!(driver.context.state.owed_reply_on_resume.is_none());
    control(&mut driver, br#"{"type":"thinking","thinking":false}"#);
    driver.context.activity.mark_speaking();
    control(&mut driver, br#"{"type":"thinking","thinking":true}"#);
    assert!(driver.context.state.thinking_unheard_reply);

    // A language switch alone asks for a reply, which is an obligation change.
    driver.context.state.thinking_hold = crate::agent::ThinkingHold::default();
    let before = driver.obligation_revision;
    driver.packet(
        Some(crate::runtime::TOPIC_CODE_UPDATE),
        Some("candidate"),
        br#"{"code":"let x = 1;","language":"javascript"}"#,
    );
    assert!(driver.obligation_revision > before);

    // A switch of language with the same code, and an edit in the same
    // language, each move the revision: whatever the reducer calls a change is
    // one the driver counts.
    let code = driver.context.state.code.clone();
    for (code, language) in [(code.as_str(), "python"), ("let x = 2;", "python")] {
        let before = driver.revision;
        let body = serde_json::json!({"code": code, "language": language}).to_string();
        driver.packet(
            Some(crate::runtime::TOPIC_CODE_UPDATE),
            Some("candidate"),
            body.as_bytes(),
        );
        assert!(driver.revision > before, "{language} {code}");
    }
    drop(driver);
    rig.finish().await;
}

/// A Thinking notice that reached the page is done: nothing is left to
/// announce and nothing is recorded as failed, so the drain does not retry it.
#[tokio::test]
async fn a_published_thinking_notice_is_neither_retried_nor_recorded_as_failed() {
    if isolated("a_published_thinking_notice_is_neither_retried_nor_recorded_as_failed") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
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
    let mut context = rig.turn.context(
        &mut rig.output,
        &mut rig.gemini,
        &mut rig.board,
        &mut rig.media,
    );
    let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
    driver.context.state.thinking_notice = Some(true);
    driver.flush_notices().await.unwrap();
    assert!(driver.context.state.thinking_notice.is_none());
    assert!(driver.failed_thinking.is_none());
    assert!(!driver.pending());
    drop(driver);
    rig.finish().await;
}

/// Prefix speech finalized during a handoff can start a Thinking hold by
/// itself. When it does with a reply owed, the owed request moves into the
/// resume debt and the driver is told the hold changed; plain speech changes
/// nothing and says so.
#[tokio::test]
async fn prefix_speech_that_starts_a_hold_moves_an_owed_reply_into_the_resume_debt() {
    if isolated("prefix_speech_that_starts_a_hold_moves_an_owed_reply_into_the_resume_debt") {
        return;
    }
    for (owed, speech, changed) in [
        (true, "Wait, let me think", true),
        (false, "Wait, let me think", true),
        (true, "The answer is two sum", false),
    ] {
        let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
        rig.media.identity = Some("candidate".into());
        if owed {
            rig.turn
                .activity
                .owe_prompt(Instant::now(), Some("Answer the question.".into()));
        }
        let mut context = rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        );
        assert!(!context.state.interviewer_silenced());
        let told =
            session::apply_recovery_transcript(&rig.room, &mut context, speech, Instant::now());
        assert_eq!(told, changed, "owed={owed} speech={speech:?}");
        assert_eq!(context.state.interviewer_silenced(), changed);
        assert_eq!(
            context.state.owed_reply_on_resume.is_some(),
            owed && changed,
            "owed={owed} speech={speech:?}"
        );
        rig.finish().await;
    }
}

/// Contexts the model has not been told are kept up to a count and a byte
/// bound, the oldest dropped to make room, and what was left out is counted
/// once. The bound is inclusive: a summary of exactly the bound fits.
#[tokio::test]
async fn held_contexts_are_kept_within_a_count_and_a_byte_bound() {
    if isolated("held_contexts_are_kept_within_a_count_and_a_byte_bound") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
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
    let mut context = rig.turn.context(
        &mut rig.output,
        &mut rig.gemini,
        &mut rig.board,
        &mut rig.media,
    );
    let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
    let kept = |driver: &Driver<'_, '_, '_>| {
        (
            driver.held_contexts.len(),
            driver.held_bytes,
            driver.held_omitted,
            driver.held_omitted_total,
        )
    };

    // One of exactly the bound fits; one past it is left out and counted once.
    driver.retain_held_context("x".repeat(MAX_HELD_BYTES));
    assert_eq!(kept(&driver), (1, MAX_HELD_BYTES, 0, 0));
    driver.retain_held_context("y".repeat(MAX_HELD_BYTES + 1));
    assert_eq!(kept(&driver), (1, MAX_HELD_BYTES, 1, 1));

    // A modest summary is kept, and making room for it drops the oldest.
    driver.retain_held_context("z".repeat(2_000));
    assert_eq!(kept(&driver), (1, 2_000, 2, 2));
    assert_eq!(driver.held_contexts[0].len(), 2_000);

    // Two that together reach the bound exactly are both kept; one more byte
    // drops the first and only the first.
    driver.held_contexts.clear();
    (
        driver.held_bytes,
        driver.held_omitted,
        driver.held_omitted_total,
    ) = (0, 0, 0);
    driver.retain_held_context("a".repeat(40_000));
    driver.retain_held_context("b".repeat(MAX_HELD_BYTES - 40_000));
    assert_eq!(kept(&driver), (2, MAX_HELD_BYTES, 0, 0));
    driver.retain_held_context("c".into());
    assert_eq!(kept(&driver), (2, MAX_HELD_BYTES - 40_000 + 1, 1, 1));
    assert!(driver.held_contexts[0].starts_with('b'));

    // The count bound drops the oldest, one at a time.
    driver.held_contexts.clear();
    (
        driver.held_bytes,
        driver.held_omitted,
        driver.held_omitted_total,
    ) = (0, 0, 0);
    for n in 0..MAX_HELD_CONTEXTS {
        driver.retain_held_context(format!("{n}"));
    }
    assert_eq!(driver.held_contexts.len(), MAX_HELD_CONTEXTS);
    assert_eq!(driver.held_omitted, 0);
    driver.retain_held_context("new".into());
    assert_eq!(driver.held_contexts.len(), MAX_HELD_CONTEXTS);
    assert_eq!((driver.held_omitted, driver.held_omitted_total), (1, 1));
    assert_eq!(driver.held_contexts[0], "1");
    assert_eq!(
        driver.held_bytes,
        driver.held_contexts.iter().map(String::len).sum::<usize>()
    );
    drop(driver);
    rig.finish().await;
}

/// A socket that stayed up past the healthy age resets the restart count, so
/// a provider that closes idle sockets after a minute never exhausts the
/// budget. One the watchdog found silent is not healthy because it stayed
/// connected: the same age must not reset the count, or a provider that
/// swallows replies a minute in would never let the budget end the interview.
#[tokio::test(flavor = "current_thread")]
async fn only_a_socket_the_watchdog_did_not_find_silent_resets_the_restart_count() {
    if isolated("only_a_socket_the_watchdog_did_not_find_silent_resets_the_restart_count") {
        return;
    }
    for (trigger, expected) in [(Trigger::GoAway, 1), (Trigger::Watchdog, 4)] {
        let mut rig = Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), None).await;
        let (fresh, endpoint) = listener().await;
        rig.endpoint = endpoint;
        rig.gemini
            .age_for_test(crate::livekit::HEALTHY_GEMINI_SOCKET + Duration::from_secs(1));
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.loops.restarts = 3;
        let server = tokio::spawn(scripted_fresh_gemini(fresh, vec![FreshStep::Ready]));
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
            let flow = tokio::time::timeout(
                Duration::from_secs(10),
                replace_gemini_session(
                    &rig.room,
                    &mut context,
                    interview,
                    &mut rig.loops,
                    trigger,
                    None,
                ),
            )
            .await
            .expect("the replacement is bounded")
            .unwrap();
            assert!(flow.is_continue());
        }
        assert_eq!(rig.loops.restarts, expected, "{trigger:?}");
        rig.finish().await;
        server.await.unwrap();
    }
}

#[tokio::test]
async fn reference_debt_survives_noop_packets_and_action_supersession() {
    if isolated("reference_debt_survives_noop_packets_and_action_supersession") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let old = rig.gemini.retire();
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
        driver.context.state.earlier_steps_pending.push("design");
        let noop = serde_json::to_vec(&serde_json::json!({"type":"unknown"})).unwrap();
        driver.packet(Some(TOPIC_CONTROL), Some("candidate"), &noop);
        assert_eq!(driver.context.state.earlier_steps_pending, ["design"]);
        for sender in [None, Some("bystander")] {
            driver.packet(Some(TOPIC_CONTROL), sender, br#"{"type":"end_interview"}"#);
            assert!(driver.terminal.is_none());
        }
        driver.context.state.owed_reply_on_resume = Some(Some("raw-platform-request".into()));
        for step in ["test", "analyze"] {
            driver.packet(
                Some(TOPIC_CONTROL),
                Some("candidate"),
                br#"{"type":"thinking","thinking":true}"#,
            );
            driver.context.state.earlier_steps_pending.push(step);
            driver.packet(
                Some(TOPIC_CONTROL),
                Some("candidate"),
                br#"{"type":"thinking","thinking":false}"#,
            );
        }
        let action = driver.action.as_ref().unwrap();
        for step in ["design", "test", "analyze"] {
            assert!(action.earlier.contains(&step));
            assert!(action.text.contains(step));
        }
        assert_eq!(
            driver.context.state.owed_reply_on_resume,
            Some(Some("raw-platform-request".into()))
        );
        assert!(driver.context.state.earlier_steps_named.is_empty());
        let mut closed = driver.context.gemini.retire();
        let (ok, spoke) = driver.flush_actions(&mut closed).await.unwrap();
        assert!(!ok && !spoke);
        for step in ["design", "test", "analyze"] {
            assert!(driver.context.state.earlier_steps_pending.contains(&step));
        }
        assert!(driver.context.state.earlier_steps_named.is_empty());
        driver.held_contexts.push_back("unsent held summary".into());
        driver.held_bytes = "unsent held summary".len();
        driver.round = Some("skipped");
        driver.loops.pending_round_notice = Some("started");
        assert!(!driver.drain(&mut closed).await.unwrap());
        assert!(
            driver.loops.pending_round_notice.is_none(),
            "the newer successful notice supersedes the older failed snapshot"
        );
        let rounds = rig
            .recorder
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                room_link::Sent::Data { message, .. } if message["type"] == "round_state" => {
                    Some(message)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rounds.len(),
            1,
            "failed model context must not skip the room notice"
        );
        assert_eq!(rounds[0]["status"], "skipped");
        driver.round = Some("started");
        driver.loops.pending_round_notice = Some("skipped");
        driver.evictions.push_back("duplicate-agent-fixture".into());
        driver.preserve_room_work();
        assert_eq!(driver.loops.pending_round_notice, Some("started"));
        assert!(driver.round.is_none());
        assert!(driver.evictions.is_empty());
        assert_eq!(driver.evictions_omitted, 1);
    }
    rig.gemini = old;
    rig.finish().await;
}

#[test]
fn expired_audio_cannot_defer_a_reply_and_empty_boundaries_are_coalesced() {
    let now = Instant::now();
    let mut ring = Ingress::default();
    ring.yielded(now - INGRESS_AGE);
    ring.push(
        AudioFrame {
            data: vec![7; 160].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 160,
        },
        now - INGRESS_AGE,
    );
    assert!(!ring.has_audio(), "expired bytes cannot defer the action");
    ring.yielded(now);
    assert!(matches!(ring.pop(now), Some(CandidateInput::Yield)));
    assert!(ring.pop(now).is_none(), "one empty boundary is sufficient");
    assert_eq!(ring.dropped_bytes, 320);
}
#[tokio::test]
async fn a_test_summary_received_during_a_notice_is_drained_before_commit() {
    if isolated("a_test_summary_received_during_a_notice_is_drained_before_commit") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.code = "def solve(): return 7".into();
    rig.turn.state.language = "python".into();
    rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
    rig.turn.state.announce_thinking();
    let old = rig.gemini.retire();
    let (mut fresh, server) = fake_recording_socket(None, 1).await;
    let (entered, release) = rig.recorder.hold_data("thinking_state", None);
    let tx = rig.tx.clone();
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
        let stimulus = async {
            entered.notified().await;
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
            release.notify_one();
        };
        let (replied, ()) = tokio::join!(driver.drain(&mut fresh), stimulus);
        assert!(!replied.unwrap());
        assert!(driver.context.state.interviewer_silenced());
        assert!(!driver.ingress.has_audio());
        assert_eq!(driver.context.state.test_runs, 1);
        assert!(!driver.pending());
    }
    let messages = server.await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["clientContent"]["turnComplete"], false);
    let body = messages[0].to_string();
    assert!(
        body.contains("0/1") || body.contains("0 of 1"),
        "the late test summary must reach the wire: {body}"
    );
    fresh.shutdown().await.unwrap();
    rig.gemini = old;
    rig.finish().await;
}

#[tokio::test]
async fn the_last_correction_keeps_its_reply_and_preserves_newer_editor_debt() {
    if isolated("the_last_correction_keeps_its_reply_and_preserves_newer_editor_debt") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.turn.state.example_drawing_enabled = true;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.owed_reply_on_resume = Some(Some("raw-question-marker".into()));
    rig.turn
        .activity
        .owe_prompt(Instant::now(), Some("raw-question-marker".into()));
    let mut signals = Vec::new();
    for _ in 0..2 {
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        rig.loops.recovery_writes.push_back(WriteBarrier {
            phase: "recovery_brief",
            entered: std::sync::Arc::clone(&entered),
            release: std::sync::Arc::clone(&release),
        });
        signals.push((entered, release));
    }
    let tx = rig.tx.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut messages = Vec::new();
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if frame.is_close() {
                break;
            }
            messages
                .push(serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap());
        }
        messages
    });
    let stimulus = async {
        signals[0].0.notified().await;
        control(
            &tx,
            serde_json::json!({"type":"thinking", "thinking":false}),
        );
        signals[0].1.notify_one();
        signals[1].0.notified().await;
        tx.send(Packet {
            sender: Some("candidate".into()),
            topic: Some(crate::runtime::TOPIC_CODE_UPDATE.into()),
            payload: serde_json::to_vec(
                &serde_json::json!({"code":"fn late_editor_marker() {}", "language":"rust"}),
            )
            .unwrap(),
        })
        .unwrap();
        control(
            &tx,
            serde_json::json!({"type":"yield_turn", "surface":"example"}),
        );
        signals[1].1.notify_one();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_continue());
    assert!(
        !rig.turn.activity.socket.discarding_output,
        "the final correction has no following correction to discard for"
    );
    assert!(rig.turn.state.code.contains("late_editor_marker"));
    assert!(
        !rig.turn.state.code_shown.contains("late_editor_marker"),
        "an older prepared briefing cannot mark a newer editor delivered"
    );
    rig.finish().await;
    let messages = server.await.unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.to_string().contains("paused their Example drawing")),
        "the new request received during the last briefing must also be sent"
    );
    assert!(
        messages.len() >= 4,
        "both prepared briefs must reach the wire"
    );
    assert!(
        messages
            .iter()
            .any(|message| message["clientContent"]["turnComplete"] == true)
    );
}

#[test]
fn a_yield_at_frame_capacity_drops_only_the_oldest_audio() {
    let now = Instant::now();
    let mut ring = Ingress::default();
    for value in 0..INGRESS_FRAMES {
        ring.push(
            AudioFrame {
                data: vec![i16::try_from(value).unwrap()].into(),
                sample_rate: 16000,
                num_channels: 1,
                samples_per_channel: 1,
            },
            now,
        );
    }
    ring.yielded(now);
    assert_eq!(ring.frames.len(), INGRESS_FRAMES);
    assert_eq!(ring.dropped_bytes, 2);
    assert_eq!(ring.bytes, (INGRESS_FRAMES - 1) * 2);
    for value in 1..INGRESS_FRAMES {
        assert!(matches!(
            ring.pop(now),
            Some(CandidateInput::BufferedFrame(frame)) if frame.data[0] == i16::try_from(value).unwrap()
        ));
    }
    assert!(matches!(ring.pop(now), Some(CandidateInput::Yield)));
    assert!(ring.pop(now).is_none());
}
