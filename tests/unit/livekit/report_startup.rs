//! Reports for interviews whose startup the room loop had not finished
//! replaying: speech an early End must still freeze, and a report reader that
//! takes over the events held back while ready was being published.

use super::*;

use super::tests::{ReceiptTestEvent, delivery_test_packet, receipt_bytes, report_test_config};
use crate::livekit::readiness::{
    RoomEventReader, StartupContext, probe_live_output, ready_runtime_activity,
    startup_ready_notice,
};
use crate::runtime::bootstrap;

#[tokio::test]
async fn startup_speech_is_frozen_when_end_wins_before_replay() {
    use crate::gemini::TokenUsage;
    use crate::livekit::{packets, room_link::RoomLink, tests, *};
    use futures_util::{FutureExt, SinkExt, StreamExt};
    use std::sync::Arc;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    const FIRST: &str = "I would track the cobalt invoice in a hash map.";
    const SECOND: &str = "The violet receipt is the matching entry we check next.";
    let occurrences = |state: &RuntimeState, text: &str| {
        state
            .transcript
            .iter()
            .filter(|line| line.contains(text))
            .count()
    };
    for (interrupted, replayed, spoken) in [
        (false, 0),
        (false, 1),
        (false, 2),
        (false, 5),
        (true, 0),
        (true, 1),
        (true, 2),
        (true, 3),
        (true, 4),
        (true, 7),
    ]
    .into_iter()
    .map(|(interrupted, replayed)| (interrupted, replayed, true))
    .chain([(false, 0, false), (false, 1, false), (false, 2, false)])
    {
        let config = report_test_config();
        let keys = Arc::new(GeminiKeys::from_config(&config));
        let boot = bootstrap(&config, "startup-end-evidence", Some("two-sum"), 45);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(socket).await.unwrap();
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
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value["clientContent"]["turnComplete"] == true {
                        generations += 1;
                        let mut frames = if spoken {
                            vec![
                                serde_json::json!({"serverContent":{"inputTranscription":{"text":FIRST}}}),
                            ]
                        } else {
                            vec![serde_json::json!({"serverContent":{"turnComplete":true}})]
                        };
                        if spoken && interrupted {
                            frames.extend([
                                serde_json::json!({"serverContent":{"interrupted":true}}),
                                serde_json::json!({"serverContent":{"inputTranscription":{"text":"The violet receipt"}}}),
                                serde_json::json!({"serverContent":{"turnComplete":true}}),
                                serde_json::json!({"serverContent":{"inputTranscription":{"text":" is the matching entry we check next."}}}),
                            ]);
                        } else if spoken {
                            frames.extend([
                                serde_json::json!({"serverContent":{"turnComplete":true}}),
                                serde_json::json!({"serverContent":{"inputTranscription":{"text":SECOND}}}),
                            ]);
                        }
                        frames.extend([
                            serde_json::json!({"usageMetadata":{"promptTokenCount":7,"responseTokenCount":3,"totalTokenCount":10}}),
                            serde_json::json!({"serverContent":{"outputTranscription":{"text":"Welcome to the interview."}}}),
                        ]);
                        for frame in frames {
                            socket
                                .send(Message::Text(frame.to_string().into()))
                                .await
                                .unwrap();
                        }
                    }
                }
            }
            assert!(
                listener.accept().now_or_never().is_none(),
                "no replacement socket"
            );
            generations
        });
        let mut gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
            .await
            .unwrap();
        let mut turn = tests::hold_turn(initial_runtime_state(&boot, Instant::now()));
        let mut media = CandidateMedia::new();
        media.identity = Some("candidate".into());
        let greeting = with_timer(&turn.state, boot.greeting.clone());
        let requested_at = Instant::now();
        let (_input, mut input) = tokio::sync::mpsc::unbounded_channel();
        let started_at = probe_live_output(
            &mut gemini,
            &mut turn.state,
            &greeting,
            Duration::from_secs(5),
            &mut StartupContext {
                media: &mut media,
                candidate_identity: "candidate",
                candidate_video: false,
                deferred_events: &mut Vec::new(),
                room_closed: &mut false,
            },
            &mut input,
        )
        .await
        .unwrap();
        turn.state.started_at = started_at;
        turn.activity = ready_runtime_activity(
            &config,
            boot.room_name,
            started_at,
            requested_at,
            7,
            &greeting,
        );
        let (room, sent) = RoomLink::recording(boot.room_name);
        let mut usage = TokenUsage::default();
        let ready = startup_ready_notice(started_at, turn.activity.startup_ready_at);
        tests::startup::publish_startup_readiness(
            &mut gemini,
            &mut usage,
            publish_interviewer_state(&room, &ready),
        )
        .await
        .unwrap();
        assert!(
            matches!(&sent.take()[0], room_link::Sent::Data { message, .. } if message["startup"] == "ready")
        );
        assert!(
            turn.state.transcript.is_empty(),
            "the probe must buffer, not assess"
        );

        let (_events, events) = tokio::sync::mpsc::unbounded_channel();
        let events = tokio::sync::Mutex::new(RoomEventReader::from(events));
        let interview = InterviewContext {
            config: &config,
            keys: &keys,
            boot: &boot,
            live_endpoint: None,
            started_at,
            candidate_identity: "candidate",
            events: &events,
            slot: None,
        };
        let (mut audio, _frames) = tests::test_output_audio();
        let mut board = Board::new();
        let mut loops = RoomLoop {
            presence: CandidatePresence::default(),
            ..crate::livekit::tests::startup::room_loop(3)
        };
        let restarts_before_end;
        let mut frozen = {
            let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
            for _ in 0..replayed {
                let event = context.gemini.next_event().await.unwrap();
                assert!(
                    on_gemini_event(&room, &mut context, Some(event), &mut loops, interview)
                        .await
                        .unwrap()
                        .is_continue()
                );
            }
            restarts_before_end = loops.restarts;

            // Select the room's End arm ahead of the remaining Gemini events.
            // These are the production reducer and pre-farewell freeze path;
            // neither transcript delivery nor a fake report supplies evidence.
            let ended = packets::reduce_data_packet(
                context.state,
                context.activity,
                crate::runtime::TOPIC_CONTROL,
                &serde_json::json!({"type":"end_interview", "reason":"candidate_ended"}),
                true,
                crate::current_epoch_millis(),
            );
            assert!(ended.finish_interview.is_some());
            assert!(context.state.ended);
            packets::finish_assessment(&room, &mut context, interview).await
        };
        assert_eq!(
            occurrences(&frozen.state, FIRST),
            usize::from(spoken),
            "frozen candidate evidence was lost (replayed={replayed})"
        );
        assert_eq!(occurrences(&frozen.state, SECOND), usize::from(spoken));
        let expected_turns = if spoken { 2 } else { 0 };
        assert_eq!(
            frozen.state.evidence_ledger.conversation.candidate_turns,
            expected_turns
        );
        assert_eq!(
            frozen.rounds().communication_evidence != crate::agent::CommunicationEvidence::NoSpeech,
            spoken
        );
        assert_eq!(frozen.prompt.matches(FIRST).count(), usize::from(spoken));
        assert_eq!(frozen.prompt.matches(SECOND).count(), usize::from(spoken));
        assert!(
            !frozen.prompt.contains("Welcome to the interview.")
                || replayed == if interrupted { 7 } else { 5 }
        );

        // A farewell may still consume the rest of the buffer. It cannot add a
        // second candidate turn or retroactively repair the frozen snapshot.
        while turn
            .state
            .transcript
            .iter()
            .all(|line| !line.contains("Welcome"))
        {
            let event = tokio::time::timeout(Duration::from_secs(5), gemini.next_event())
                .await
                .unwrap()
                .unwrap();
            let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
            session::handle_gemini_event(&room, &mut context, event, Interruptible::No)
                .await
                .unwrap();
        }
        session::close_turns(
            &room,
            &mut turn.context(&mut audio, &mut gemini, &mut board, &mut media),
        );
        assert_eq!(
            turn.state.evidence_ledger.conversation.candidate_turns,
            expected_turns
        );
        assert_eq!(occurrences(&turn.state, FIRST), usize::from(spoken));
        assert_eq!(occurrences(&turn.state, SECOND), usize::from(spoken));
        session::drain_live_usage(
            &room,
            &mut turn.context(&mut audio, &mut gemini, &mut board, &mut media),
        );
        assert_eq!(turn.activity.live_usage.samples, 1);
        assert_eq!(turn.activity.live_usage.total, 10);
        assert_eq!(loops.restarts, restarts_before_end);
        assert_eq!(turn.activity.live_socket, 1);
        gemini.shutdown().await.unwrap();
        assert_eq!(server.await.unwrap(), 1, "the opening is generated once");

        // Farewell/live mutations cannot change the already frozen eligibility.
        turn.state
            .evidence_ledger
            .record_conversation_turn(100, "candidate");
        turn.state
            .transcript
            .push("Candidate: a late crimson comment".into());
        assert_eq!(
            turn.state.evidence_ledger.conversation.candidate_turns,
            expected_turns + 1
        );
        assert_frozen_startup_report(&boot, &keys, &mut frozen, spoken).await;
    }
}

async fn assert_frozen_startup_report(
    boot: &RuntimeBootstrap<'_>,
    keys: &GeminiKeys,
    frozen: &mut FrozenAssessment,
    spoken: bool,
) {
    let mut answer = crate::gemini::tests::valid_report();
    answer["improvementPlan"]
        .as_array_mut()
        .unwrap()
        .truncate(2);
    answer["communicationFeedback"]["improvements"] =
        answer["codingFeedback"]["improvements"].clone();
    if !spoken {
        answer["communicationScore"] = serde_json::Value::Null;
        answer["communicationFeedback"]["strengths"] = serde_json::json!([]);
        answer["communicationFeedback"]["improvements"] = serde_json::json!([]);
    }
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let received = std::sync::Arc::clone(&requests);
    let response = answer.clone();
    let app = axum::Router::new().route("/", axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
        received.lock().unwrap().push(body);
        let response = response.clone();
        async move {
            axum::Json(serde_json::json!({"candidates":[{"content":{"parts":[{"text":response.to_string()}]}}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let http = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let generated = crate::gemini::tests::generate_report_for_round_at(
        keys,
        &endpoint,
        std::time::Duration::ZERO,
        &frozen.prompt,
        boot.problem,
        frozen.rounds(),
        crate::gemini::ReportRun {
            scope: boot.room_name,
            seed: crate::gemini::GENERATION_SEED,
            refused: &frozen.refused,
        },
    )
    .await
    .unwrap();
    stop.send(()).unwrap();
    http.await.unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "no semantic repair should be needed");
    assert_eq!(
        requests[0]["contents"][0]["parts"][0]["text"],
        frozen.prompt
    );
    assert!(!frozen.prompt.contains("late crimson comment"));
    let schema = &requests[0]["generationConfig"]["responseSchema"];
    assert_eq!(*schema, crate::agent::report_response_schema(spoken));
    assert_eq!(
        schema["properties"]["communicationFeedback"]["properties"]["strengths"]["maxItems"],
        if spoken { 4 } else { 0 }
    );
    let report = report_value(
        boot,
        &mut frozen.state,
        "candidate_ended",
        keys,
        Ok(Ok(generated)),
    );
    assert!(report.get("incomplete").is_none(), "{report}");
    assert_eq!(report["codingScore"], 82);
    assert_eq!(report["decision"], "HIRE");
    assert_eq!(report["communicationScore"], answer["communicationScore"]);
    assert_eq!(
        report["communicationFeedback"],
        answer["communicationFeedback"]
    );

    // Final publication independently rejects the opposite eligibility policy.
    answer["communicationScore"] = if spoken {
        serde_json::Value::Null
    } else {
        serde_json::json!(74)
    };
    let invalid = report_value(
        boot,
        &mut frozen.state,
        "candidate_ended",
        keys,
        Ok(Ok(answer)),
    );
    assert_eq!(invalid["incomplete"], true);
}

#[tokio::test]
async fn deferred_report_reader_keeps_skip_authorization_and_recovery_order() {
    use crate::livekit::tests::startup::publication::candidate_room;
    use futures_util::FutureExt;
    let (room, _sdk_events, _arrive, server) = candidate_room("candidate", "{}", true).await;
    let candidate = room.remote_participants().into_values().next().unwrap();
    let packet = |kind, topic: &str, participant| RoomEvent::DataReceived {
        topic: Some(topic.into()),
        payload: std::sync::Arc::new(
            serde_json::to_vec(&serde_json::json!({"type":kind})).unwrap(),
        ),
        kind: ::livekit::DataPacketKind::Reliable,
        participant,
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let at = std::time::Instant::now();
    let mut events = RoomEventReader {
        events: rx,
        startup_sync_pending: false,
        deferred: [
            (at, packet("skip_report", "control", None)),
            (
                at,
                packet("skip_report", "wrong-topic", Some(candidate.clone())),
            ),
        ]
        .into(),
    };
    assert!(
        wait_for_skip(&mut events, "candidate")
            .now_or_never()
            .is_none()
    );
    tx.send(packet("skip_report", "control", Some(candidate.clone())))
        .unwrap();
    assert!(
        wait_for_skip(&mut events, "candidate")
            .now_or_never()
            .is_some()
    );

    events
        .deferred
        .push_back((at, packet("retry_report", "control", Some(candidate))));
    tx.send(RoomEvent::Disconnected {
        reason: ::livekit::DisconnectReason::ClientInitiated,
    })
    .unwrap();
    let mut recovery = LiveRecoveryRoom {
        room: &room,
        room_name: "handoff-recovery",
        candidate: "candidate",
        events: &mut events,
    };
    assert_eq!(recovery.next().await, RecoveryEvent::Retry);
    assert_eq!(recovery.next().await, RecoveryEvent::Left);
    room.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn deferred_report_receipt_precedes_new_events_even_after_departure() {
    for present in [true, false] {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let payload = br#"{"codingScore":80}"#.to_vec();
        let id = crate::sha256_hex(&[&payload]);
        let receipt = |sender| ReceiptTestEvent {
            sender,
            payload: receipt_bytes(&id),
        };
        let at = std::time::Instant::now();
        let mut events = RoomEventReader {
            events: rx,
            startup_sync_pending: false,
            deferred: [(at, receipt("peer")), (at, receipt("candidate"))].into(),
        };
        tx.send(receipt("newer-event")).unwrap();
        let mut publishes = 0;
        assert!(
            deliver_report(
                |_| {
                    publishes += 1;
                    std::future::ready(Ok::<_, std::io::Error>(()))
                },
                &mut events,
                "candidate",
                "handoff-receipt",
                || present,
                delivery_test_packet(payload),
            )
            .await
            .unwrap()
        );
        assert_eq!(publishes, 1);
        assert_eq!(events.try_recv().unwrap().sender, "newer-event");
    }
}
