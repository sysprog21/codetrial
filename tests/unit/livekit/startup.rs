use super::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_tungstenite::tungstenite::Message;

struct StartupWire {
    audio: Vec<u8>,
    generations: usize,
    pings: usize,
}

async fn startup_socket(
    answer_after_bytes: Option<usize>,
) -> (
    GeminiLiveSession,
    tokio::task::JoinHandle<StartupWire>,
    Arc<AtomicUsize>,
) {
    let config = startup_config("60");
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let received = Arc::new(AtomicUsize::new(0));
    let observed = received.clone();
    let server = tokio::spawn(async move {
        use base64::Engine;
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut wire = StartupWire {
            audio: Vec::new(),
            generations: 0,
            pings: 0,
        };
        while let Some(Ok(message)) = socket.next().await {
            match message {
                Message::Close(_) => break,
                Message::Ping(bytes) => {
                    wire.pings += 1;
                    socket.send(Message::Pong(bytes)).await.unwrap();
                }
                Message::Text(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value["clientContent"]["turnComplete"] == true {
                        wire.generations += 1;
                        if answer_after_bytes.is_none() {
                            observed.store(1, Ordering::SeqCst);
                        }
                    }
                    if let Some(data) = value["realtimeInput"]["audio"]["data"].as_str() {
                        wire.audio.extend(
                            base64::engine::general_purpose::STANDARD
                                .decode(data)
                                .unwrap(),
                        );
                        observed.store(wire.audio.len(), Ordering::SeqCst);
                        if Some(wire.audio.len()) == answer_after_bytes {
                            socket.send(Message::Text(r#"{"serverContent":{"modelTurn":{"parts":[{"text":"Hello"}]}}}"#.into())).await.unwrap();
                        }
                    }
                }
                _ => {}
            }
        }
        wire
    });
    let gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        .await
        .unwrap();
    (gemini, server, received)
}

fn startup_config(first_output: &str) -> AgentConfig {
    load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "startup-media"),
        ("CODETRIAL_GEMINI_FIRST_OUTPUT_S", first_output),
        ("CODETRIAL_GEMINI_REPLY_TIMEOUT_S", "90"),
    ])
    .unwrap()
}

fn startup_state_for(config: &AgentConfig) -> RuntimeState {
    let boot = crate::runtime::bootstrap(config, "interview-fixed", Some("two-sum"), 45);
    initial_runtime_state(&boot, Instant::now())
}

#[tokio::test]
async fn startup_pumps_more_than_the_audio_queue_before_a_late_first_output() {
    let wire = startup_with_audio_until_output("60", 5_500).await;
    assert!((3..=5).contains(&wire.pings), "pings={}", wire.pings);
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

async fn startup_with_audio_until_output(first_output: &str, frames: usize) -> StartupWire {
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
    let probe = crate::livekit::probe_live_output(
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
        [RoomEvent::Reconnecting, RoomEvent::Reconnected]
    ));
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
        let error = crate::livekit::probe_live_output(
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
            let result = crate::livekit::probe_live_output(
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
            [RoomEvent::Reconnecting, RoomEvent::Reconnected]
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
    let events = tokio::sync::Mutex::new(events);
    let mut deferred = Vec::new();
    let mut left = false;
    crate::livekit::probe_live_output(
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
    assert!(matches!(deferred.as_slice(), [RoomEvent::Reconnecting]));
    gemini.shutdown().await.unwrap();
    let wire = server.await.unwrap();
    assert_eq!(wire.audio, expected);
    assert_eq!(wire.generations, 1);
}
