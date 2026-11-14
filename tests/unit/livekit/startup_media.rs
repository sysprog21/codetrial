use super::*;

struct MeasuredStartupInput {
    receiver: mpsc::UnboundedReceiver<StartupInput>,
    yielded: Arc<AtomicUsize>,
}

impl StartupInputs for MeasuredStartupInput {
    async fn next(&mut self, _media: &mut CandidateMedia) -> StartupInput {
        let input = self.receiver.recv().await.unwrap();
        self.yielded.fetch_add(1, Ordering::SeqCst);
        input
    }
}

#[tokio::test]
async fn startup_success_after_a_delayed_microphone_write_keeps_the_receive_time() {
    use ::livekit::webrtc::audio_frame::AudioFrame;
    use futures_util::FutureExt;

    let config = startup_config("60");
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpSocket::new_v4().unwrap();
    listener.set_recv_buffer_size(4096).unwrap();
    listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = listener.listen(1).unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let (opened, opening) = tokio::sync::oneshot::channel();
    let (answer, answered) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        loop {
            let message = startup_fixture_io("peer opening request", socket.next())
                .await
                .unwrap()
                .unwrap();
            if let Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["clientContent"]["turnComplete"] == true {
                    break;
                }
            }
        }
        opened.send(()).unwrap();
        startup_fixture_io("peer output release", answered)
            .await
            .unwrap();
        socket.send(Message::Text(r#"{"usageMetadata":{"promptTokenCount":7,"responseTokenCount":3},"serverContent":{"inputTranscription":{"text":"candidate"},"modelTurn":{"parts":[{"text":"Hello"}]},"turnComplete":true}}"#.into())).await.unwrap();

        // Keep the write pending even after the independent reader answers.
        // Releasing the peer's reads must complete it successfully, not invoke
        // either the write-error rescue or the outer startup deadline.
        startup_fixture_io("peer read release", released)
            .await
            .unwrap();
        let mut generations = 1;
        let mut audio_bytes = 0;
        while let Some(Ok(message)) = socket.next().await {
            match message {
                Message::Close(_) => break,
                Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
                Message::Text(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value["clientContent"]["turnComplete"] == true
                        || value["realtimeInput"]["text"].is_string()
                        || value["toolResponse"].is_object()
                    {
                        generations += 1;
                    }
                    if let Some(data) = value["realtimeInput"]["audio"]["data"].as_str() {
                        use base64::Engine;
                        audio_bytes += base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .unwrap()
                            .len();
                    }
                }
                _ => {}
            }
        }
        assert!(listener.accept().now_or_never().is_none());
        (generations, audio_bytes)
    });
    let server = StartupPeer(server);
    GeminiLiveSession::constrain_next_startup_test_socket();
    let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        .await
        .unwrap();
    let (observed_usage, reader) = gemini.startup_reader_observer();
    let mut state = startup_state_for(&config);
    let greeting = with_timer(&state, "opening".into());
    let mut media = CandidateMedia::new();
    let mut deferred = Vec::new();
    let mut room_closed = false;
    let (tx, receiver) = mpsc::unbounded_channel();
    let yielded = Arc::new(AtomicUsize::new(0));
    let mut input = MeasuredStartupInput {
        receiver,
        yielded: std::sync::Arc::clone(&yielded),
    };
    let requested_at = Instant::now();
    let limit = Duration::from_secs(10);
    let (ready_at, released_at, finished_at, blocked) = {
        let mut context = StartupContext {
            media: &mut media,
            candidate_identity: "candidate",
            candidate_video: false,
            deferred_events: &mut deferred,
            room_closed: &mut room_closed,
        };
        let probe = crate::livekit::readiness::probe_live_output(
            &mut gemini,
            &mut state,
            &greeting,
            limit,
            &mut context,
            &mut input,
        );
        tokio::pin!(probe);
        tokio::select! {
            result = &mut probe => panic!("setup is not readiness: {result:?}"),
            result = opening => result.unwrap(),
        }
        for value in 0..200 {
            tx.send(StartupInput::Audio(Some(AudioFrame {
                data: vec![value; 160].into(),
                sample_rate: 16_000,
                num_channels: 1,
                samples_per_channel: 160,
            })))
            .unwrap();
        }
        tokio::select! {
            result = &mut probe => panic!("no model output yet: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        let blocked = yielded.load(Ordering::SeqCst);
        assert!((1..200).contains(&blocked), "a real write is pending");
        answer.send(()).unwrap();
        tokio::select! {
            result = &mut probe => panic!("the write still blocks output consumption: {result:?}"),
            _ = tokio::time::timeout(Duration::from_secs(1), async {
                while observed_usage() == 0 {
                    tokio::task::yield_now().await;
                }
            }) => assert_eq!(observed_usage(), 1),
        }
        tokio::select! {
            result = &mut probe => panic!("decoded output must wait for the write: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        assert_eq!(yielded.load(Ordering::SeqCst), blocked);
        let released_at = Instant::now();
        release.send(()).unwrap();
        let ready_at = tokio::time::timeout(Duration::from_secs(3), &mut probe)
            .await
            .unwrap()
            .unwrap();
        (ready_at, released_at, Instant::now(), blocked)
    };
    assert!(
        finished_at < requested_at + limit,
        "no outer-timeout rescue"
    );
    assert!(
        !reader.is_finished(),
        "no write-error rescue aborted the reader"
    );
    assert!(!gemini.audio_write_queued());
    assert!(media.audio_bytes.is_empty(), "the write succeeded");
    assert!(!room_closed);
    let mut received_at = None;
    for expected in [
        GeminiEvent::InputTranscript("candidate".into()),
        GeminiEvent::Text("Hello".into()),
        GeminiEvent::UsageRecorded,
        GeminiEvent::TurnComplete,
    ] {
        assert_eq!(gemini.next_event().await, Some(expected));
        let at = gemini.event_received_at().unwrap();
        assert_eq!(*received_at.get_or_insert(at), at);
    }
    let received_at = received_at.unwrap();
    assert!(received_at >= requested_at);
    assert!(released_at.duration_since(received_at) >= Duration::from_millis(100));
    let usage = gemini.take_usage().unwrap();
    assert_eq!((usage.prompt, usage.response), (7, 3));
    assert!(gemini.drain_usage().is_empty());
    assert_eq!(gemini.startup_test_queue_len(), 0);
    gemini.shutdown().await.unwrap();
    assert!(reader.is_finished());
    let (generations, audio_bytes) = startup_fixture_io("audio peer close", server)
        .await
        .unwrap();
    assert_eq!(generations, 1);
    assert_eq!(audio_bytes, yielded.load(Ordering::SeqCst) * 320);
    assert!(yielded.load(Ordering::SeqCst) >= blocked);
    eprintln!(
        "normal startup: received={:?}, released={:?}, finished={:?}, ready={:?}, frames={blocked}",
        received_at.duration_since(requested_at),
        released_at.duration_since(requested_at),
        finished_at.duration_since(requested_at),
        ready_at.duration_since(requested_at),
    );
    assert_eq!(
        ready_at, received_at,
        "normal readiness must retain receive time"
    );

    // Apply the probe result as open_session does. The production clock and
    // readiness age must already include the time spent waiting on the write.
    state.started_at = ready_at;
    let activity = ready_runtime_activity(
        &config,
        boot.room_name,
        ready_at,
        requested_at,
        7,
        &greeting,
    );
    assert_eq!(activity.buffered_startup_output_at, Some(received_at));
    assert!(!activity.reply_timed_out(finished_at, false));
    let age_before = crate::agent::elapsed_ms(&state);
    let ready = startup_ready_notice(state.started_at, activity.startup_ready_at);
    let age_after = crate::agent::elapsed_ms(&state);
    assert!(age_before >= 100);
    assert!((age_before..=age_after).contains(&ready["elapsedMs"].as_u64().unwrap()));
}

pub(super) async fn startup_deadline_backpressure(
    limit: Duration,
    answer_at: Option<Duration>,
    output: bool,
    backpressure: bool,
) {
    startup_deadline_reader_pause(limit, answer_at, output, backpressure, false).await;
}

#[tokio::test]
async fn startup_outer_deadline_observes_decoded_output_before_enqueue() {
    startup_deadline_reader_pause(
        Duration::from_secs(1),
        Some(Duration::from_millis(900)),
        true,
        true,
        true,
    )
    .await;
}

#[tokio::test]
async fn startup_outer_deadline_rejects_late_decoded_output_before_enqueue() {
    startup_deadline_reader_pause(
        Duration::from_secs(1),
        Some(Duration::from_millis(1100)),
        true,
        true,
        true,
    )
    .await;
}

async fn startup_deadline_reader_pause(
    limit: Duration,
    answer_at: Option<Duration>,
    output: bool,
    backpressure: bool,
    pause_before_enqueue: bool,
) {
    use ::livekit::webrtc::audio_frame::AudioFrame;
    use futures_util::FutureExt;

    let config = startup_config("60");
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpSocket::new_v4().unwrap();
    listener.set_recv_buffer_size(4096).unwrap();
    listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = listener.listen(1).unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let (opened, opening) = tokio::sync::oneshot::channel();
    let (answer, answered) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut generations = 0;
        loop {
            let message = startup_fixture_io("deadline peer opening request", socket.next())
                .await
                .unwrap()
                .unwrap();
            if let Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["clientContent"]["turnComplete"] == true {
                    generations += 1;
                    break;
                }
            }
        }
        opened.send(()).unwrap();
        if tokio::time::timeout(limit + Duration::from_secs(5), answered)
            .await
            .expect("deadline peer output release")
            .unwrap()
        {
            // Setup and protocol keepalives must not establish readiness
            // either.
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            socket
                .send(Message::Pong(Default::default()))
                .await
                .unwrap();
            let model = if output {
                r#""modelTurn":{"parts":[{"text":"Hello"}]},"#
            } else {
                ""
            };
            let interrupted = !pause_before_enqueue;
            socket.send(Message::Text(format!(r#"{{"usageMetadata":{{"promptTokenCount":7,"responseTokenCount":3}},"serverContent":{{{model}"inputTranscription":{{"text":"candidate"}},"interrupted":{interrupted},"turnComplete":true}}}}"#).into())).await.unwrap();
        }

        // The test intentionally keeps reads stopped across the outer probe
        // limit and the explicit six-second orphaned-write observation below.
        // Neither phase may turn a missing handshake into an unbounded wait.
        tokio::time::timeout(
            limit + Duration::from_secs(6) + Duration::from_secs(5),
            released,
        )
        .await
        .expect("deadline peer read release")
        .unwrap();
        let mut microphone = Vec::new();
        while let Some(Ok(message)) = socket.next().await {
            match message {
                Message::Close(_) => break,
                Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
                Message::Text(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value["clientContent"]["turnComplete"] == true
                        || value["realtimeInput"]["text"].is_string()
                        || value["toolResponse"].is_object()
                    {
                        generations += 1;
                    }
                    if let Some(data) = value["realtimeInput"]["audio"]["data"].as_str() {
                        use base64::Engine;
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .unwrap();
                        let (frames, remainder) = bytes.as_chunks::<320>();
                        assert!(remainder.is_empty());
                        for frame in frames {
                            microphone.push(i16::from_le_bytes([frame[0], frame[1]]));
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(
            listener.accept().now_or_never().is_none(),
            "no replacement socket"
        );
        (generations, microphone)
    });
    let server = StartupPeer(server);
    GeminiLiveSession::constrain_next_startup_test_socket();
    let (mut decoded, resume) = if pause_before_enqueue {
        let (decoded, resume) = GeminiLiveSession::pause_next_startup_test_output();
        (Some(decoded), Some(resume))
    } else {
        (None, None)
    };
    let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        .await
        .unwrap();
    let (observed_usage, reader) = gemini.startup_reader_observer();
    let mut state = startup_state_for(&config);
    let greeting = with_timer(&state, "opening".into());
    let mut media = CandidateMedia::new();
    let mut deferred = Vec::new();
    let mut room_closed = false;
    let (tx, receiver) = mpsc::unbounded_channel();
    let yielded = Arc::new(AtomicUsize::new(0));
    let mut input = MeasuredStartupInput {
        receiver,
        yielded: std::sync::Arc::clone(&yielded),
    };
    let began = tokio::time::Instant::now();
    let deadline = began + limit;
    let late = answer_at.is_some_and(|at| at >= limit);
    let mut decoded_at = None;
    let mut context = StartupContext {
        media: &mut media,
        candidate_identity: "candidate",
        candidate_video: false,
        deferred_events: &mut deferred,
        room_closed: &mut room_closed,
    };
    let (result, finished_at) = {
        let probe = crate::livekit::readiness::probe_live_output(
            &mut gemini,
            &mut state,
            &greeting,
            limit,
            &mut context,
            &mut input,
        );
        tokio::pin!(probe);
        tokio::select! {
            _ = &mut probe => panic!("startup completed before the opening was observed"),
            result = opening => result.unwrap(),
        }
        tokio::select! {
            _ = &mut probe => panic!("startup completed before microphone backpressure"),
            _ = tokio::time::sleep_until(began + limit * 8 / 10) => {}
        }

        // At most two seconds of ordinary 10 ms mono PCM frames. Tiny socket
        // buffers, not a multi-megabyte synthetic audio frame, force
        // backpressure.
        for value in 0..if backpressure { 200 } else { 0 } {
            tx.send(StartupInput::Audio(Some(AudioFrame {
                data: vec![value; 160].into(),
                sample_rate: 16_000,
                num_channels: 1,
                samples_per_channel: 160,
            })))
            .unwrap();
        }
        tokio::select! {
            _ = &mut probe => panic!("startup completed before the answer"),
            _ = tokio::time::sleep_until(began + limit * 9 / 10) => {}
        }
        let blocked = yielded.load(Ordering::SeqCst);
        assert!(
            if backpressure {
                (1..200).contains(&blocked)
            } else {
                blocked == 0
            },
            "normal frames really reached a pending write: {blocked}"
        );
        if late {
            // Delay polling the probe until late output is already in its
            // queue. A rescue using now(), or no cutoff, would incorrectly
            // accept it.
            tokio::time::sleep_until(began + answer_at.unwrap()).await;
        }
        answer.send(answer_at.is_some()).unwrap();
        if answer_at.is_some() {
            let observed = async {
                if let Some(decoded) = decoded.as_mut() {
                    let (at, event) = decoded.await.unwrap();
                    assert_eq!(event, GeminiEvent::Text("Hello".into()));
                    decoded_at = Some(at);
                } else {
                    let safety = tokio::time::Instant::now() + Duration::from_secs(1);
                    while observed_usage() == 0 {
                        assert!(tokio::time::Instant::now() < safety);
                        tokio::task::yield_now().await;
                    }
                }
            };
            if late {
                observed.await;
            } else {
                tokio::select! {
                    _ = &mut probe => panic!("pending microphone write must prevent normal output polling"),
                    _ = observed => {}
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "reader decoded before expiry"
                );
            }
            assert_eq!(observed_usage(), 1, "the real reader decoded the response");
        }
        assert_eq!(
            yielded.load(Ordering::SeqCst),
            blocked,
            "forwarding is still backpressured"
        );
        (probe.await, tokio::time::Instant::now())
    };
    if let Some(at) = decoded_at {
        assert_eq!(at < deadline.into_std(), !late);
        assert_eq!(
            gemini.startup_test_queue_len(),
            0,
            "the model event is still held before queue insertion"
        );
    }
    assert_eq!(
        result.is_ok(),
        output && !late,
        "only pre-deadline reader output can rescue startup: {result:?}"
    );
    assert!(finished_at >= deadline);
    assert!(finished_at < began + limit * 8 / 10 + Duration::from_secs(5));
    assert!(
        gemini.audio_write_queued() == backpressure,
        "the pending microphone flush was cancelled"
    );
    assert!(
        !reader.is_finished(),
        "outer cancellation did not run the write timeout"
    );
    if pause_before_enqueue && !late {
        assert_eq!(
            gemini.take_buffered_startup_speech(),
            vec![GeminiEvent::InputTranscript("candidate".into())]
        );
        assert!(gemini.take_buffered_startup_speech().is_empty());
    }
    if let Some(resume) = resume {
        resume.send(()).unwrap();
    }
    let mut received_output_at = None;
    if answer_at.is_some() {
        let mut expected = Vec::new();
        if output {
            expected.push(GeminiEvent::Text("Hello".into()));
        }
        if pause_before_enqueue && late {
            expected.insert(0, GeminiEvent::InputTranscript("candidate".into()));
        }
        expected.push(GeminiEvent::UsageRecorded);
        if !pause_before_enqueue {
            expected.push(GeminiEvent::Interrupted);
        }
        expected.push(GeminiEvent::TurnComplete);
        if !pause_before_enqueue {
            expected.push(GeminiEvent::InputTranscript("candidate".into()));
        }
        let mut frame_time = None;
        for event in expected {
            assert_eq!(
                startup_fixture_io("deadline event replay", gemini.next_event())
                    .await
                    .as_ref(),
                Some(&event)
            );
            let received_at = gemini.event_received_at().unwrap();
            assert_eq!(
                *frame_time.get_or_insert(received_at),
                received_at,
                "one decoded frame retains its receive time throughout replay"
            );
            assert_eq!(received_at < deadline.into_std(), !late);
            if matches!(event, GeminiEvent::Text(_)) {
                received_output_at = Some(received_at);
            }
            if matches!(event, GeminiEvent::UsageRecorded) {
                let usage = gemini.take_usage().unwrap();
                assert_eq!((usage.prompt, usage.response), (7, 3));
            }
        }
    }
    assert!(gemini.drain_usage().is_empty());
    if pause_before_enqueue {
        assert_eq!(received_output_at, decoded_at);
    }

    // Only Tokio's timer needs advancing here: no new receive timestamp is
    // taken. An orphaned five-second write would abort the reader by now.
    if output && !late {
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(6)).await;
        tokio::task::yield_now().await;
        assert!(
            !reader.is_finished(),
            "no detached write timeout survived cancellation"
        );
        tokio::time::resume();
    }
    release.send(()).unwrap();
    if result.is_ok() {
        assert!(
            media.audio_bytes.is_empty(),
            "the sink already owns the cancelled audio message"
        );
        pump_audio(
            &mut media,
            &mut gemini,
            Some(AudioFrame {
                data: vec![200; 160].into(),
                sample_rate: 16_000,
                num_channels: 1,
                samples_per_channel: 160,
            }),
        )
        .await
        .unwrap();
    } else if backpressure {
        assert!(
            !media.audio_bytes.is_empty(),
            "failed startup retains its audio buffer"
        );
    }
    gemini.shutdown().await.unwrap();
    assert!(reader.is_finished(), "teardown must join the reader");
    let (generations, microphone) = startup_fixture_io("deadline peer close", server)
        .await
        .unwrap();
    assert_eq!(generations, 1);
    assert!(if backpressure {
        (1..200).contains(&microphone.len())
    } else {
        microphone.is_empty()
    });
    if result.is_ok() {
        assert_eq!(microphone.last(), Some(&200));
    }
    assert!(
        microphone.windows(2).all(|pair| pair[0] < pair[1]),
        "cancelled microphone samples must not be resent: {microphone:?}"
    );
    eprintln!(
        "startup deadline: limit={limit:?}, output_at={:?}, probe_finished={:?}, attempted_frames={}, wire_frames={}, result={result:?}",
        received_output_at.map(|at| at.duration_since(began.into_std())),
        finished_at.duration_since(began),
        yielded.load(Ordering::SeqCst),
        microphone.len()
    );
    if output && !late {
        assert_eq!(
            Some(result.expect("pre-deadline reader output must rescue the outer timeout")),
            received_output_at
        );
    } else {
        let error = result.expect_err("only pre-deadline model output can establish readiness");
        assert_eq!(
            crate::gemini::startup_failure_reason(error.as_ref()),
            "unavailable"
        );
    }
}

#[tokio::test]
async fn startup_microphone_write_failure_preserves_already_received_output() {
    use ::livekit::webrtc::audio_frame::AudioFrame;
    use futures_util::FutureExt;
    use tokio::io::AsyncReadExt;
    for output in [false, true] {
        let config = startup_config("60");
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let (finish, finished) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            loop {
                let opening = startup_fixture_io("write-failure opening request", socket.next())
                    .await
                    .unwrap()
                    .unwrap();
                if let Message::Text(opening) = opening {
                    let opening: serde_json::Value = serde_json::from_str(&opening).unwrap();
                    if opening["clientContent"]["turnComplete"] == true {
                        break;
                    }
                }
            }

            // Read only the start of the audio write, then stop draining it.
            // Output arrives while the production writer is backpressured,
            // before its finite write timeout aborts the reader.
            startup_fixture_io(
                "write-failure audio prefix",
                socket.get_mut().read_exact(&mut [0u8; 1]),
            )
            .await
            .unwrap();
            let content = if output {
                r#""modelTurn":{"parts":[{"text":"Hello"}]},"#
            } else {
                ""
            };
            socket.send(Message::Text(format!(r#"{{"usageMetadata":{{"promptTokenCount":7,"responseTokenCount":3}},"serverContent":{{{content}"inputTranscription":{{"text":"candidate"}},"turnComplete":true}}}}"#).into())).await.unwrap();
            tokio::time::timeout(Duration::from_secs(35), finished)
                .await
                .expect("write-failure client completion")
                .unwrap();
            assert!(
                listener.accept().now_or_never().is_none(),
                "startup must not open a replacement socket"
            );
        });
        let server = StartupPeer(server);
        let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
            .await
            .unwrap();
        let mut state = startup_state_for(&config);
        let greeting = with_timer(&state, "opening".into());
        let mut media = CandidateMedia::new();
        let mut deferred = Vec::new();
        let mut room_closed = false;
        let (tx, mut input) = mpsc::unbounded_channel();

        // Larger than the loopback socket buffers, with bounded memory, so the
        // write cannot finish merely because the kernel accepted one frame.
        tx.send(StartupInput::Audio(Some(AudioFrame {
            data: vec![1; 16 * 1024 * 1024].into(),
            sample_rate: 16_000,
            num_channels: 1,
            samples_per_channel: 16 * 1024 * 1024,
        })))
        .unwrap();
        let result = crate::livekit::readiness::probe_live_output(
            &mut gemini,
            &mut state,
            &greeting,
            Duration::from_secs(30),
            &mut StartupContext {
                media: &mut media,
                candidate_identity: "candidate",
                candidate_video: false,
                deferred_events: &mut deferred,
                room_closed: &mut room_closed,
            },
            &mut input,
        )
        .await;
        finish.send(()).unwrap();
        startup_fixture_io("write-failure peer close", server)
            .await
            .unwrap();
        let usage = gemini.drain_usage();
        assert_eq!(
            usage.len(),
            1,
            "the real reader decoded the response before the write failed"
        );
        assert_eq!((usage[0].prompt, usage[0].response), (7, 3));
        assert_eq!(
            result.is_ok(),
            output,
            "only actual model output can rescue the failed write: {result:?}"
        );
        assert!(
            !media.audio_bytes.is_empty(),
            "the microphone write really failed"
        );
        assert_eq!(
            gemini.next_event().await,
            Some(GeminiEvent::InputTranscript("candidate".into()))
        );
        if output {
            let ready_at = result.unwrap();
            assert_eq!(
                gemini.next_event().await,
                Some(GeminiEvent::Text("Hello".into()))
            );
            assert_eq!(
                gemini.event_received_at(),
                Some(ready_at),
                "readiness retains the output's receive time, not the failed write's end"
            );
        }
        assert_eq!(gemini.next_event().await, Some(GeminiEvent::UsageRecorded));
        assert_eq!(gemini.next_event().await, Some(GeminiEvent::TurnComplete));
        assert!(gemini.drain_usage().is_empty());
        gemini.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn startup_accepts_output_after_a_short_recovery_deadline() {
    let wire = startup_with_audio_until_output("5", 600).await;
    assert!(wire.pings <= 1, "pings={}", wire.pings);
}

#[tokio::test]
async fn startup_preserves_participant_departure_and_return_until_ready() {
    startup_with_audio_until_output("5", 2).await;
}

pub(super) async fn startup_with_audio_until_output(
    first_output: &str,
    frames: usize,
) -> StartupWire {
    use ::livekit::webrtc::audio_frame::AudioFrame;
    let expected: Vec<u8> = (0..frames)
        .flat_map(|i| (0..160).flat_map(move |_| (i as i16).to_le_bytes()))
        .collect();
    let (mut gemini, server, received) = startup_socket(Some(expected.len())).await;
    let config = startup_config(first_output);
    assert_eq!(
        config.gemini_first_output_s,
        first_output.parse::<u32>().unwrap()
    );
    let mut state = startup_state_for(&config);
    let greeting = with_timer(&state, "opening".into());
    let mut media = CandidateMedia::new();
    let mut deferred = Vec::new();
    let mut left = false;
    let (tx, mut rx) = mpsc::unbounded_channel();

    // LiveKit owns RemoteParticipant construction. Distinct opaque events prove
    // that classified departures and later room events retain their order.
    tx.send(StartupInput::ParticipantLeft(RoomEvent::Reconnecting))
        .unwrap();
    tx.send(StartupInput::Room(Some(RoomEvent::Reconnected)))
        .unwrap();
    let mut context = StartupContext {
        media: &mut media,
        candidate_identity: "candidate",
        candidate_video: false,
        deferred_events: &mut deferred,
        room_closed: &mut left,
    };
    tokio::time::pause();
    let began = tokio::time::Instant::now();
    let observed_after = Instant::now();
    let probe = crate::livekit::readiness::probe_live_output(
        &mut gemini,
        &mut state,
        &greeting,
        startup_output_limit(&config),
        &mut context,
        &mut rx,
    );
    let feed = async {
        let real_deadline = Instant::now() + Duration::from_secs(10);
        for batch in 0..frames / 2 {
            for offset in 0..2 {
                let value = (batch * 2 + offset) as i16;
                tx.send(StartupInput::Audio(Some(AudioFrame {
                    data: vec![value; 160].into(),
                    sample_rate: 16_000,
                    num_channels: 1,
                    samples_per_channel: 160,
                })))
                .unwrap();
            }

            // Keep the scheduler awake while the real loopback socket drains;
            // paused Tokio time must not jump to the deadline during kernel IO.
            while received.load(Ordering::SeqCst) < (batch + 1) * 640 {
                assert!(
                    Instant::now() < real_deadline,
                    "startup stopped pumping audio"
                );
                tokio::task::yield_now().await;
            }
            tokio::time::advance(Duration::from_millis(20)).await;
        }
        for _ in 0..1_000 {
            tokio::task::yield_now().await;
        }
    };
    let (ready, ()) = tokio::join!(probe, feed);
    ready.unwrap();
    assert!(began.elapsed() >= Duration::from_millis(frames as u64 * 10));
    assert!(!left);
    assert!(matches!(
        deferred.as_slice(),
        [(_, RoomEvent::Reconnecting), (_, RoomEvent::Reconnected)]
    ));
    assert!(observed_after <= deferred[0].0 && deferred[0].0 <= deferred[1].0);
    assert!(deferred[1].0 <= Instant::now());
    let mut presence = CandidatePresence::default();
    candidate_departed(&mut presence, deferred[0].0);
    assert_eq!(
        presence.deadline(CANDIDATE_ABSENCE_LIMIT),
        Some(deferred[0].0 + CANDIDATE_ABSENCE_LIMIT)
    );
    assert_eq!(
        gemini.next_event().await,
        Some(GeminiEvent::Text("Hello".into()))
    );
    gemini.shutdown().await.unwrap();
    tokio::time::resume();
    let wire = server.await.unwrap();
    assert_eq!(wire.audio, expected);
    assert_eq!(wire.generations, 1);
    wire
}

#[tokio::test]
async fn startup_room_loss_still_cancels_the_wait() {
    for event in [
        StartupInput::Room(None),
        StartupInput::Room(Some(RoomEvent::Disconnected {
            reason: DisconnectReason::ClientInitiated,
        })),
    ] {
        let (mut gemini, server, _) = startup_socket(None).await;
        let config = startup_config("60");
        let mut state = startup_state_for(&config);
        let greeting = with_timer(&state, "opening".into());
        let mut media = CandidateMedia::new();
        let mut deferred = Vec::new();
        let mut left = false;
        let (_tx, mut input) = mpsc::unbounded_channel();
        _tx.send(StartupInput::Room(Some(RoomEvent::Reconnecting)))
            .unwrap();
        _tx.send(event).unwrap();
        let before = tokio::time::Instant::now();
        let error = crate::livekit::readiness::probe_live_output(
            &mut gemini,
            &mut state,
            &greeting,
            startup_output_limit(&config),
            &mut StartupContext {
                media: &mut media,
                candidate_identity: "candidate",
                candidate_video: false,
                deferred_events: &mut deferred,
                room_closed: &mut left,
            },
            &mut input,
        )
        .await
        .unwrap_err();
        assert!(left);
        assert!(before.elapsed() < Duration::from_secs(1));
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::Interrupted
        );
        assert!(matches!(
            input.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        gemini.shutdown().await.unwrap();
        assert_eq!(server.await.unwrap().generations, 1);
    }
}

#[tokio::test]
async fn startup_uses_the_first_output_limit_despite_repeated_media_wakeups() {
    for (setting, seconds) in [("", 10), ("5", 10), ("10", 10), ("23", 23), ("120", 90)] {
        let (mut gemini, server, requested) = startup_socket(None).await;
        let config = startup_config(setting);
        assert_eq!(startup_output_limit(&config), Duration::from_secs(seconds));
        let mut state = startup_state_for(&config);
        let greeting = with_timer(&state, "opening".into());
        let mut media = CandidateMedia::new();
        let mut deferred = Vec::new();
        let mut left = false;
        let (tx, mut input) = mpsc::unbounded_channel();
        tx.send(StartupInput::ParticipantLeft(RoomEvent::Reconnecting))
            .unwrap();
        tx.send(StartupInput::Room(Some(RoomEvent::Reconnected)))
            .unwrap();
        let mut context = StartupContext {
            media: &mut media,
            candidate_identity: "candidate",
            candidate_video: false,
            deferred_events: &mut deferred,
            room_closed: &mut left,
        };
        let done = std::sync::atomic::AtomicBool::new(false);
        tokio::time::pause();
        let run = async {
            let result = crate::livekit::readiness::probe_live_output(
                &mut gemini,
                &mut state,
                &greeting,
                startup_output_limit(&config),
                &mut context,
                &mut input,
            )
            .await;
            done.store(true, Ordering::SeqCst);
            result
        };
        let drive = async {
            let real_deadline = Instant::now() + Duration::from_secs(5);
            while requested.load(Ordering::SeqCst) == 0 {
                assert!(Instant::now() < real_deadline, "opening was never sent");
                tokio::task::yield_now().await;
            }
            for _ in 0..seconds {
                assert!(
                    !done.load(Ordering::SeqCst),
                    "deadline ended early: {setting}"
                );
                tx.send(StartupInput::Audio(None)).unwrap();
                for _ in 0..100 {
                    tokio::task::yield_now().await;
                }
                tokio::time::advance(Duration::from_secs(1)).await;
            }
            // Tokio's timer wheel rounds a non-aligned deadline up to a ms.
            tokio::time::advance(Duration::from_millis(1)).await;
            for _ in 0..100 {
                tokio::task::yield_now().await;
            }
            assert!(
                done.load(Ordering::SeqCst),
                "media restarted the deadline: {setting}"
            );
        };
        let (result, ()) = tokio::join!(run, drive);
        let error = result.unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(
            crate::gemini::startup_failure_reason(error.as_ref()),
            "unavailable"
        );
        assert!(!left);
        assert!(matches!(
            deferred.as_slice(),
            [(_, RoomEvent::Reconnecting), (_, RoomEvent::Reconnected)]
        ));
        gemini.shutdown().await.unwrap();
        tokio::time::resume();
        assert_eq!(server.await.unwrap().generations, 1);
    }
}

#[tokio::test]
async fn startup_reads_native_microphone_frames_and_preserves_other_room_events() {
    use ::livekit::prelude::{LocalAudioTrack, RtcAudioSource};
    use ::livekit::webrtc::audio_frame::AudioFrame;
    use ::livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
    let expected = vec![0u8; 80 * 320];
    let (mut gemini, server, _) = startup_socket(Some(expected.len())).await;
    let config = startup_config("5");
    let mut state = startup_state_for(&config);
    let greeting = with_timer(&state, "opening".into());
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 16_000, 1, 0);
    let track = LocalAudioTrack::create_audio_track(
        "startup-microphone",
        RtcAudioSource::Native(source.clone()),
    );
    let mut media = CandidateMedia::new();
    media.audio = Some(NativeAudioStream::with_options(
        track.rtc_track(),
        16_000,
        1,
        NativeAudioStreamOptions {
            queue_size_frames: Some(4000),
        },
    ));
    for _ in 0..80 {
        source
            .capture_frame(&AudioFrame {
                data: vec![0; 160].into(),
                sample_rate: 16_000,
                num_channels: 1,
                samples_per_channel: 160,
            })
            .await
            .unwrap();
    }
    let (tx, events) = mpsc::unbounded_channel();
    tx.send(RoomEvent::Reconnecting).unwrap();
    let events = tokio::sync::Mutex::new(RoomEventReader::from(events));
    let mut deferred = Vec::new();
    let mut left = false;
    crate::livekit::readiness::probe_live_output(
        &mut gemini,
        &mut state,
        &greeting,
        startup_output_limit(&config),
        &mut StartupContext {
            media: &mut media,
            candidate_identity: "candidate",
            candidate_video: false,
            deferred_events: &mut deferred,
            room_closed: &mut left,
        },
        &mut &events,
    )
    .await
    .unwrap();
    assert!(!left);
    assert!(matches!(
        deferred.as_slice(),
        [(_, RoomEvent::Reconnecting)]
    ));
    gemini.shutdown().await.unwrap();
    let wire = server.await.unwrap();
    assert_eq!(wire.audio, expected);
    assert_eq!(wire.generations, 1);
}
