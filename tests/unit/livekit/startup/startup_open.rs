use super::*;

/// The interview starts once, at the opening's first output: the instant the
/// browser places on its own clock from the ready notice's elapsed time.
/// Stamping setup's or a later instant instead puts this process off the
/// tab's clock for the whole interview, and the round boundary is announced
/// off the browser's. Read as source because the stamps are only
/// distinguishable in a room that really connected.
#[test]
fn the_interview_clock_starts_at_the_probed_first_output() {
    let source = include_str!("../../../../src/livekit/startup/open.rs");
    let function = |name: &str| {
        source
            .split(name)
            .nth(1)
            .unwrap_or_else(|| panic!("{name} is still defined here"))
            .split("\n}\n")
            .next()
            .unwrap_or_default()
    };
    let body = function("async fn open_interview(");
    let probed = body
        .find("let started_at = match probed")
        .expect("the clock is bound from the probe's result");
    let stamped = body
        .find("turn.state.started_at = started_at;")
        .expect("the probed instant is the interview clock");
    assert!(probed < stamped);
    for consumer in [
        "ready_runtime_activity(\n        config,\n        room_name,\n        started_at,",
        "startup_ready_notice(started_at,",
        "Ok(started_at)",
    ] {
        assert!(body.contains(consumer), "{consumer} reads the same clock");
    }
}

#[test]
fn startup_results_use_the_existing_control_path() {
    assert_eq!(
        startup_state(None),
        serde_json::json!({
            "type": "interviewer_state", "reconnecting": false, "startup": "ready"
        })
    );
    let error = std::io::Error::other("1011 Internal error, 503, or silence");
    assert_eq!(startup_state(Some(&error))["reason"], "unavailable");
}

#[tokio::test]
async fn startup_probe_waits_for_output_and_preserves_the_opening_events() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "startup-success"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let (asked, request) = tokio::sync::oneshot::channel();
    let (release, output) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let clock = socket.next().await.unwrap().unwrap();
        let clock: serde_json::Value = serde_json::from_str(clock.to_text().unwrap()).unwrap();
        assert_eq!(clock["clientContent"]["turnComplete"], false);
        assert!(
            clock["clientContent"]["turns"][0]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .contains("TIMER:")
        );
        let message = socket.next().await.unwrap().unwrap();
        let message: serde_json::Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(message["clientContent"]["turnComplete"], true);
        assert_eq!(
            message["clientContent"]["turns"][0]["parts"][0]["text"],
            "opening"
        );
        asked.send(()).unwrap();
        output.await.unwrap();
        socket
            .send(Message::Text(
                r#"{"serverContent":{"inputTranscription":{"text":"Candidate"}}}"#.into(),
            ))
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
    let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        .await
        .unwrap();
    let mut state = initial_runtime_state(&boot, Instant::now());
    let greeting = with_timer(&state, "opening".to_string());
    let before = Instant::now();
    let ready = {
        let probe = probe_live_output(&mut gemini, &mut state, &greeting, Duration::from_secs(5));
        tokio::pin!(probe);
        tokio::select! {
            result = &mut probe => panic!("setup alone must not start the interview: {result:?}"),
            _ = request => {}
        }
        tokio::select! {
            result = &mut probe => panic!("no model output yet: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        release.send(()).unwrap();
        probe.await.unwrap()
    };
    assert!(ready.duration_since(before) >= Duration::from_millis(20));
    assert_eq!(
        gemini.next_event().await,
        Some(GeminiEvent::InputTranscript("Candidate".into()))
    );
    assert_eq!(
        gemini.next_event().await,
        Some(GeminiEvent::Text("Hello".into()))
    );
    assert_eq!(gemini.event_received_at(), Some(ready));
    gemini.shutdown().await.unwrap();
    assert_eq!(server.await.unwrap(), 1);
}

#[test]
fn startup_activity_preserves_opening_latency_and_observed_output() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "startup-latency"),
        ("CODETRIAL_GEMINI_FIRST_OUTPUT_S", "5"),
        ("CODETRIAL_GEMINI_REPLY_TIMEOUT_S", "90"),
    ])
    .unwrap();
    let requested = Instant::now();
    let received = requested + Duration::from_secs(55);
    let ready = requested + Duration::from_secs(60);
    let (activity, mut probes) =
        ready_runtime_activity(&config, "startup-room", ready, requested, 7, "opening");
    assert!(!activity.reply_timed_out(ready, false));
    assert_eq!(
        activity.expired_reply_wait(ready + Duration::from_secs(5), false),
        None,
    );
    assert!(
        !activity.generating,
        "queued boundaries have not been replayed yet"
    );
    assert_eq!(activity.buffered_startup_output_at, Some(ready));
    assert_eq!(
        activity.expired_reply_wait(ready + Duration::from_secs(90), false),
        Some(Duration::from_secs(90))
    );
    assert_eq!(activity.reply_timeout, Duration::from_secs(90));
    let record = probes.latency.audio(received).unwrap();
    assert_eq!(record["room"], "startup-room");
    assert_eq!(record["session"], 7);
    assert_eq!(record["origin"], "prompt");
    assert_eq!(record["elapsed_ms"], 55_000);
}

#[tokio::test]
async fn startup_probe_bounds_silence_without_retrying_or_guessing_quota() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    for frame in [
        r#"{}"#,
        r#"{"goAway":{"timeLeft":"9.5s"}}"#,
        r#"{"serverContent":{"turnComplete":true}}"#,
        r#"{"serverContent":{"modelTurn":{"parts":[{"text":"  "}]}}}"#,
        r#"{"error":{"code":402,"message":"billing is not enabled"}}"#,
        r#"{"error":{"code":429,"message":"resource exhausted"}}"#,
        r#"{"error":{"code":401,"message":"invalid API key"}}"#,
        r#"{"error":{"code":403,"message":"permission denied"}}"#,
        r#"{"error":{"code":503,"message":"internal error"}}"#,
    ] {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "startup-failure"),
        ])
        .unwrap();
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            let clock = socket.next().await.unwrap().unwrap();
            let clock: serde_json::Value = serde_json::from_str(clock.to_text().unwrap()).unwrap();
            assert_eq!(clock["clientContent"]["turnComplete"], false);
            let request = socket.next().await.unwrap().unwrap();
            let request: serde_json::Value =
                serde_json::from_str(request.to_text().unwrap()).unwrap();
            assert_eq!(request["clientContent"]["turnComplete"], true);
            socket.send(Message::Text(frame.into())).await.unwrap();
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
        let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
            .await
            .unwrap();
        let mut state = initial_runtime_state(&boot, Instant::now());
        let greeting = with_timer(&state, "opening".to_string());
        let error = probe_live_output(
            &mut gemini,
            &mut state,
            &greeting,
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        let reason = crate::gemini::startup_failure_reason(error.as_ref());
        assert_eq!(
            reason,
            if frame.contains("402") {
                "billing"
            } else if frame.contains("429") {
                "quota"
            } else if frame.contains("401") {
                "credentials"
            } else if frame.contains("403") {
                "refused"
            } else {
                "unavailable"
            }
        );
        gemini.shutdown().await.unwrap();
        drop(gemini);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap(),
            1
        );
    }
}

#[tokio::test]
async fn startup_slow_ready_publication_keeps_the_original_greeting() {
    use futures_util::{FutureExt, SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    for (watch_first, prefix) in [
        (true, None),
        (false, None),
        (true, Some(GeminiEvent::TurnComplete)),
        (true, Some(GeminiEvent::Interrupted)),
    ] {
        let config = packet_config();
        let keys = Arc::new(GeminiKeys::from_config(&config));
        let boot = crate::runtime::bootstrap(&config, "startup-handoff", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let leading = prefix.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            let mut generations = 0;
            while let Some(Ok(message)) = socket.next().await {
                if message.is_close() {
                    break;
                }
                if let Message::Text(text) = message {
                    let request: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if request["clientContent"]["turnComplete"] == true {
                        generations += 1;
                        if let Some(event) = &leading {
                            let field = if matches!(event, GeminiEvent::TurnComplete) {
                                "turnComplete"
                            } else {
                                "interrupted"
                            };
                            socket
                                .send(Message::Text(
                                    serde_json::json!({"serverContent": {field: true}})
                                        .to_string()
                                        .into(),
                                ))
                                .await
                                .unwrap();
                        }
                        socket.send(Message::Text(r#"{"usageMetadata":{"promptTokenCount":42,"responseTokenCount":7,"totalTokenCount":49}}"#.into())).await.unwrap();
                        socket
                            .send(Message::Text(
                                r#"{"serverContent":{"outputTranscription":{"text":"Hello. "}}}"#
                                    .into(),
                            ))
                            .await
                            .unwrap();
                        socket.send(Message::Text(r#"{"serverContent":{"outputTranscription":{"text":"Let's begin."},"turnComplete":true}}"#.into())).await.unwrap();
                    }
                }
            }
            assert!(
                listener.accept().now_or_never().is_none(),
                "no replacement connection"
            );
            generations
        });
        let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
            .await
            .unwrap();
        let mut turn = hold_turn(initial_runtime_state(&boot, Instant::now()));
        let greeting = with_timer(&turn.state, "opening".to_string());
        let requested = Instant::now();
        let started = probe_live_output(
            &mut gemini,
            &mut turn.state,
            &greeting,
            startup_output_limit(&config),
        )
        .await
        .unwrap();
        turn.state.started_at = started;
        (turn.activity, turn.probes) =
            ready_runtime_activity(&config, boot.room_name, started, requested, 7, &greeting);
        let (room, sent) = RoomLink::recording(boot.room_name);

        // Pause only after the real local socket has produced output. Runtime
        // activity uses std::Instant, so pass the matching explicit instant to
        // the same watchdog decision the room tick uses.
        tokio::time::pause();
        let publishing = tokio::time::Instant::now();
        publish_startup_readiness(
            &mut gemini,
            &mut crate::gemini::TokenUsage::default(),
            async {
                tokio::time::sleep(Duration::from_secs(12)).await;
                publish_interviewer_state(&room, &startup_state(None)).await
            },
        )
        .await
        .unwrap();
        let elapsed = publishing.elapsed();
        tokio::time::resume();
        assert!(elapsed >= Duration::from_secs(12));
        assert!(elapsed <= Duration::from_secs(12) + Duration::from_millis(1));
        let tick_at = started + elapsed;
        let mut loops = RoomLoop {
            presence: CandidatePresence::default(),
            ..crate::livekit::tests::startup::room_loop(3)
        };
        let (_tx, events) = mpsc::unbounded_channel();
        let events = tokio::sync::Mutex::new(RoomEventReader::from(events));
        let interview = InterviewContext {
            config: &config,
            keys: &keys,
            boot: &boot,
            live_endpoint: None,
            started_at: started,
            candidate_identity: "candidate",
            events: &events,
            slot: None,
        };
        let (mut output_audio, _frames) = test_output_audio();
        let mut board = Board::new();
        let mut media = CandidateMedia::new();
        let mut delivered = Vec::new();
        let mut received = None;
        let count = 4 + usize::from(prefix.is_some());
        for index in 0..count {
            if watch_first || index > 1 {
                turn.activity.settle_stalls(tick_at, false);
                assert_ne!(
                    reply_watch(&turn.state, &turn.activity, tick_at, false),
                    ReplyWatch::Recover,
                    "ready publication is not a silent Gemini generation; event {index}"
                );
                let mut context =
                    turn.context(&mut output_audio, &mut gemini, &mut board, &mut media);
                assert!(
                    on_watch_tick(&room, &mut context, &mut loops, interview, tick_at)
                        .await
                        .unwrap()
                        .is_continue()
                );
                assert_eq!(loops.restarts, 3);
            }
            let event = gemini.next_event().await.unwrap();
            let timestamp = gemini.event_received_at().unwrap();
            assert!(received.is_none_or(|previous| timestamp >= previous));
            received = Some(timestamp);
            delivered.push(event.clone());
            let mut context = turn.context(&mut output_audio, &mut gemini, &mut board, &mut media);
            assert!(
                on_gemini_event(&room, &mut context, Some(event), &mut loops, interview)
                    .await
                    .unwrap()
                    .is_continue()
            );
            assert_eq!(loops.restarts, if index == count - 1 { 0 } else { 3 });
            assert_eq!(turn.activity.live_socket, 1);
        }
        let mut expected: Vec<_> = prefix.into_iter().collect();
        expected.extend([
            GeminiEvent::UsageRecorded,
            GeminiEvent::OutputTranscript("Hello. ".into()),
            GeminiEvent::OutputTranscript("Let's begin.".into()),
            GeminiEvent::TurnComplete,
        ]);
        assert_eq!(delivered, expected);
        assert_eq!(
            turn.state.transcript,
            vec!["Interviewer: Hello. Let's begin."]
        );
        assert_eq!(turn.activity.live_usage.samples, 1);
        assert_eq!(turn.activity.live_usage.total, 49);
        assert_eq!(
            reply_watch(&turn.state, &turn.activity, tick_at, false),
            ReplyWatch::Settled
        );
        assert!(sent.take().iter().all(|event| !matches!(event, room_link::Sent::Data { message, .. } if message["reconnecting"] == true)));
        gemini.shutdown().await.unwrap();
        assert_eq!(
            server.await.unwrap(),
            1,
            "only the original greeting generation"
        );
    }
}

#[test]
fn startup_handoff_uses_the_progress_timeout_without_changing_later_deadlines() {
    let mut config = packet_config();
    for first in [5, 10] {
        for progress in [20, 45, 90] {
            config.gemini_first_output_s = first;
            config.gemini_reply_timeout_s = progress;
            let start = Instant::now();
            let state = RuntimeState::default();
            let (mut activity, _) =
                ready_runtime_activity(&config, "startup-handoff", start, start, 7, "opening");
            assert_ne!(
                reply_watch(&state, &activity, start + Duration::from_secs(12), false),
                ReplyWatch::Recover
            );
            let deadline = start + Duration::from_secs(u64::from(progress));
            assert!(!activity.reply_timed_out(deadline - Duration::from_millis(1), false));
            assert_eq!(
                activity.expired_reply_wait(deadline, false),
                Some(Duration::from_secs(u64::from(progress)))
            );
            activity.note_output(start + Duration::from_secs(12));
            activity.note_turn_boundary(start + Duration::from_secs(13));
            activity.mark_prompted(deadline, Some("next question"), false);
            let next_deadline = deadline + Duration::from_secs(u64::from(first));
            assert!(!activity.reply_timed_out(next_deadline - Duration::from_millis(1), false));
            assert_eq!(
                reply_watch(&state, &activity, next_deadline, false),
                ReplyWatch::Recover
            );
        }
    }
}

#[test]
fn startup_buffered_output_is_consumed_even_when_a_hold_drops_it() {
    let config = packet_config();
    let now = Instant::now();
    for held in [false, true] {
        let (mut activity, _) =
            ready_runtime_activity(&config, "startup-handoff", now, now, 7, "opening");
        assert!(activity.accept_event(&GeminiEvent::UsageRecorded, held, now));
        assert_eq!(activity.buffered_startup_output_at, Some(now));
        let accepted =
            activity.accept_event(&GeminiEvent::OutputTranscript("Hello".into()), held, now);
        assert_eq!(accepted, !held);
        assert_eq!(activity.buffered_startup_output_at, None);
        activity.note_turn_boundary(now);
        activity.mark_prompted(now, Some("new question"), false);
        assert_eq!(
            activity.expired_reply_wait(now + FIRST_OUTPUT_DEADLINE, false),
            Some(FIRST_OUTPUT_DEADLINE)
        );
    }
    let (mut activity, _) =
        ready_runtime_activity(&config, "startup-handoff", now, now, 7, "opening");
    let mut state = RuntimeState::default();
    let (mut output_audio, _frames) = test_output_audio();
    hand_over(
        &mut state,
        &mut activity,
        &mut TurnProbes::default(),
        &mut output_audio,
    );
    assert_eq!(
        activity.buffered_startup_output_at, None,
        "a replacement cannot inherit buffered progress"
    );
}

#[tokio::test]
async fn startup_silent_generation_times_out_on_the_controlled_clock() {
    use futures_util::{FutureExt, SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let config = packet_config();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "startup-silent", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let (sent, requested) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut sent = Some(sent);
        let mut generations = 0;
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
            if let Message::Text(text) = message {
                let request: serde_json::Value = serde_json::from_str(&text).unwrap();
                if request["clientContent"]["turnComplete"] == true {
                    generations += 1;
                    if let Some(sent) = sent.take() {
                        sent.send(()).unwrap();
                    }
                }
            }
        }
        assert!(listener.accept().now_or_never().is_none());
        generations
    });
    let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        .await
        .unwrap();
    let mut state = initial_runtime_state(&boot, Instant::now());
    let greeting = with_timer(&state, "opening".to_string());
    let limit = startup_output_limit(&config);
    let error = {
        let probe = probe_live_output(&mut gemini, &mut state, &greeting, limit);
        tokio::pin!(probe);
        tokio::select! {
            result = &mut probe => panic!("startup must still be waiting: {result:?}"),
            result = requested => result.unwrap(),
        }
        tokio::time::pause();
        assert!(probe.as_mut().now_or_never().is_none());
        tokio::time::advance(limit + Duration::from_millis(1)).await;
        let error = probe.await.unwrap_err();
        tokio::time::resume();
        error
    };
    assert!(
        error.to_string().contains("did not produce startup output"),
        "{error}"
    );
    assert_eq!(
        crate::gemini::startup_failure_reason(error.as_ref()),
        "unavailable"
    );
    gemini.shutdown().await.unwrap();
    assert_eq!(
        server.await.unwrap(),
        1,
        "silence never requests another startup generation"
    );
}

#[tokio::test]
async fn startup_ready_publication_preserves_observed_usage() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    for outcome in ["success", "error", "stall"] {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "startup-accounting"),
        ])
        .unwrap();
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            socket.next().await.unwrap().unwrap();
            socket.next().await.unwrap().unwrap();
            socket.send(Message::Text(r#"{"usageMetadata":{"promptTokenCount":42,"responseTokenCount":7,"totalTokenCount":49}}"#.into())).await.unwrap();
            socket
                .send(Message::Text(
                    r#"{"serverContent":{"modelTurn":{"parts":[{"text":"Hello"}]}}}"#.into(),
                ))
                .await
                .unwrap();
            socket
                .next()
                .await
                .is_some_and(|message| message.unwrap().is_close())
        });
        let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
            .await
            .unwrap();
        let mut state = initial_runtime_state(&boot, Instant::now());
        let greeting = with_timer(&state, "opening".to_string());
        let mut summaries = Vec::new();
        let mut publication_elapsed = Duration::ZERO;
        let result = account_startup(
            &config,
            "interview-fixed",
            7,
            |line| summaries.push(line),
            async |usage| {
                probe_live_output(&mut gemini, &mut state, &greeting, Duration::from_secs(5))
                    .await?;
                tokio::time::pause();
                let began = tokio::time::Instant::now();
                let result = publish_startup_readiness(&mut gemini, usage, async {
                    if outcome == "stall" {
                        std::future::pending::<()>().await;
                    }
                    if outcome == "success" {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        Ok(())
                    } else {
                        Err(std::io::Error::other("notice rejected").into())
                    }
                })
                .await;
                publication_elapsed = began.elapsed();
                tokio::time::resume();
                result
            },
        )
        .await;
        if outcome == "success" {
            result.unwrap();

            // Tokio's timer wheel may round a deadline up to the next
            // millisecond.
            assert!(publication_elapsed >= Duration::from_secs(2));
            assert!(publication_elapsed <= Duration::from_secs(2) + Duration::from_millis(1));
            assert!(publication_elapsed > RECONNECT_NOTICE_LIMIT);
            assert!(
                summaries.is_empty(),
                "success has no startup failure summary"
            );
            assert!(!server.is_finished(), "publication must keep Gemini open");
            gemini.shutdown().await.unwrap();
            let mut usage = crate::gemini::TokenUsage::default();
            for observed in gemini.drain_usage() {
                usage.add(observed);
            }
            assert_eq!(usage.prompt, 42);
            assert_eq!(usage.response, 7);
            assert_eq!(usage.total, 49);
            assert_eq!(usage.samples, 1);
            assert!(gemini.drain_usage().is_empty());
            assert!(server.await.unwrap(), "the live session closes on shutdown");
            continue;
        }
        let error = result.unwrap_err();
        assert_eq!(startup_state(Some(error.as_ref()))["reason"], "delivery");
        let delivery = error.downcast_ref::<StartupDeliveryError>().unwrap();
        assert_eq!(
            delivery.0.downcast_ref::<std::io::Error>().unwrap().kind(),
            if outcome == "stall" {
                std::io::ErrorKind::TimedOut
            } else {
                std::io::ErrorKind::Other
            }
        );
        if outcome == "stall" {
            assert!(publication_elapsed >= STARTUP_READY_PUBLICATION_LIMIT);
            assert!(
                publication_elapsed
                    <= STARTUP_READY_PUBLICATION_LIMIT
                        + crate::gemini::CLOSE_TIMEOUT
                        + Duration::from_millis(1)
            );
        } else {
            assert!(publication_elapsed <= crate::gemini::CLOSE_TIMEOUT);
        }
        assert!(
            gemini.drain_usage().is_empty(),
            "observations are accounted once"
        );
        assert_eq!(error.to_string().contains("timed out"), outcome == "stall");
        assert_eq!(
            summaries.len(),
            1,
            "the production error exit logs exactly once"
        );
        let summary = &summaries[0];
        assert!(
            summary.contains("phase=startup prompt_tokens=42 response_tokens=7"),
            "{summary}"
        );
        assert!(
            summary.contains("usage_samples=1 total_tokens=49"),
            "{summary}"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap(),
            "the failed startup closes its socket"
        );
    }
}

#[tokio::test]
async fn startup_room_join_failure_is_accounted_exactly_once() {
    let mut config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "startup-join-accounting"),
    ])
    .unwrap();
    // Fail in the SDK before room setup, without reaching any external host.
    config.livekit_url = "not-a-livekit-url".to_string();
    let keys = Arc::new(GeminiKeys::from_config(&config));
    let mut summaries = Vec::new();

    // A hang guard, not a timing claim: the URL cannot even be parsed, so
    // nothing waits on a network, but a loaded test host can still be slow.
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        open_session(
            &config,
            &keys,
            "interview-startup-failure",
            1,
            7,
            None,
            |line| {
                summaries.push(line);
            },
        ),
    )
    .await
    .expect("an invalid URL must fail without waiting on a network");
    assert!(
        matches!(result, Ok(OpenSessionOutcome::FailedBeforeCandidate(_))),
        "a failed room join must preserve the pre-candidate release outcome"
    );
    assert_eq!(summaries.len(), 1, "every startup failure is counted once");
    let summary = &summaries[0];
    assert!(summary.starts_with("codetrial live_usage room=interview-startup-failure session=7 "));
    assert!(
        summary.contains("elapsed_s=0 outcome=error phase=startup"),
        "{summary}"
    );
    assert!(summary.contains("usage_samples=0"), "{summary}");
    assert!(summary.contains("prompt_tokens=0"), "{summary}");
}

#[tokio::test(start_paused = true)]
async fn startup_failure_waits_for_receipt_before_leaving() {
    let (received, receipt) = tokio::sync::oneshot::channel();
    let published = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = std::sync::Arc::clone(&published);
    let delivery = tokio::spawn(async move {
        publish_startup_failure(
            async {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
            async {
                receipt.await.unwrap();
            },
        )
        .await;
    });
    tokio::task::yield_now().await;
    assert_eq!(published.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        !delivery.is_finished(),
        "SDK publication only queues the notice; leaving now loses a delayed classified packet's sender"
    );
    received.send(()).unwrap();
    delivery.await.unwrap();
    assert_eq!(published.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn startup_failure_missing_receipt_shares_the_publication_deadline() {
    let began = tokio::time::Instant::now();
    let delivery = tokio::spawn(async {
        publish_startup_failure(
            async {
                tokio::time::sleep(RECONNECT_NOTICE_LIMIT / 2).await;
                Ok(())
            },
            std::future::pending(),
        )
        .await;
    });
    tokio::task::yield_now().await;
    tokio::time::advance(RECONNECT_NOTICE_LIMIT / 2).await;
    tokio::task::yield_now().await;
    assert!(
        !delivery.is_finished(),
        "a delayed publish must still allow a receipt"
    );
    tokio::time::advance(RECONNECT_NOTICE_LIMIT / 2 - Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert!(!delivery.is_finished());
    tokio::time::advance(Duration::from_millis(1)).await;
    delivery.await.unwrap();
    assert_eq!(began.elapsed(), RECONNECT_NOTICE_LIMIT);
}

#[test]
fn startup_receipt_matches_browser_bytes_and_only_the_candidate() {
    let wire: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/startup-receipt.json")).unwrap();
    assert_eq!(wire["cases"].as_array().unwrap().len(), 1);
    let payload = serde_json::to_vec(&wire["cases"][0]["payload"]).unwrap();
    let packet = br#"{"type":"interviewer_state","reconnecting":false,"startup":"failed","reason":"billing"}"#;
    let id = crate::sha256_hex(&[packet]);
    let topic = wire["topic"].as_str();
    assert!(is_startup_receipt(
        topic,
        Some("candidate"),
        "candidate",
        &payload,
        &id
    ));
    assert!(!is_startup_receipt(
        topic,
        Some("other"),
        "candidate",
        &payload,
        &id
    ));
    assert!(!is_startup_receipt(topic, None, "candidate", &payload, &id));
    assert!(!is_startup_receipt(
        None,
        Some("candidate"),
        "candidate",
        &payload,
        &id
    ));
    assert!(!is_startup_receipt(
        topic,
        Some("candidate"),
        "candidate",
        &payload,
        "different-notice"
    ));
    assert!(!is_startup_receipt(
        topic,
        Some("candidate"),
        "candidate",
        b"invalid json",
        &id
    ));
    let report =
        serde_json::to_vec(&serde_json::json!({"type":"report_received", "deliveryId":id}))
            .unwrap();
    assert!(!is_startup_receipt(
        topic,
        Some("candidate"),
        "candidate",
        &report,
        &id
    ));
}

#[tokio::test(start_paused = true)]
async fn startup_failure_rejected_or_stalled_publication_does_not_wait_for_receipt() {
    for stalled in [false, true] {
        let began = tokio::time::Instant::now();
        publish_startup_failure(
            async {
                if stalled {
                    std::future::pending::<()>().await;
                }
                Err(std::io::Error::other("notice rejected").into())
            },
            async {
                panic!("a rejected publication cannot wait for a receipt");
            },
        )
        .await;
        assert_eq!(
            began.elapsed(),
            if stalled {
                RECONNECT_NOTICE_LIMIT
            } else {
                Duration::ZERO
            }
        );
    }
}

#[tokio::test]
async fn startup_open_failover_does_not_repeat_the_readiness_generation() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    for answers in [false, true] {
        let primary = format!("startup-open-{answers}-first");
        let backup = format!("startup-open-{answers}-backup");
        let configured = format!("{primary},{backup}");
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEYS", configured.as_str()),
        ])
        .unwrap();
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut refused = tokio_tungstenite::accept_async(socket).await.unwrap();
            refused.next().await.unwrap().unwrap();
            refused
                .send(Message::Text(
                    r#"{"error":{"code":401,"message":"invalid API key"}}"#.into(),
                ))
                .await
                .unwrap();
            drop(refused);
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            let clock: serde_json::Value =
                serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(clock["clientContent"]["turnComplete"], false);
            let request: serde_json::Value =
                serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(request["clientContent"]["turnComplete"], true);
            socket
                .send(Message::Text(
                    if answers {
                        r#"{"serverContent":{"modelTurn":{"parts":[{"text":"Hello"}]}}}"#
                    } else {
                        r#"{"serverContent":{"turnComplete":true}}"#
                    }
                    .into(),
                ))
                .await
                .unwrap();
            let mut generations = 1;
            while let Some(Ok(frame)) = socket.next().await {
                if frame.is_close() {
                    break;
                }
                if frame.is_text() {
                    generations += 1;
                }
            }
            generations
        });
        let mut restarts = 0;
        let mut gemini = crate::gemini::first_open_within(
            crate::gemini::FIRST_OPEN_LIMIT,
            retry_cold_open(
                &keys,
                &mut restarts,
                Some(tokio::time::Instant::now() + crate::gemini::FIRST_OPEN_LIMIT),
                || crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None),
                |_| async {},
            ),
        )
        .await
        .unwrap();
        assert_eq!(restarts, 1);
        assert_eq!(keys.select().unwrap(), backup);
        let mut state = initial_runtime_state(&boot, Instant::now());
        let greeting = with_timer(&state, "opening".to_string());
        assert_eq!(
            probe_live_output(
                &mut gemini,
                &mut state,
                &greeting,
                Duration::from_millis(100)
            )
            .await
            .is_ok(),
            answers
        );
        let _ = gemini.shutdown().await;
        drop(gemini);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap(),
            1
        );
    }
}

#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is a full HTTP response"
)]
async fn startup_open_preserves_quota_when_cooldown_outlasts_its_budget() {
    use tokio_tungstenite::tungstenite::handshake::server;

    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEYS", "startup-cooled-a,startup-cooled-b"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = tokio_tungstenite::accept_hdr_async(
                socket,
                |_: &server::Request, _: server::Response| {
                    let mut refusal = server::ErrorResponse::new(None);
                    *refusal.status_mut() = axum::http::StatusCode::TOO_MANY_REQUESTS;
                    Err(refusal)
                },
            )
            .await;
        }
    });
    for _ in 0..2 {
        assert!(
            crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
                .await
                .is_err()
        );
    }
    server.await.unwrap();

    let mut restarts = 0;
    let mut attempts = 0;
    let mut notices = Vec::new();
    let limit = COLD_OPEN_BACKOFF / 4;
    let error = crate::gemini::first_open_within(
        limit,
        retry_cold_open(
            &keys,
            &mut restarts,
            Some(tokio::time::Instant::now() + limit),
            || {
                attempts += 1;
                let error = keys.select().unwrap_err();
                async move { Err(error.into()) }
            },
            |notice| {
                notices.push(notice);
                async {}
            },
        ),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(
        crate::gemini::startup_failure_reason(error.as_ref()),
        "quota"
    );
    assert!(crate::gemini::exhausted_until(error.as_ref()).is_some());
    assert_eq!(attempts, 1);
    assert_eq!(restarts, 0);
    assert!(notices.is_empty());
}

/// Every configured key refused: the rotation runs dry with no cooldown, and
/// the reason the candidate is given is the provider's refusal, not a bare
/// empty rotation that reads as an unanswered startup check.
#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is a full HTTP response"
)]
async fn a_first_open_refused_on_every_key_reports_the_refusal() {
    use tokio_tungstenite::tungstenite::handshake::server;
    for (status, prefix, expected) in [
        (
            axum::http::StatusCode::UNAUTHORIZED,
            "dry-invalid",
            "credentials",
        ),
        (axum::http::StatusCode::FORBIDDEN, "dry-refused", "refused"),
    ] {
        let configured = format!("{prefix}-first,{prefix}-second");
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEYS", configured.as_str()),
        ])
        .unwrap();
        let keys = GeminiKeys::from_config(&config);
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let _ = tokio_tungstenite::accept_hdr_async(socket, |_: &server::Request, _| {
                    let mut refusal = server::ErrorResponse::new(None);
                    *refusal.status_mut() = status;
                    Err(refusal)
                })
                .await;
            }
        });
        let error = crate::gemini::first_open_within(
            Duration::from_secs(10),
            retry_cold_open(
                &keys,
                &mut 0,
                None,
                || fixture_session_at(&url, &keys, &boot, None),
                |_| async {},
            ),
        )
        .await
        .err()
        .unwrap();
        server.abort();
        assert_eq!(
            crate::gemini::startup_failure_reason(error.as_ref()),
            expected,
            "{prefix}: {error}"
        );
    }
}

/// What a retry moved past stands in only for a rotation that then ran dry.
/// A transient failure followed by the provider's own refusal reports the
/// refusal, not the earlier failure.
#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is a full HTTP response"
)]
async fn a_refusal_after_a_retried_failure_reports_the_refusal() {
    use tokio_tungstenite::tungstenite::handshake::server;
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "retry-then-refused"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for status in [
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::http::StatusCode::UNAUTHORIZED,
        ] {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = tokio_tungstenite::accept_hdr_async(socket, |_: &server::Request, _| {
                let mut refusal = server::ErrorResponse::new(None);
                *refusal.status_mut() = status;
                Err(refusal)
            })
            .await;
        }
    });
    let error = crate::gemini::first_open_within(
        Duration::from_secs(10),
        retry_cold_open(
            &keys,
            &mut 0,
            None,
            || fixture_session_at(&url, &keys, &boot, None),
            |_| async {},
        ),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(
        crate::gemini::startup_failure_reason(error.as_ref()),
        "credentials",
        "{error}"
    );

    // Bounded: a cold open that stopped after the first refusal never comes
    // back for the second, and the peer would wait for it forever.
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("both attempts reached the peer")
        .unwrap();
}

/// A probe the provider refused marks the key it used, but not when the
/// room's own closing ended it; and what arrived while the candidate was
/// awaited and is not media stays queued for the room loop.
#[tokio::test]
async fn a_refused_probe_marks_its_key_only_while_the_room_is_open() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        (
            "GOOGLE_API_KEYS",
            "probe-refused-first,probe-refused-second",
        ),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Text(text) = message
                && text.contains(r#""turnComplete":true"#)
            {
                socket
                    .send(Message::Text(
                        r#"{"error":{"code":401,"message":"invalid API key"}}"#.into(),
                    ))
                    .await
                    .unwrap();
            }
        }
    });
    let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        .await
        .unwrap();
    let mut state = initial_runtime_state(&boot, Instant::now());
    let greeting = with_timer(&state, "opening".to_string());
    let error = probe_live_output(
        &mut gemini,
        &mut state,
        &greeting,
        Duration::from_millis(500),
    )
    .await
    .unwrap_err();
    assert_eq!(
        crate::gemini::startup_failure_reason(error.as_ref()),
        "credentials"
    );

    let kept = crate::livekit::open::attach_waiting_media(
        &mut CandidateMedia::new(),
        &mut CandidateStreams::default(),
        &mut gemini,
        "candidate",
        false,
        vec![
            (Instant::now(), RoomEvent::Reconnecting),
            (Instant::now(), RoomEvent::Reconnected),
        ],
    )
    .await
    .unwrap();
    assert!(matches!(
        kept[..],
        [(_, RoomEvent::Reconnecting), (_, RoomEvent::Reconnected)]
    ));

    crate::livekit::open::record_probe_failure(&gemini, &keys, true);
    assert_eq!(keys.select().unwrap(), "probe-refused-first");
    crate::livekit::open::record_probe_failure(&gemini, &keys, false);
    assert_eq!(keys.select().unwrap(), "probe-refused-second");
    gemini.shutdown().await.unwrap();
    server.abort();
}
