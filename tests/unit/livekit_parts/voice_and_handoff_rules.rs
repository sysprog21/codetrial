// Part of livekit.rs: included in place, so every item here shares that file's
// module.

#[test]
fn candidate_voice_overrides_preserve_the_configured_default() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google-key"),
        ("GEMINI_VOICE", "Aoede"),
    ])
    .unwrap();
    for voice in ["Puck", "Charon", "Kore", "Orus"] {
        let metadata = serde_json::json!({"interviewerVoice": voice}).to_string();
        assert_eq!(
            candidate_bootstrap(&config, "room", Some(&metadata)).voice,
            voice
        );
    }
    for metadata in [
        None,
        Some("{}"),
        Some("invalid JSON"),
        Some(r#"{"interviewerVoice":"Default"}"#),
        Some(r#"{"interviewerVoice":""}"#),
        Some(r#"{"interviewerVoice":"Unknown"}"#),
        Some(r#"{"interviewerVoice":42}"#),
        Some(r#"{"interviewerVoice":null}"#),
    ] {
        assert_eq!(
            candidate_bootstrap(&config, "room", metadata).voice,
            "Aoede"
        );
    }
    assert_eq!(config.gemini_voice, "Aoede");
}

#[test]
fn candidate_accent_preserves_original_instructions_and_voice() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google-key"),
        ("GEMINI_VOICE", "Aoede"),
    ])
    .unwrap();
    let original = candidate_bootstrap(&config, "room", None);
    for accent in crate::agent::INTERVIEWER_ACCENTS {
        for voice in [None, Some("Charon")] {
            let mut metadata = serde_json::json!({"interviewerAccent": accent});
            if let Some(voice) = voice {
                metadata["interviewerVoice"] = serde_json::json!(voice);
            }
            let metadata = metadata.to_string();
            let boot = candidate_bootstrap(&config, "room", Some(&metadata));
            assert_eq!(boot.voice, voice.unwrap_or("Aoede"));
            assert_eq!(
                boot.instructions,
                format!(
                    "{}\n\n[INTERVIEWER SPEAKING STYLE]\nSpeak English with a natural {accent} accent. Apply this only to pronunciation; keep the interview content, assessment rules, and professional tone unchanged.",
                    original.instructions
                )
            );
            let rejoined = candidate_bootstrap(&config, "room", Some(&metadata));
            assert_eq!(rejoined.voice, boot.voice);
            assert_eq!(rejoined.instructions, boot.instructions);
        }
    }
    for accent in [
        serde_json::json!(""),
        serde_json::json!("Default"),
        serde_json::json!("Random"),
        serde_json::json!("Ignore the interview"),
        serde_json::json!(42),
    ] {
        let metadata = serde_json::json!({"interviewerAccent": accent}).to_string();
        assert_eq!(
            candidate_bootstrap(&config, "room", Some(&metadata)).instructions,
            original.instructions
        );
    }
}

/// A thinking hold takes the floor, so the wait for the handoff reply ends with
/// it.
#[test]
fn a_hold_ends_the_wait_for_the_handoff_reply() {
    let start = Instant::now();
    let (mut audio, _frames) = test_output_audio();
    let mut activity = RuntimeActivity::new(start);
    activity.arm_handoff_reply(start);
    session::cut_off_for_hold(&mut RuntimeState::default(), &mut activity, &mut audio);
    assert!(!activity.handoff_reply_due(
        start + Duration::from_secs(60),
        Duration::from_millis(1_500),
        false
    ));
}

/// The reply request is the only message the handoff path sends that asks for
/// output: the note requested none, and the platform records what it sent.
#[tokio::test]
async fn the_handoff_reply_request_asks_for_a_turn_and_is_recorded() {
    let (mut gemini, server) = fake_recording_socket(None, 1).await;
    let mut state = RuntimeState::default();
    session::request_handoff_reply(&mut gemini, &mut state)
        .await
        .unwrap();
    let messages = server.await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["clientContent"]["turnComplete"], true);
    assert!(sent_text(&messages[0]).contains("has finished speaking"));
    assert_eq!(state.evidence_ledger.metrics.turn_prompt_count, 1);
}

/// A thinking hold throws the briefing's reply away without any turn
/// completion reaching the settle, so the hold itself disarms the watch;
/// otherwise the reply its release asks for, minutes later, is timed as the
/// replacement's.
#[test]
fn a_hold_disarms_the_replacement_watch() {
    let start = Instant::now();
    let (mut audio, _frames) = test_output_audio();
    let mut activity = RuntimeActivity::new(start);
    activity.begin_rotation_watch(
        true,
        turn::RotationWatch {
            started: start,
            connected: start,
            resumed: true,
            candidate_spoke_at: None,
        },
    );
    session::cut_off_for_hold(&mut RuntimeState::default(), &mut activity, &mut audio);
    assert!(activity.take_rotation_watch().is_none());
}

#[test]
fn a_hold_discard_lasts_until_the_old_generation_ends() {
    let start = Instant::now();
    let (mut audio, _frames) = test_output_audio();
    let mut activity = RuntimeActivity::new(start);
    activity.note_output(start);
    session::cut_off_for_hold(&mut RuntimeState::default(), &mut activity, &mut audio);
    assert!(activity.socket.discarding_output);
    activity.mark_prompted(start, Some("resume"), false);
    assert!(!accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut activity,
        false,
        start
    ));
    assert!(activity.owes_prompt());
}

#[tokio::test]
async fn a_button_hold_closes_latency_while_waiting_or_in_a_tool_only_reply() {
    for tool_only in [false, true] {
        let start = Instant::now();
        let mut activity = RuntimeActivity::new(start);
        activity.latency.voice(start);
        activity
            .latency
            .voice_end(start + Duration::from_millis(100));
        if tool_only {
            activity
                .latency
                .output(start + Duration::from_millis(150), true);
        }
        assert_eq!(activity.floor, Floor::Listening);
        let (mut gemini, server) = fake_resumed_socket().await;
        begin_button_hold(
            &mut gemini,
            &mut CandidateMedia::new(),
            &mut activity,
            start + Duration::from_millis(200),
            THINKING_TRANSCRIPT_GRACE,
        )
        .await
        .unwrap();
        assert_eq!(
            server.await.unwrap(),
            crate::gemini::realtime_audio_end_message()
        );
        assert!(
            activity
                .latency
                .boundary(
                    latency::Boundary::TurnComplete,
                    start + Duration::from_millis(300)
                )
                .is_none()
        );
        activity.latency.voice(start + Duration::from_secs(120));
        let record = activity
            .latency
            .audio(start + Duration::from_millis(120_500))
            .unwrap();
        assert_eq!(record["origin"], "last_voiced_pcm");
        assert_eq!(record["elapsed_ms"], 500);
        assert_eq!(record["first_tool_call_ms"], serde_json::Value::Null);
    }
}

#[tokio::test]
async fn a_listening_floor_hold_discards_the_tool_generation_before_a_release_reply() {
    let start = Instant::now() - Duration::from_secs(1);
    let mut turn = hold_turn(RuntimeState::default());
    turn.activity.latency.voice(start);
    turn.activity
        .latency
        .output(start + Duration::from_millis(100), true);
    turn.activity.note_output(Instant::now());
    assert_eq!(turn.activity.floor, Floor::Listening);
    let (mut output_audio, _frames) = test_output_audio();
    let (mut gemini, server) = fake_resumed_socket().await;
    let mut media = CandidateMedia::new();
    settle(
        &mut turn,
        &mut output_audio,
        &mut gemini,
        &mut media,
        &crate::agent::DataEventResult {
            thinking_changed: Some(true),
            ..Default::default()
        },
        &mut None,
    )
    .await;
    assert_eq!(
        server.await.unwrap(),
        crate::gemini::realtime_audio_end_message()
    );
    let released = Instant::now();
    turn.activity
        .mark_prompted(released, Some("release"), false);
    assert!(!session::accept_gemini_event(
        &GeminiEvent::Text("old continuation".into()),
        &mut turn.activity,
        false,
        released + Duration::from_millis(100)
    ));
    assert!(!session::accept_gemini_event(
        &GeminiEvent::TurnComplete,
        &mut turn.activity,
        false,
        released + Duration::from_millis(200)
    ));
    let record = turn
        .activity
        .latency
        .audio(released + Duration::from_millis(500))
        .unwrap();
    assert_eq!(record["origin"], "prompt");
    assert_eq!(record["elapsed_ms"], 500);
    assert_eq!(record["first_tool_call_ms"], serde_json::Value::Null);
}

#[tokio::test]
async fn thinking_release_rebases_existing_tool_continuation_waits() {
    let start = Instant::now();
    let mut turn = hold_turn(RuntimeState::default());
    turn.activity.note_output(start);
    turn.activity.note_tool_response(start);
    let released = start + Duration::from_secs(60);
    turn.activity.settle_stalls(released, false);
    let (mut audio, _frames) = test_output_audio();
    let (mut gemini, server) = fake_recording_socket(None, 1).await;
    let mut media = CandidateMedia::new();
    let mut board = board::Board::new();
    let result = crate::agent::DataEventResult {
        thinking_changed: Some(false),
        ..Default::default()
    };
    let mut reply = Some("resume".to_string());
    {
        let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
        settle_hold(
            &mut context,
            &result,
            &mut reply,
            released,
            THINKING_TRANSCRIPT_GRACE,
        )
        .await;
    }
    assert!(!turn.activity.reply_timed_out(released, false));
    assert!(
        turn.activity
            .reply_timed_out(released + FIRST_OUTPUT_DEADLINE, false)
    );
    server.await.unwrap();
}

/// LiveKit's transport is vendored so its proxied `wss://` path runs on the
/// bundled roots; upstream refuses that path unless the machine's roots are
/// enabled. The connection runs in a child so `HTTPS_PROXY` reaches only that
/// process, and the fake proxy here sees the CONNECT and the TLS handshake
/// that follows it through the tunnel.
#[test]
fn livekit_signaling_tunnels_through_an_https_proxy() {
    use std::io::{BufRead, Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());

    // Reported over a channel so a child that never reaches the proxy fails the
    // test instead of leaving it blocked in accept.
    let (seen, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(socket);
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        let mut socket = reader.into_inner();
        socket
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .unwrap();
        let mut record = [0u8; 1];
        socket.read_exact(&mut record).unwrap();
        let _ = seen.send((request, record[0]));
    });
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "livekit::tests::livekit_proxy_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("HTTPS_PROXY", &proxy)
        .env("CODETRIAL_LIVEKIT_PROXY_TEST_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    let (request, record) = heard
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the child tunneled through the proxy");
    assert_eq!(request.trim_end(), "CONNECT livekit.example:443 HTTP/1.1");
    // A TLS handshake record: the ClientHello went into the tunnel.
    assert_eq!(record, 0x16);
}

#[tokio::test]
async fn livekit_proxy_child() {
    if std::env::var_os("CODETRIAL_LIVEKIT_PROXY_TEST_CHILD").is_none() {
        return;
    }
    crate::install_crypto_provider();
    let client = livekit_net::testing::native_ws_client();
    let error = client
        .connect("wss://livekit.example/rtc".into(), Vec::new(), 5_000)
        .await
        .err()
        .expect("the fake proxy ends the tunnel after the ClientHello");
    assert!(!error.to_string().contains("requires"), "{error}");
}

/// Everything a closed socket owed is dropped with it, in one assignment, so
/// a wait added to `SocketWork` cannot be forgotten by the replacement. What
/// belongs to the candidate rather than the socket survives.
#[test]
fn a_replacement_drops_every_wait_the_old_socket_owed() {
    let now = Instant::now();
    let mut state = RuntimeState {
        thinking_hold: ThinkingHold::Held { since: now },
        end_requested: true,
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(now);
    activity.socket = turn::SocketWork {
        discarding_output: true,
        thinking_ignore_input_until: Some(now),
        reply_after_thinking_discard: true,
        thinking_reply_fallback: Some((now, "ready".into())),
        interrupted_turn_pending: true,
        stale_turn_pending: true,
        tool_response_outstanding: true,
    };
    clear_abandoned_socket_work(&mut state, &mut activity);
    assert_eq!(activity.socket, turn::SocketWork::default());
    assert!(!state.end_requested);
    assert!(state.thinking_hold.is_declared());
}

/// The interview a packet handler is given, for a room that never connected.
fn packet_config() -> AgentConfig {
    load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "packet-test-key"),
    ])
    .unwrap()
}

#[tokio::test]
async fn an_idle_goaway_resumes_on_a_replacement_socket() {
    const EDITOR_CODE: &str = "def two_sum(values, target):\n    return []\n";
    if std::env::var_os("CODETRIAL_GOAWAY_TEST_CHILD").is_none() {
        // Capture the real lifecycle line without a production logging hook. An
        // ignored test endpoint must never connect to the live provider.
        let guard = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        guard.set_nonblocking(true).unwrap();
        let proxy = format!("http://{}", guard.local_addr().unwrap());
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "livekit::tests::an_idle_goaway_resumes_on_a_replacement_socket",
                "--nocapture",
            ])
            .env("CODETRIAL_GOAWAY_TEST_CHILD", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env("NO_PROXY", "")
            .env("no_proxy", "");
        for name in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            child.env(name, &proxy);
        }
        let mut child = child.spawn().unwrap();
        let started = Instant::now();
        let output = loop {
            if child.try_wait().unwrap().is_some() {
                break child.wait_with_output().unwrap();
            }
            if started.elapsed() >= Duration::from_secs(20) {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "GoAway test child hung: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            guard.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "the replacement must not attempt an external endpoint"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        let replacements: Vec<_> = stderr
            .lines()
            .filter(|line| line.starts_with("replacement: "))
            .collect();
        assert_eq!(replacements.len(), 1, "{stderr}");
        assert!(replacements[0].contains("trigger=goaway handover=resumed fallback=none"));
        assert!(replacements[0].ends_with(" room=room-test"));
        let fields: std::collections::HashMap<_, _> = replacements[0]
            .split_whitespace()
            .filter_map(|field| field.split_once('='))
            .collect();
        assert!(
            fields["checkpoint_age"]
                .strip_suffix('s')
                .is_some_and(|seconds| seconds.parse::<u64>().is_ok())
        );
        assert_eq!(fields["owed"], "false");
        assert_eq!(fields["owed_chars"], "0");
        assert_eq!(
            fields["editor_chars"],
            EDITOR_CODE.chars().count().to_string()
        );
        let usage: Vec<_> = stderr
            .lines()
            .filter(|line| line.starts_with("codetrial live_turn_usage "))
            .collect();
        assert_eq!(usage.len(), 1, "{stderr}");
        let usage_fields: std::collections::HashMap<_, _> = usage[0]
            .split_whitespace()
            .filter_map(|field| field.split_once('='))
            .collect();
        assert_eq!(usage_fields["cause"], "tool");
        assert_eq!(usage_fields["prompt_tokens"], "42");
        assert_eq!(usage_fields["socket"], "1");
        assert_eq!(usage_fields["usage_event"], "1");
        return;
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        async fn receive_json(
            socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        ) -> serde_json::Value {
            let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("the client must send the expected setup or briefing")
                .unwrap()
                .unwrap();
            serde_json::from_str(frame.to_text().unwrap()).unwrap()
        }

        let config = packet_config();
        let keys = Arc::new(GeminiKeys::from_config(&config));
        let boot = crate::runtime::bootstrap(&config, "room-test", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut first = tokio_tungstenite::accept_async(socket).await.unwrap();
            let initial = receive_json(&mut first).await;
            first
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            first
                .send(Message::Text(
                    r#"{"sessionResumptionUpdate":{"resumable":true,"newHandle":"goaway-checkpoint"},"usageMetadata":{"promptTokenCount":42}}"#
                        .into(),
                ))
                .await
                .unwrap();
            first
                .send(Message::Text(r#"{"goAway":{"timeLeft":"9s"}}"#.into()))
                .await
                .unwrap();
            assert!(first.next().await.unwrap().unwrap().is_close());
            drop(first);

            let (socket, _) = listener.accept().await.unwrap();
            let mut second = tokio_tungstenite::accept_async(socket).await.unwrap();
            let setup = receive_json(&mut second).await;
            second
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            let briefing = receive_json(&mut second).await;
            assert!(second.next().await.unwrap().unwrap().is_close());
            (initial, setup, briefing)
        });
        let mut gemini = fixture_session_at(&endpoint, &keys, &boot, None)
            .await
            .unwrap();
        gemini.input_cause = Some("tool");

        // Leave the observation unaccounted, as queued usage abandoned by a
        // replacement would be. The actual replacement must drain its owner.
        assert!(matches!(gemini.next_event().await, Some(GeminiEvent::UsageRecorded)));
        let event = gemini.next_event().await.unwrap();
        assert!(matches!(event, GeminiEvent::GoAway { .. }));
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let events = tokio::sync::Mutex::new(receiver);
        let interview = InterviewContext {
            config: &config,
            keys: &keys,
            boot: &boot,
            live_endpoint: Some(&endpoint),
            started_at: Instant::now(),
            candidate_identity: "candidate",
            events: &events,
            slot: None,
        };
        let code = EDITOR_CODE;
        let mut turn = hold_turn(RuntimeState {
            code: code.into(),
            ..RuntimeState::default()
        });
        let initial_socket = turn.activity.live_socket;
        let (mut output, _frames) = test_output_audio();
        let mut board = board::Board::new();
        let mut media = CandidateMedia::new();
        let (room, recorder) = room_link::RoomLink::recording("room-test");
        let mut loops = RoomLoop { pending_round_notice: None,
            recovery_terminal: None, recovery_inputs: None, recovery_phase: None, recovery_reader: None, recovery_writes: Default::default(), recovery_after: Default::default(),
            deadline_at: tokio::time::Instant::now() + Duration::from_secs(3600),
            restarts: 0,
            deferred_restart: DeferredRestart::default(),
            resume_recovery: ResumeRecovery::default(),
            interim_review: SideCall::default(),
            presence: CandidatePresence::default(),
        };
        let flow = on_gemini_event(
            &room,
            &mut turn.context(&mut output, &mut gemini, &mut board, &mut media),
            Some(event),
            &mut loops,
            interview,
        )
        .await
        .unwrap();
        assert!(flow.is_continue());
        assert_eq!(loops.restarts, 1);
        assert!(loops.resume_recovery.resumed);
        assert!(!loops.deferred_restart.is_armed());
        assert_eq!(turn.activity.live_socket, initial_socket + 1);
        assert_eq!(turn.activity.live_usage.samples, 1);
        assert_eq!(turn.activity.live_usage.prompt, 42);
        assert!(!turn.activity.reply_unfinished());
        assert_eq!(turn.state.code, code);
        assert_eq!(turn.state.evidence_ledger.metrics.live_setup_count, 1);
        let records = recorder.take();
        assert!(!records.iter().any(|record| matches!(record, room_link::Sent::Left)));
        let notices: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                room_link::Sent::Data { topic, message }
                    if topic.as_deref() == Some(TOPIC_CONTROL)
                        && message["type"] == "interviewer_state" =>
                {
                    message["reconnecting"].as_bool()
                }
                _ => None,
            })
            .collect();
        assert_eq!(notices, [true, false]);
        gemini.shutdown().await.unwrap();
        let (initial, setup, briefing) = server.await.unwrap();
        assert_eq!(initial["setup"]["sessionResumption"], serde_json::json!({}));
        assert_eq!(setup["setup"]["sessionResumption"]["handle"], "goaway-checkpoint");
        assert_eq!(briefing["clientContent"]["turnComplete"], false);
        let context = sent_text(&briefing);
        for line in code.lines() {
            assert!(context.contains(line.trim()), "missing editor line: {line}");
        }
    })
    .await
    .expect("the actual GoAway handler must complete its local replacement");
}

#[tokio::test]
async fn only_a_packet_that_starts_a_hold_flushes_the_open_floor_overlap() {
    use crate::livekit::floor::Event;
    let config = packet_config();
    let keys = Arc::new(GeminiKeys::from_config(&config));
    let boot = candidate_bootstrap(&config, "room-test", None);
    let (_room_events, events) = tokio::sync::mpsc::unbounded_channel();
    let events = tokio::sync::Mutex::new(events);
    for (payload, flushed) in [
        (serde_json::json!({"type":"thinking","thinking":true}), true),
        (
            serde_json::json!({"type":"thinking","thinking":false}),
            false,
        ),
    ] {
        let interview = InterviewContext {
            config: &config,
            keys: &keys,
            boot: &boot,
            live_endpoint: None,
            started_at: Instant::now(),
            candidate_identity: "candidate",
            events: &events,
            slot: None,
        };
        let mut turn = hold_turn(RuntimeState::default());
        turn.activity.floor_tracker.observe(
            Event::Start {
                generating: true,
                ahead: Duration::ZERO,
            },
            Instant::now(),
            &mut |_| {},
        );
        let (mut gemini, _server) = fake_socket(None).await;
        let (mut output_audio, _queued) = test_output_audio();
        let mut board = board::Board::new();
        let mut media = CandidateMedia::new();
        let (room, _sent) = room_link::RoomLink::recording("room-test");
        {
            let mut context = turn.context(&mut output_audio, &mut gemini, &mut board, &mut media);
            let flow = apply_data_packet(
                &room,
                &mut context,
                interview,
                crate::runtime::TOPIC_CONTROL,
                &payload,
                true,
            )
            .await
            .unwrap();
            assert!(flow.is_continue());
        }
        let mut records = Vec::new();
        turn.activity
            .floor_tracker
            .observe(Event::Reset, Instant::now(), &mut |r| records.push(r));
        assert_eq!(
            records.iter().all(|r| r.kind != "overlap"),
            flushed,
            "{payload}"
        );
    }
}

#[test]
fn only_a_failed_reply_that_carried_thinking_debt_stays_owed() {
    let result = |carries_thinking_debt| crate::agent::DataEventResult {
        carries_thinking_debt,
        ..crate::agent::DataEventResult::default()
    };
    assert!(owes_failed_reply(&result(true)));
    assert!(!owes_failed_reply(&result(false)));
}

#[test]
fn only_a_test_results_packet_is_logged_as_a_test_run() {
    let state = RuntimeState::default();
    let result = crate::agent::DataEventResult::default();
    let line = test_run_line(crate::runtime::TOPIC_TEST_RESULTS, &state, &result, true)
        .expect("a test results packet is always logged");
    assert!(line.reacted && !line.ended);
    assert!(test_run_line(TOPIC_CONTROL, &state, &result, true).is_none());
}

#[test]
fn the_interview_length_is_counted_in_minutes() {
    let config = packet_config();
    let keys = Arc::new(GeminiKeys::from_config(&config));
    let boot = candidate_bootstrap(&config, "room-test", None);
    let (_room_events, events) = tokio::sync::mpsc::unbounded_channel();
    let events = tokio::sync::Mutex::new(events);
    let interview = InterviewContext {
        config: &config,
        keys: &keys,
        boot: &boot,
        live_endpoint: None,
        started_at: Instant::now().checked_sub(Duration::from_secs(90)).unwrap(),
        candidate_identity: "candidate",
        events: &events,
        slot: None,
    };
    let minutes = interview.elapsed_min();
    assert!((1.5..1.6).contains(&minutes), "{minutes}");
}

#[test]
fn delivery_uncertainty_is_shared_by_hold_and_socket_handoff() {
    for cut in ["hold", "handoff"] {
        let mut state = RuntimeState {
            transcript: vec!["Interviewer: Explain the map.".into()],
            ..RuntimeState::default()
        };
        let mut activity = RuntimeActivity::new(Instant::now());
        activity.generating = true;
        let (mut output, _) = test_output_audio();
        output.playout_deadline = Instant::now() + Duration::from_secs(60);
        state.interviewer_delivery.transcript(0);
        state.interviewer_delivery.audio(output.playout_deadline);
        match cut {
            "hold" => session::cut_off_for_hold(&mut state, &mut activity, &mut output),
            "handoff" => {
                hand_over(&mut state, &mut activity, &mut output);
            }
            _ => unreachable!(),
        }
        assert!(state.interviewer_delivery.uncertain.contains(&0), "{cut}");
        assert_eq!(state.transcript[0], "Interviewer: Explain the map.");
        state.interviewer_delivery.transcript(1);
        assert_eq!(
            state.interviewer_delivery.uncertain.contains(&1),
            cut != "handoff",
            "{cut}"
        );
    }
}

// A line whose audio already played is delivered, however the floor is taken
// afterwards. The same open line with completed output still playing may be
// that output's trailing words, and is marked.
#[test]
fn delivery_uncertainty_marks_an_open_line_only_when_its_cut_stops_audio() {
    for (cut, playing) in ["hold", "handoff"]
        .into_iter()
        .flat_map(|cut| [(cut, false), (cut, true)])
    {
        let mut state = RuntimeState {
            transcript: vec!["Interviewer: Explain the map.".into()],
            ..RuntimeState::default()
        };
        let mut activity = RuntimeActivity::new(Instant::now());
        let (mut output, _) = test_output_audio();
        state.interviewer_delivery.transcript(0);
        state.interviewer_delivery.audio(Instant::now());
        if playing {
            output.playout_deadline = Instant::now() + Duration::from_secs(60);
        }
        assert_eq!(output.is_playing(), playing);
        match cut {
            "hold" => session::cut_off_for_hold(&mut state, &mut activity, &mut output),
            "handoff" => {
                hand_over(&mut state, &mut activity, &mut output);
            }
            _ => unreachable!(),
        }
        assert_eq!(
            state.interviewer_delivery.uncertain.contains(&0),
            playing,
            "{cut} playing={playing}"
        );
        assert_eq!(
            state.interviewer_delivery.accepts(0),
            !playing,
            "{cut} playing={playing}"
        );

        // A replaced socket sends nothing of the cut output, so its window
        // ends.
        state.interviewer_delivery.transcript(1);
        assert_eq!(
            state.interviewer_delivery.uncertain.contains(&1),
            playing && cut != "handoff",
            "{cut} playing={playing}"
        );
    }
}

// A hold that cuts a running generation seals its line, so the reply after the
// release is not written into text every reader omits.
#[test]
fn delivery_uncertainty_seals_the_line_a_hold_cut() {
    let mut state = RuntimeState {
        transcript: vec!["Interviewer: Explain the map.".into()],
        ..RuntimeState::default()
    };
    let mut activity = RuntimeActivity::new(Instant::now());
    activity.generating = true;
    let (mut output, _) = test_output_audio();
    state.interviewer_delivery.transcript(0);
    session::cut_off_for_hold(&mut state, &mut activity, &mut output);
    assert!(state.interviewer_delivery.uncertain.contains(&0));
    assert!(!state.interviewer_delivery.accepts(0));
    state.interviewer_delivery.complete();
    assert!(!state.interviewer_delivery.accepts(0));
    state.interviewer_delivery.transcript(1);
    assert!(!state.interviewer_delivery.uncertain.contains(&1));
}

// A stalled generation's completion may never come, so cutting it must not
// leave a window that marks the next, delivered reply until its own end.
#[test]
fn delivery_uncertainty_after_a_stalled_hold_ends_at_the_next_audio() {
    let mut state = RuntimeState {
        transcript: vec!["Interviewer: Explain the map.".into()],
        ..RuntimeState::default()
    };
    let now = Instant::now();
    let mut activity = RuntimeActivity::new(now);
    activity.generating = true;
    activity.last_output_at = Some(now - PROMPT_STALL);
    assert!(activity.generation_stalled(now));
    let (mut output, _) = test_output_audio();
    state.interviewer_delivery.transcript(0);
    session::cut_off_for_hold(&mut state, &mut activity, &mut output);
    assert!(state.interviewer_delivery.uncertain.contains(&0));
    state.interviewer_delivery.audio(Instant::now());
    state.interviewer_delivery.transcript(1);
    assert!(!state.interviewer_delivery.uncertain.contains(&1));
}

// A review that already read a line before it was cut is not taken back: the
// cursor stays where it was and the next window starts after it.
#[test]
fn delivery_uncertainty_does_not_rewind_an_interim_review() {
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut state = RuntimeState {
        transcript: vec![
            "Candidate: I use a map.".into(),
            "Interviewer: Use the queued hint.".into(),
        ],
        ..RuntimeState::default()
    };
    assert!(take_interim_review_window(&mut state, &boot).contains("queued hint"));
    state.interviewer_delivery.uncertain.insert(1);
    state
        .transcript
        .push("Candidate: I compare the keys.".into());
    let next = take_interim_review_window(&mut state, &boot);
    assert_eq!(state.interim_transcript_lines, 3);
    assert!(next.contains("I compare the keys."));
    assert!(!next.contains(crate::agent::CANCELLED_INTERVIEWER_OUTPUT));
    assert!(!next.contains("queued hint"));
}

#[test]
fn delivery_uncertainty_reaches_interim_and_phase_judge_windows() {
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut state = RuntimeState {
        transcript: vec![
            "Candidate: I use a map.".into(),
            "Interviewer: Use the secret cancelled hint.".into(),
            "Candidate: I compare the keys.".into(),
        ],
        interim_transcript_lines: 1,
        ..RuntimeState::default()
    };
    state.interviewer_delivery.uncertain.insert(1);
    for window in [
        take_interim_review_window(&mut state, &boot),
        crate::agent::take_phase_judge_window(&mut state, boot.problem),
    ] {
        assert!(window.contains(crate::agent::CANCELLED_INTERVIEWER_OUTPUT));
        assert!(!window.contains("secret cancelled hint"));
        assert!(window.contains("I compare the keys."));
    }
    assert_eq!(
        state.transcript[1],
        "Interviewer: Use the secret cancelled hint."
    );
}

/// The soak judges a handover from the key=value fields of the `replacement:`
/// line, so the spellings it parses have to be the ones printed. Each value is
/// looked for in the judge's source, and the line carries all three keys.
#[test]
fn the_soak_parses_the_replacement_fields_the_agent_prints() {
    let judge = include_str!("../../../scripts/soak-judge.cjs");
    for trigger in [Trigger::Watchdog, Trigger::Close, Trigger::GoAway] {
        assert!(
            judge.contains(&format!("\"{}\"", trigger.as_str())),
            "{trigger:?}"
        );
    }
    for handover in [Handover::Resumed, Handover::ColdHandoff, Handover::Degraded] {
        assert!(
            judge.contains(&format!("\"{}\"", handover.as_str())),
            "{handover:?}"
        );
    }
    assert_eq!(
        replacement_fields(
            Trigger::GoAway,
            Handover::ColdHandoff,
            Fallback::CheckpointBeforeHandoff
        ),
        "trigger=goaway handover=cold_handoff fallback=checkpoint_before_handoff"
    );
}

/// An operator reads these, so a deliberate cold fallback must not read as a
/// lost conversation, and no notice may contain another's distinguishing text.
#[test]
fn the_replacement_notices_tell_the_three_handovers_apart() {
    let resumed = replacement_notice(Handover::Resumed);
    let cold_handoff = replacement_notice(Handover::ColdHandoff);
    let degraded = replacement_notice(Handover::Degraded);

    let cold_marker = "restart rebuilt locally";
    let degraded_marker = "restart degraded";
    assert!(cold_handoff.contains(cold_marker));
    assert!(degraded.contains(degraded_marker));
    assert!(!cold_handoff.contains(degraded_marker) && !resumed.contains(degraded_marker));
    assert!(!degraded.contains(cold_marker) && !resumed.contains(cold_marker));
}

/// The room goes after the text the soak matches, so it names the room without
/// moving the match.
#[test]
fn a_replacement_notice_names_its_room_after_the_matched_text() {
    for handover in [Handover::Resumed, Handover::ColdHandoff, Handover::Degraded] {
        let line = replacement_notice_line(handover, "room-a");
        assert_eq!(
            line,
            format!("{} room=room-a", replacement_notice(handover))
        );
    }
}

/// Only the speech handoff's own checkpoint rule makes a cold start the
/// designed fallback. Anything else that ends cold, no handle, resumption
/// suppressed after a young resumed socket, a handle that was offered and
/// refused, or no speech handoff at all, is a failure to resume and stays
/// degraded. The reason is named, so the soak can tell a stall's rebuild from a
/// rotation that lost its handle.
#[test]
fn only_the_handoff_checkpoint_rule_makes_a_cold_start_the_designed_fallback() {
    use Fallback::{CheckpointBeforeHandoff, NoHandle, ResumeRefused, ResumeSuppressed};
    use Handover::{ColdHandoff, Degraded, Resumed};
    // resumed, speech_handoff, allow_resume, had_handle, handle_offered
    for (case, reason, expected) in [
        ((true, true, true, true, true), Fallback::None, Resumed),
        ((true, false, true, true, true), Fallback::None, Resumed),
        (
            (false, true, true, true, false),
            CheckpointBeforeHandoff,
            ColdHandoff,
        ),
        ((false, true, true, false, false), NoHandle, Degraded),
        (
            (false, true, false, true, false),
            ResumeSuppressed,
            Degraded,
        ),
        ((false, true, true, true, true), ResumeRefused, Degraded),
        ((false, false, true, true, false), ResumeRefused, Degraded),
        ((false, false, false, false, false), NoHandle, Degraded),
    ] {
        let (resumed, handoff, allow, had, offered) = case;
        let fallback = fallback_reason(resumed, handoff, allow, had, offered);
        assert_eq!(fallback, reason, "{case:?}");
        assert_eq!(handover_kind(fallback), expected, "{case:?}");
    }
}

/// Joins a fake provider's task, failing the test when the client never gave
/// it what it waits for, so a client that stops sending fails a test instead of
/// hanging the suite.
async fn bounded<T>(task: tokio::task::JoinHandle<T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the fake provider was left waiting for the client")
        .unwrap()
}

pub(super) async fn fake_recording_socket(
    handle: Option<&str>,
    count: usize,
) -> (
    GeminiLiveSession,
    tokio::task::JoinHandle<Vec<serde_json::Value>>,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "sequence-test-key"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let _setup = socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();

        // Bounded, so a write that never happens fails the test instead of
        // hanging it: under mutation testing a hang is a timeout, which is
        // neither caught nor missed.
        let mut expected = count.max(1);
        let mut messages = Vec::with_capacity(expected);
        while messages.len() < expected {
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("the client never sent the message the test waits for")
                .unwrap()
                .unwrap();
            if message.is_close() {
                break;
            }
            let message: serde_json::Value =
                serde_json::from_str(message.to_text().unwrap()).unwrap();
            if count == 0
                && messages.is_empty()
                && message["clientContent"]["turnComplete"] == false
                && sent_text(&message).starts_with("[SYSTEM EVENT] TIMER:")
            {
                expected = 2;
            }
            messages.push(message);
        }
        messages
    });
    let gemini = fixture_session_at(
        &url,
        &keys,
        &boot,
        handle.map(|handle| ("sequence-test-key", handle)),
    )
    .await
    .unwrap();
    (gemini, server)
}

/// A local peer either completes setup promptly or fails its fixture. Keep
/// failures from paying the production setup deadline in every room test.
pub(super) async fn fixture_session_at(
    endpoint: &str,
    keys: &GeminiKeys,
    boot: &dyn crate::gemini::LiveSession,
    handle: Option<(&str, &str)>,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    tokio::time::timeout(
        Duration::from_secs(2),
        crate::gemini::live_session_with_keys_at(endpoint, keys, boot, handle),
    )
    .await
    .expect("the fixture provider must complete setup")
}
