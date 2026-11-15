//! Starting a Live session: the credential check that mirrors a first open,
//! the startup failure reasons the browser words, the probe that observes the
//! first model output without consuming it, and the reader hooks those tests
//! use to stall a socket at an exact point.

use super::*;
use crate::gemini::startup::check_live_session_at;

#[test]
fn startup_failure_identifies_exhausted_rotations() {
    for (prefix, failures, expected) in [
        (
            "startup-exhausted-quota",
            [CredentialFailure::Quota, CredentialFailure::Quota],
            "quota",
        ),
        (
            "startup-exhausted-billing",
            [CredentialFailure::Billing, CredentialFailure::Billing],
            "billing",
        ),
        (
            "startup-exhausted-mixed",
            [CredentialFailure::Quota, CredentialFailure::Billing],
            "billing",
        ),
        (
            "startup-exhausted-refused",
            [CredentialFailure::Invalid, CredentialFailure::Refused],
            "unavailable",
        ),
    ] {
        let first = format!("{prefix}-first");
        let second = format!("{prefix}-second");
        let configured = format!("{first},{second}");
        let config = live_config(&[("GOOGLE_API_KEYS", &configured)]);
        let keys = GeminiKeys::from_config(&config);
        for (key, failure) in [(&first, failures[0]), (&second, failures[1])] {
            keys.failed(key, failure, ApiSurface::Live);
        }
        let error = keys.select().unwrap_err();
        assert_eq!(credential_failure(&error), None);
        assert_eq!(startup_failure_reason(&error), expected, "{prefix}");
    }
    assert_eq!(
        startup_failure_reason(&io::Error::other("1011 Internal error")),
        "unavailable"
    );
}

thread_local! {
    static CONSTRAIN_STARTUP_SOCKET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static STARTUP_OUTPUT_PAUSE: std::cell::RefCell<Option<StartupOutputPause>> = const { std::cell::RefCell::new(None) };
}

pub(in crate::gemini) struct StartupOutputPause {
    decoded: tokio::sync::oneshot::Sender<(std::time::Instant, GeminiEvent)>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

pub(in crate::gemini) fn take_startup_output_pause() -> Option<StartupOutputPause> {
    STARTUP_OUTPUT_PAUSE.take()
}

pub(in crate::gemini) async fn pause_startup_output(
    pause: &mut Option<StartupOutputPause>,
    received_at: std::time::Instant,
    event: &GeminiEvent,
) {
    if crate::gemini::startup::model_output(event)
        && let Some(pause) = pause.take()
    {
        pause.decoded.send((received_at, event.clone())).unwrap();
        let _ = pause.resume.await;
    }
}

// Consumed once on this test's current-thread runtime; other sockets keep their
// production settings. The reader and writer themselves remain unchanged.
pub(in crate::gemini) fn constrain_startup_socket(stream: &super::super::socket::Socket) {
    if CONSTRAIN_STARTUP_SOCKET.replace(false) {
        let tokio_tungstenite::MaybeTlsStream::Plain(tcp) = stream.get_ref() else {
            panic!("startup backpressure fixture requires a loopback socket");
        };
        socket2::SockRef::from(tcp)
            .set_send_buffer_size(4096)
            .unwrap();
    }
}

impl GeminiLiveSession {
    pub(crate) fn startup_test_events(
        &mut self,
    ) -> tokio::sync::mpsc::Sender<(std::time::Instant, GeminiEvent)> {
        let (send, events) = channel(GEMINI_EVENT_QUEUE);
        self.events = events;
        send
    }

    pub(crate) fn pause_next_startup_test_output() -> (
        tokio::sync::oneshot::Receiver<(std::time::Instant, GeminiEvent)>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (decoded, received) = tokio::sync::oneshot::channel();
        let (resume, release) = tokio::sync::oneshot::channel();
        STARTUP_OUTPUT_PAUSE.set(Some(StartupOutputPause {
            decoded,
            resume: release,
        }));
        (received, resume)
    }

    pub(crate) fn startup_test_queue_len(&self) -> usize {
        self.events.len()
    }

    pub(crate) fn constrain_next_startup_test_socket() {
        CONSTRAIN_STARTUP_SOCKET.set(true);
    }

    pub(crate) fn startup_reader_observer(
        &self,
    ) -> (impl Fn() -> usize + use<>, tokio::task::AbortHandle) {
        let usage = Arc::clone(&self.usage);
        (
            move || usage.lock().unwrap().len(),
            self.reader.as_ref().unwrap().abort_handle(),
        )
    }
}

#[tokio::test]
async fn startup_go_away_is_replayed_before_output_with_original_timestamps() {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let request = socket.next().await.unwrap().unwrap();
        let request: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
        assert_eq!(request["clientContent"]["turnComplete"], true);
        socket
            .send(Message::Text(r#"{"goAway":{"timeLeft":"9.5s"}}"#.into()))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                r#"{"serverContent":{"modelTurn":{"parts":[{"text":"Hello"}]}}}"#.into(),
            ))
            .await
            .unwrap();
        let mut generations = 1;
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
            if message.is_text() {
                generations += 1;
            }
        }
        generations
    });
    let mut session = open_live_session_at(&url, &boot, None).await.unwrap();

    // Tap the real reader, retaining its timestamps rather than inventing the
    // observations the replay is supposed to preserve.
    let (send, recv) = channel(GEMINI_EVENT_QUEUE);
    let mut events = std::mem::replace(&mut session.events, recv);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let captured = std::sync::Arc::clone(&observed);
    let forwarding = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            captured.lock().unwrap().push(event.clone());
            if send.send(event).await.is_err() {
                break;
            }
        }
    });
    session.send_context("opening", true).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        session.wait_for_model_output(&mut tokio::time::interval(KEEPALIVE_INTERVAL)),
    )
    .await
    .unwrap()
    .unwrap();
    let expected = observed.lock().unwrap().clone();
    assert_eq!(expected.len(), 2);
    assert!(matches!(&expected[0].1, GeminiEvent::GoAway { time_left } if time_left == "9.5s"));
    assert_eq!(expected[1].1, GeminiEvent::Text("Hello".into()));
    // Replay later, so stamping at consumption cannot accidentally pass.
    tokio::time::sleep(Duration::from_millis(20)).await;
    for (received_at, event) in expected {
        assert_eq!(session.next_event().await, Some(event));
        assert_eq!(session.event_received_at(), Some(received_at));
    }

    // Events the probe buffered were consumed once, when it took them off the
    // channel: a replayed prefix is no backlog in the pressure record.
    let pressure = session.pressure_for_test();
    assert_eq!(pressure.consumed, pressure.received);
    session.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), forwarding)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn startup_speech_extraction_preserves_remaining_events_and_timestamps() {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "startup-extract", Some("two-sum"), 45);
    let (address, _frames) = stalled_live_server(false).await;
    let mut session = open_live_session_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    let events = [
        GeminiEvent::InputTranscript("first candidate turn".into()),
        GeminiEvent::TurnComplete,
        GeminiEvent::InputTranscript("second candidate turn".into()),
        GeminiEvent::UsageRecorded,
        GeminiEvent::Text("opening".into()),
    ];
    let began = std::time::Instant::now();
    for (index, event) in events.iter().enumerate() {
        session
            .buffer_startup_event((began + Duration::from_millis(index as u64), event.clone()))
            .unwrap();
    }
    assert_eq!(session.take_buffered_startup_speech(), events[..3]);
    assert_eq!(
        session.take_buffered_startup_speech(),
        [GeminiEvent::TurnComplete]
    );
    for index in [1, 3, 4] {
        assert_eq!(session.next_event().await, Some(events[index].clone()));
        assert_eq!(
            session.event_received_at(),
            Some(began + Duration::from_millis(index as u64))
        );
    }
    assert!(session.pending_events.is_empty());
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_output_requires_content_and_keeps_a_bounded_replay() {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let (address, _frames) = stalled_live_server(false).await;
    let mut session = open_live_session_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    let empty = [
        GeminiEvent::Audio {
            bytes: vec![],
            mime_type: "audio/pcm".into(),
        },
        GeminiEvent::Text(" ".into()),
        GeminiEvent::OutputTranscript("\n".into()),
        GeminiEvent::ToolCall(vec![]),
        GeminiEvent::InputTranscript("candidate".into()),
        GeminiEvent::CandidateVoiceActivity {
            active: true,
            audio_offset: None,
        },
        GeminiEvent::CandidateVoiceActivity {
            active: false,
            audio_offset: None,
        },
        GeminiEvent::UsageRecorded,
        GeminiEvent::TurnComplete,
        GeminiEvent::Interrupted,
    ];
    let (send, recv) = channel(16);
    session.events = recv;
    for event in &empty {
        send.send((std::time::Instant::now(), event.clone()))
            .await
            .unwrap();
    }
    drop(send);
    assert!(
        session
            .wait_for_model_output(&mut tokio::time::interval(KEEPALIVE_INTERVAL))
            .await
            .is_err()
    );
    for output in [
        GeminiEvent::Audio {
            bytes: vec![1, 2],
            mime_type: "audio/pcm".into(),
        },
        GeminiEvent::Text("answer".into()),
        GeminiEvent::OutputTranscript("answer".into()),
        GeminiEvent::ToolCall(vec![GeminiFunctionCall {
            id: "1".into(),
            name: "read_editor".into(),
            args: json!({}),
        }]),
    ] {
        session.pending_events.clear();
        let (send, recv) = channel(16);
        session.events = recv;
        let received_at = std::time::Instant::now() - Duration::from_secs(2);
        for event in &empty {
            send.send((received_at, event.clone())).await.unwrap();
        }
        send.send((received_at + Duration::from_millis(10), output.clone()))
            .await
            .unwrap();
        drop(send);
        assert_eq!(
            session
                .wait_for_model_output(&mut tokio::time::interval(KEEPALIVE_INTERVAL))
                .await
                .unwrap(),
            received_at + Duration::from_millis(10)
        );
        for expected in &empty {
            assert_eq!(session.next_event().await.as_ref(), Some(expected));
            assert_eq!(session.event_received_at(), Some(received_at));
        }
        assert_eq!(session.next_event().await, Some(output));
        assert_eq!(
            session.event_received_at(),
            Some(received_at + Duration::from_millis(10))
        );
        // The live queue after replay must retain its own receive time too.
        let (send, recv) = channel(1);
        session.events = recv;
        let next_received_at = received_at + Duration::from_millis(20);
        send.send((next_received_at, GeminiEvent::TurnComplete))
            .await
            .unwrap();
        assert_eq!(session.next_event().await, Some(GeminiEvent::TurnComplete));
        assert_eq!(session.event_received_at(), Some(next_received_at));
    }
    let (send, recv) = channel(GEMINI_EVENT_QUEUE + 1);
    session.events = recv;
    for _ in 0..=GEMINI_EVENT_QUEUE {
        send.send((std::time::Instant::now(), GeminiEvent::TurnComplete))
            .await
            .unwrap();
    }
    drop(send);
    assert!(
        session
            .wait_for_model_output(&mut tokio::time::interval(KEEPALIVE_INTERVAL))
            .await
            .unwrap_err()
            .to_string()
            .contains("event limit")
    );
    assert_eq!(session.pending_events.len(), GEMINI_EVENT_QUEUE);
    let reader = session.reader.as_ref().unwrap().abort_handle();
    drop(session);
    tokio::task::yield_now().await;
    assert!(
        reader.is_finished(),
        "startup cancellation must retire the reader"
    );
}

#[tokio::test]
async fn startup_buffered_rescue_uses_an_exclusive_fixed_deadline() {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "startup-cutoff", Some("two-sum"), 45);
    let (address, _frames) = stalled_live_server(false).await;
    let mut session = open_live_session_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    let deadline = std::time::Instant::now();
    for pending in [false, true] {
        for received_at in [
            deadline - Duration::from_nanos(1),
            deadline,
            deadline + Duration::from_nanos(1),
        ] {
            for output in [
                GeminiEvent::Audio {
                    bytes: vec![1, 2],
                    mime_type: "audio/pcm".into(),
                },
                GeminiEvent::Text("opening".into()),
                GeminiEvent::OutputTranscript("opening".into()),
                GeminiEvent::ToolCall(vec![GeminiFunctionCall {
                    id: "1".into(),
                    name: "read_editor".into(),
                    args: json!({}),
                }]),
            ] {
                let expected = [
                    GeminiEvent::TurnComplete,
                    output,
                    GeminiEvent::UsageRecorded,
                    GeminiEvent::Interrupted,
                ];
                let (send, recv) = channel(GEMINI_EVENT_QUEUE);
                session.events = recv;
                for event in &expected {
                    if pending {
                        session
                            .buffer_startup_event((received_at, event.clone()))
                            .unwrap();
                    } else {
                        send.send((received_at, event.clone())).await.unwrap();
                    }
                }
                assert_eq!(
                    session.buffered_startup_output(deadline).unwrap(),
                    (received_at < deadline).then_some(received_at)
                );
                for event in expected {
                    assert_eq!(session.next_event().await, Some(event));
                    assert_eq!(session.event_received_at(), Some(received_at));
                }
                assert!(session.pending_events.is_empty());
                assert!(session.events.is_empty());
            }
        }
    }
    let (send, recv) = channel(GEMINI_EVENT_QUEUE + 1);
    session.events = recv;
    for _ in 0..=GEMINI_EVENT_QUEUE {
        send.send((
            deadline - Duration::from_nanos(1),
            GeminiEvent::TurnComplete,
        ))
        .await
        .unwrap();
    }
    assert!(
        session
            .buffered_startup_output(deadline)
            .unwrap_err()
            .to_string()
            .contains("event limit")
    );
    assert_eq!(session.pending_events.len(), GEMINI_EVENT_QUEUE);
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_reader_observation_keeps_the_cutoff_and_prefix_bound() {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "startup-observation", Some("two-sum"), 45);
    let (address, _frames) = stalled_live_server(false).await;
    let mut session = open_live_session_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    let deadline = std::time::Instant::now();
    // No published state cannot be rescued without waiting for the reader.
    assert_eq!(session.buffered_startup_output(deadline).unwrap(), None);
    for received_at in [
        deadline - Duration::from_nanos(1),
        deadline,
        deadline + Duration::from_nanos(1),
    ] {
        session.first_output = Arc::new(OnceLock::new());
        session.first_output.set(received_at).unwrap();
        assert_eq!(
            session.buffered_startup_output(deadline).unwrap(),
            (received_at < deadline).then_some(received_at)
        );
        assert!(session.pending_events.is_empty());
        assert!(
            session.events.is_empty(),
            "observation must not manufacture replay events"
        );
    }
    session.first_output = Arc::new(OnceLock::new());
    session
        .first_output
        .set(deadline - Duration::from_nanos(1))
        .unwrap();

    // A failed write aborts the reader and can discard its in-flight event.
    // Metadata alone cannot substitute for replay once that owner is gone.
    session.dead = true;
    assert_eq!(session.buffered_startup_output(deadline).unwrap(), None);
    session.dead = false;
    let (send, recv) = channel(GEMINI_EVENT_QUEUE + 1);
    session.events = recv;
    for _ in 0..=GEMINI_EVENT_QUEUE {
        send.send((
            deadline - Duration::from_nanos(1),
            GeminiEvent::TurnComplete,
        ))
        .await
        .unwrap();
    }
    assert!(
        session
            .buffered_startup_output(deadline)
            .unwrap_err()
            .to_string()
            .contains("event limit"),
        "metadata must not bypass the bounded replay prefix"
    );
    assert_eq!(session.pending_events.len(), GEMINI_EVENT_QUEUE);
    session.pending_events.clear();
    session.stop_reader().await;
    assert_eq!(
        session.buffered_startup_output(deadline).unwrap(),
        None,
        "an ended reader no longer owns an event to enqueue"
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_output_wait_keeps_a_late_first_reply_alive() {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (asked, request) = tokio::sync::oneshot::channel();
    let (release, mut output) = tokio::sync::oneshot::channel();
    let (acknowledged, mut acks) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let frame = socket.next().await.unwrap().unwrap();
        let input: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        assert_eq!(input["realtimeInput"]["text"], "opening");
        asked.send(()).unwrap();
        let mut generations = 1;
        let mut pings = 0;
        let mut sent_output = false;
        loop {
            tokio::select! {
                _ = &mut output, if !sent_output => {
                    socket.send(Message::Text(r#"{"serverContent":{"modelTurn":{"parts":[{"text":"late answer"}]}}}"#.into())).await.unwrap();
                    sent_output = true;
                }
                frame = socket.next() => match frame.unwrap().unwrap() {
                    Message::Ping(payload) => {
                        socket.send(Message::Pong(payload)).await.unwrap();

                        // The observation acknowledges receipt by the reader,
                        // not just completion of the server's loopback write.
                        socket.send(Message::Text(r#"{"usageMetadata":{"promptTokenCount":0}}"#.into())).await.unwrap();
                        pings += 1;
                        acknowledged.send(pings).unwrap();
                    }
                    Message::Text(_) => generations += 1,
                    Message::Close(_) => break,
                    _ => {},
                }
            }
        }
        (generations, pings)
    });
    let mut session = open_live_session_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    session.send_text("opening").await.unwrap();
    let request = tokio::time::timeout(Duration::from_secs(5), request).await;
    if !matches!(request, Ok(Ok(()))) {
        let _ = session.shutdown().await;
        server.abort();
        let _ = server.await;
        panic!("startup fixture never received the opening request: {request:?}");
    }
    let observed = Arc::clone(&session.usage);
    let waiting = tokio::spawn(async move {
        let result = tokio::time::timeout(
            Duration::from_secs(90),
            session.wait_for_model_output(&mut tokio::time::interval(KEEPALIVE_INTERVAL)),
        )
        .await;
        (session, result)
    });
    tokio::task::yield_now().await;
    let began = tokio::time::Instant::now();
    for count in 1..=3 {
        tokio::time::pause();
        tokio::time::advance(KEEPALIVE_INTERVAL).await;
        tokio::time::resume();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), acks.recv())
                .await
                .unwrap(),
            Some(count)
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while observed.lock().unwrap().len() < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!waiting.is_finished(), "no model output has been sent yet");
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::time::resume();
    assert!(began.elapsed() > READ_IDLE_LIMIT);
    release.send(()).unwrap();
    let (mut session, result) = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .unwrap()
        .unwrap();
    result.unwrap().unwrap();
    for _ in 0..3 {
        assert_eq!(session.next_event().await, Some(GeminiEvent::UsageRecorded));
    }
    assert_eq!(
        session.next_event().await,
        Some(GeminiEvent::Text("late answer".into()))
    );
    session.shutdown().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap(),
        (1, 3)
    );
}

/// A check that reached a key the interview's first open never could would
/// pass on a configuration that cannot start an interview.
#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is a full HTTP response"
)]
async fn check_stops_where_the_first_interview_open_would() {
    let names: Vec<String> = (0..crate::livekit::GEMINI_RESTART_LIMIT + 3)
        .map(|index| format!("check-budget-{index}"))
        .collect();
    let config = live_config(&[("GOOGLE_API_KEYS", &names.join(","))]);
    let keys = GeminiKeys::from_config(&config);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = Arc::clone(&connections);
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = tokio_tungstenite::accept_hdr_async(
                socket,
                |_: &tokio_tungstenite::tungstenite::handshake::server::Request,
                 _: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let mut refusal =
                        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(None);
                    *refusal.status_mut() = axum::http::StatusCode::UNAUTHORIZED;
                    Err(refusal)
                },
            )
            .await;
        }
    });
    assert!(check_live_session_at(&url, &keys, &boot).await.is_err());
    server.abort();
    assert_eq!(
        connections.load(std::sync::atomic::Ordering::SeqCst),
        crate::livekit::GEMINI_RESTART_LIMIT + 1
    );
}

/// A 503 says nothing about the key, so a check reports it instead of
/// spreading the outage across the backups.
#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is a full HTTP response"
)]
async fn check_reports_a_503_without_trying_the_backups() {
    let config = live_config(&[("GOOGLE_API_KEYS", "check-503-first,check-503-backup")]);
    let keys = GeminiKeys::from_config(&config);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = Arc::clone(&connections);
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = tokio_tungstenite::accept_hdr_async(
                socket,
                |_: &tokio_tungstenite::tungstenite::handshake::server::Request,
                 _: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let mut refusal =
                        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(None);
                    *refusal.status_mut() = axum::http::StatusCode::SERVICE_UNAVAILABLE;
                    Err(refusal)
                },
            )
            .await;
        }
    });
    assert!(check_live_session_at(&url, &keys, &boot).await.is_err());
    server.abort();
    assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(keys.select().unwrap(), "check-503-first");
}

#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is a full HTTP response"
)]
async fn check_moves_past_a_rejected_key_to_a_working_backup() {
    let config = live_config(&[("GOOGLE_API_KEYS", "check-rejected,check-backup")]);
    let keys = GeminiKeys::from_config(&config);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut seen = Vec::new();
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let mut key = String::new();
            let accepted = tokio_tungstenite::accept_hdr_async(
                socket,
                |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                    key = request.uri().query().unwrap_or_default().to_string();
                    if key.ends_with("rejected") {
                        let mut refusal =
                            tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(
                                None,
                            );
                        *refusal.status_mut() = axum::http::StatusCode::UNAUTHORIZED;
                        Err(refusal)
                    } else {
                        Ok(response)
                    }
                },
            )
            .await;
            seen.push(key);
            if let Ok(mut socket) = accepted {
                socket.next().await.unwrap().unwrap();
                socket
                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                    .await
                    .unwrap();

                // Bounded: a close that never arrives is `shutdown`'s failure
                // to report, and this test is about which keys were tried.
                let _ = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
                return seen;
            }
        }
    });
    let session = check_live_session_at(&url, &keys, &boot).await.unwrap();
    let _ = session.close().await;
    assert_eq!(
        server.await.unwrap(),
        ["key=check-rejected", "key=check-backup"]
    );
    assert_eq!(keys.select().unwrap(), "check-backup");
}
